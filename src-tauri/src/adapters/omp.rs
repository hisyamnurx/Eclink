//! Oh My Pi (`omp`) adapter.
//!
//! OMP is not a clone of Codex or Claude Code. It keeps a provider registry in
//! `models.yml` and model-role assignments in `config.yml`, both YAML, both
//! user-editable, and it resolves provider credentials from a command supplied
//! with a leading `!`. This adapter therefore:
//!
//! * edits the two YAML files losslessly (`yaml-edit`) rather than serialising a
//!   parsed document, because comments, anchors, and key order belong to the
//!   user;
//! * writes only a namespaced provider it can prove it owns, and records every
//!   provider ID plus the roles it changed in a local ledger;
//! * never writes a key. `apiKey` is `!<helper> secret resolve <profile-id>`,
//!   resolved by OMP at request time from DPAPI;
//! * snapshots both files together, so restore and undo cannot leave a role
//!   pointing at a provider that no longer exists.
//!
//! OMP named profiles are supported end to end. The selected profile is kept in
//! [`OmpAdapter`] as shared state so detection, preview, apply, restore, and
//! launch all resolve the same directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use yaml_edit::Document;

use crate::adapters::detect::{clean_version, version_at_least};
use crate::adapters::{
    ActiveConfiguration, AgentAdapter, AgentDetection, AgentDiagnostics, ApplyOptions,
    ApplyOutcome, ConfigParseState, ConfigSnapshot, LaunchPlan, MutationReason,
};
use crate::backup::{BackupManager, Snapshot, SnapshotBundle, SnapshotBundleFile};
use crate::config::fs::{read_to_string_opt, sha256_hex, write_atomic, FileLock};
use crate::error::{BridgeError, ErrorKind};
use crate::profiles::{
    omp_managed_provider_id, validate_omp_selector, AgentId, CompatibilityStatus, Profile,
};

/// Minimum OMP version whose models/provider and profile contracts Eclink has
/// verified. Older builds are reported but not silently treated as compatible.
pub const MIN_OMP_VERSION: (u64, u64, u64) = (18, 0, 0);
/// Human-readable form of [`MIN_OMP_VERSION`] for messages.
pub const MIN_OMP_VERSION_LABEL: &str = "18.0.0";

const DEFAULT_PROFILE: &str = "default";
#[allow(dead_code)]
const SUPPORTED_APIS: [&str; 3] = [
    "openai-completions",
    "openai-responses",
    "anthropic-messages",
];

/// The subset of `omp models --json` Eclink verifies after a write.
#[derive(Debug, Deserialize)]
struct OmpModelsList {
    models: Vec<OmpListedModel>,
}

#[derive(Debug, Deserialize)]
struct OmpListedModel {
    provider: String,
    id: String,
}

fn default_profile() -> String {
    DEFAULT_PROFILE.to_string()
}

/// Paths OMP resolves for one profile.
#[derive(Debug, Clone)]
pub struct OmpPaths {
    pub base_root: PathBuf,
    pub config_dir: PathBuf,
    pub models_path: PathBuf,
    pub config_path: PathBuf,
    pub profile: String,
    pub from_env: bool,
}

impl OmpPaths {
    fn resolve(profile: &str) -> Result<Self, BridgeError> {
        let profile = normalize_profile_name(profile)?;
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .ok_or_else(|| {
                BridgeError::internal("Eclink could not determine the user home directory.")
            })?;
        let base_root = std::env::var_os("PI_CONFIG_DIR")
            .map(PathBuf::from)
            .map(|value| {
                if value.is_absolute() {
                    value
                } else {
                    home.join(value)
                }
            })
            .unwrap_or_else(|| home.join(".omp"));

        let (config_dir, from_env) = if profile == DEFAULT_PROFILE {
            match std::env::var_os("PI_CODING_AGENT_DIR")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
            {
                Some(path) => (path, true),
                None => (base_root.join("agent"), false),
            }
        } else {
            (
                base_root.join("profiles").join(&profile).join("agent"),
                true,
            )
        };

        Ok(Self {
            models_path: preferred_yaml(&config_dir, "models.yml", "models.yaml"),
            config_path: preferred_yaml(&config_dir, "config.yml", "config.yaml"),
            base_root,
            config_dir,
            profile,
            from_env,
        })
    }

    /// All paths a snapshot must capture.
    fn bundle_paths(&self) -> [PathBuf; 2] {
        [self.models_path.clone(), self.config_path.clone()]
    }
}

/// OMP's canonical write target is `.yml`; if the user already has the legacy
/// `.yaml` spelling and no `.yml`, edit their file rather than creating a
/// competing one that OMP would also read. Checked in OMP's precedence order.
fn preferred_yaml(dir: &Path, canonical: &str, fallback: &str) -> PathBuf {
    let canonical_path = dir.join(canonical);
    if canonical_path.exists() {
        return canonical_path;
    }
    let fallback_path = dir.join(fallback);
    if fallback_path.exists() {
        return fallback_path;
    }
    canonical_path
}

/// Validate an OMP named profile using OMP's own rules.
pub fn normalize_profile_name(value: &str) -> Result<String, BridgeError> {
    let value = value.trim();
    if value.eq_ignore_ascii_case(DEFAULT_PROFILE) {
        return Ok(default_profile());
    }
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        });
    const RESERVED: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let stem = value.split('.').next().unwrap_or(value);
    if !valid || RESERVED.contains(&stem) {
        return Err(
            BridgeError::validation("That is not a valid Oh My Pi profile name.").with_hint(
                "Use lowercase letters, digits, dots, underscores, or hyphens (1–64 characters). \
             The name must start with a letter or digit and may not be a reserved device name.",
            ),
        );
    }
    Ok(value.to_string())
}

/// A named OMP profile as the UI lists it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OmpProfileSummary {
    pub name: String,
    pub label: String,
    pub config_dir: String,
    pub exists: bool,
}

/// A provider that a preview would create or replace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OmpPreview {
    pub profile: String,
    pub config_dir: String,
    pub models_path: String,
    pub config_path: String,
    pub provider_id: String,
    pub selector: String,
    pub api: String,
    pub base_url: String,
    pub model: String,
    pub roles: Vec<OmpRolePreview>,
    pub provider_exists: bool,
    /// True only when the ledger proves the existing entry is Eclink's.
    pub owned: bool,
    pub conflict: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OmpRolePreview {
    pub role: String,
    pub selector: String,
    pub previous: Option<String>,
}

/// The local ownership ledger.
///
/// It contains no credential and no config content: only IDs, paths, hashes,
/// and the role values Eclink is entitled to put back.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OmpLedger {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: Vec<OmpLedgerEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OmpLedgerEntry {
    profile_id: String,
    profile_name: String,
    provider_id: String,
    omp_profile: String,
    config_dir: String,
    models_path: String,
    config_path: String,
    selector: String,
    active: bool,
    models_sha256: String,
    config_sha256: String,
    updated_at: String,
    /// The values Eclink overwrote the first time it touched each role. `None`
    /// means the role did not exist.
    #[serde(default)]
    previous_roles: BTreeMap<String, Option<String>>,
    /// The value Eclink most recently wrote for each role, so restore never
    /// reverts a role the user has since changed by hand.
    #[serde(default)]
    assigned_roles: BTreeMap<String, String>,
}

/// One serialisable provider entry. Kept private: this is only a template for
/// the YAML snippet, never the whole file.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderTemplate {
    base_url: String,
    api: String,
    api_key: String,
    auth_header: bool,
    models: Vec<ModelTemplate>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelTemplate {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ModelsDoc {
    #[serde(default)]
    providers: BTreeMap<String, ProviderDoc>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct ProviderDoc {
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    models: Vec<ModelDoc>,
}

#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
struct ModelDoc {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ConfigDoc {
    #[serde(default)]
    model_roles: BTreeMap<String, String>,
}

/// Whether `key` exists in `raw` with an explicit null value (`key:` with no
/// children). An empty section is a normal starting point; a scalar or a list
/// is not, and Eclink must refuse to replace it.
fn key_is_explicit_null(raw: Option<&str>, key: &str) -> bool {
    let Some(raw) = raw else { return false };
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(raw) else {
        return false;
    };
    value
        .get(key)
        .is_some_and(|node| matches!(node, serde_yaml::Value::Null))
}

/// Storage key for one OMP profile's immutable original. OMP is the only agent
/// whose configuration set depends on a selected profile, so each profile gets
/// its own original instead of sharing one directory.
fn original_key(profile: &str) -> String {
    format!("omp__{profile}")
}

/// Shared selection of the OMP profile Eclink manages.
#[derive(Debug, Clone)]
pub struct OmpSelection(Arc<RwLock<String>>);

impl OmpSelection {
    pub fn new(profile: impl Into<String>) -> Self {
        let profile = profile.into();
        let normalized = normalize_profile_name(&profile).unwrap_or_else(|_| default_profile());
        Self(Arc::new(RwLock::new(normalized)))
    }

    pub fn get(&self) -> String {
        self.0
            .read()
            .map(|value| value.clone())
            .unwrap_or_else(|_| default_profile())
    }

    pub fn set(&self, profile: &str) -> Result<String, BridgeError> {
        let profile = normalize_profile_name(profile)?;
        if let Ok(mut guard) = self.0.write() {
            *guard = profile.clone();
        }
        Ok(profile)
    }
}

/// Oh My Pi adapter.
#[derive(Clone)]
pub struct OmpAdapter {
    backups: BackupManager,
    ledger_path: PathBuf,
    selection: OmpSelection,
}

impl OmpAdapter {
    pub fn new(backups: BackupManager, ledger_path: PathBuf, selection: OmpSelection) -> Self {
        Self {
            backups,
            ledger_path,
            selection,
        }
    }

    pub fn selection(&self) -> OmpSelection {
        self.selection.clone()
    }

    pub fn selected_profile(&self) -> String {
        self.selection.get()
    }

    pub fn set_profile(&self, profile: &str) -> Result<String, BridgeError> {
        self.selection.set(profile)
    }

    pub fn paths(&self) -> Result<OmpPaths, BridgeError> {
        OmpPaths::resolve(&self.selected_profile())
    }

    fn lock(&self) -> Result<FileLock, BridgeError> {
        FileLock::acquire(self.backups.root().join("omp-config.lock"))
    }

    /// List the default profile and every named profile directory.
    pub fn list_profiles(&self) -> Result<Vec<OmpProfileSummary>, BridgeError> {
        let selected = self.selected_profile();
        let paths = self.paths()?;
        let mut profiles = vec![OmpProfileSummary {
            name: default_profile(),
            label: "Default".to_string(),
            config_dir: OmpPaths::resolve(DEFAULT_PROFILE)?
                .config_dir
                .display()
                .to_string(),
            exists: OmpPaths::resolve(DEFAULT_PROFILE)?.config_dir.exists(),
        }];
        let profiles_root = paths.base_root.join("profiles");
        if let Ok(entries) = std::fs::read_dir(&profiles_root) {
            let mut named: Vec<OmpProfileSummary> = entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    normalize_profile_name(&name).ok().map(|name| {
                        let config_dir = entry.path().join("agent");
                        OmpProfileSummary {
                            exists: config_dir.exists(),
                            label: name.clone(),
                            config_dir: config_dir.display().to_string(),
                            name,
                        }
                    })
                })
                .collect();
            named.sort_by(|a, b| a.name.cmp(&b.name));
            profiles.extend(named);
        }
        // The selected profile might not have a directory yet; still show it.
        if !profiles.iter().any(|profile| profile.name == selected) {
            profiles.push(OmpProfileSummary {
                name: selected.clone(),
                label: selected,
                config_dir: paths.config_dir.display().to_string(),
                exists: false,
            });
        }
        Ok(profiles)
    }

    fn read_ledger(&self) -> Result<OmpLedger, BridgeError> {
        let Some(raw) = read_to_string_opt(&self.ledger_path)? else {
            return Ok(OmpLedger {
                version: 1,
                entries: Vec::new(),
            });
        };
        match serde_json::from_str::<OmpLedger>(&raw) {
            Ok(mut ledger) => {
                if ledger.version == 0 {
                    ledger.version = 1;
                }
                Ok(ledger)
            }
            Err(err) => {
                // Losing the ownership ledger must not make Eclink claim a
                // provider it cannot prove it wrote. Treat it as empty, which
                // makes activation refuse a collision instead of overwriting.
                tracing::warn!(
                    target: "eclink",
                    error = %err,
                    "OMP ownership ledger is unreadable; refusing to claim existing providers"
                );
                Ok(OmpLedger {
                    version: 1,
                    entries: Vec::new(),
                })
            }
        }
    }

    fn write_ledger(&self, ledger: &OmpLedger) -> Result<(), BridgeError> {
        let json = serde_json::to_string_pretty(ledger).map_err(|err| {
            BridgeError::internal("Could not record the OMP ownership ledger.")
                .with_details(err.to_string())
        })?;
        if let Some(parent) = self.ledger_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_atomic(&self.ledger_path, json.as_bytes())?;
        let _ = crate::config::fs::harden_permissions(&self.ledger_path);
        Ok(())
    }

    fn ledger_entry_for(
        &self,
        profile: &Profile,
        provider_id: &str,
    ) -> Result<Option<OmpLedgerEntry>, BridgeError> {
        let ledger = self.read_ledger()?;
        Ok(ledger
            .entries
            .into_iter()
            .find(|entry| entry.profile_id == profile.id && entry.provider_id == provider_id))
    }

    /// Read both YAML files. `None` means the file did not exist.
    fn read_pair(&self, paths: &OmpPaths) -> Result<(Option<String>, Option<String>), BridgeError> {
        Ok((
            read_to_string_opt(&paths.models_path)?,
            read_to_string_opt(&paths.config_path)?,
        ))
    }

    fn parse_models(&self, raw: Option<&str>) -> Result<ModelsDoc, BridgeError> {
        let Some(raw) = raw else {
            return Ok(ModelsDoc::default());
        };
        if raw.trim().is_empty() {
            return Ok(ModelsDoc::default());
        }
        serde_yaml::from_str::<ModelsDoc>(raw).map_err(|err| {
            BridgeError::config_invalid(
                "Eclink could not read the Oh My Pi models file, so it did not change anything.",
            )
            .with_hint("Fix the YAML in models.yml, then try again.")
            .with_details(err.to_string())
        })
    }

    fn parse_config(&self, raw: Option<&str>) -> Result<ConfigDoc, BridgeError> {
        let Some(raw) = raw else {
            return Ok(ConfigDoc::default());
        };
        if raw.trim().is_empty() {
            return Ok(ConfigDoc::default());
        }
        serde_yaml::from_str::<ConfigDoc>(raw).map_err(|err| {
            BridgeError::config_invalid(
                "Eclink could not read the Oh My Pi config file, so it did not change anything.",
            )
            .with_hint("Fix the YAML in config.yml, then try again.")
            .with_details(err.to_string())
        })
    }

    fn provider_template(
        &self,
        profile: &Profile,
        helper: &Path,
    ) -> Result<ProviderTemplate, BridgeError> {
        let helper = helper.display().to_string();
        if helper.contains('"') || helper.contains('\'') {
            return Err(BridgeError::internal(
                "The credential helper path contains an unsupported character.",
            ));
        }
        let command = format!("\"{helper}\" secret resolve {}", profile.id);
        Ok(ProviderTemplate {
            base_url: crate::profiles::normalize_base_url(&profile.base_url),
            api: profile.omp.protocol.as_omp_str().to_string(),
            api_key: format!("!{command}"),
            auth_header: true,
            models: vec![ModelTemplate {
                id: profile.wire_model().to_string(),
                name: profile
                    .omp
                    .model_name
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| profile.model.clone()),
                context_window: profile.omp.context_window,
                max_tokens: profile.omp.max_tokens,
            }],
        })
    }

    fn selector_for(&self, profile: &Profile, provider_id: &str) -> String {
        let fallback = format!("{provider_id}/{}", profile.wire_model());
        profile
            .omp
            .default_model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or(fallback)
    }

    fn roles_for(&self, profile: &Profile, provider_id: &str) -> Vec<(String, String)> {
        profile
            .omp
            .role_assignments(&self.selector_for(profile, provider_id))
    }

    /// Build the new models.yml from the user's current text.
    fn edit_models(
        &self,
        raw: Option<&str>,
        provider_id: &str,
        provider: &ProviderTemplate,
    ) -> Result<String, BridgeError> {
        let template = serde_yaml::to_string(provider).map_err(|err| {
            BridgeError::internal("Could not build the OMP provider entry.")
                .with_details(err.to_string())
        })?;
        let snippet = format!(
            "providers:\n  {provider_id}:\n{}",
            indent_yaml(&template, 4)
        );
        let parsed = Document::from_str(&snippet).map_err(|err| {
            BridgeError::internal("Could not build the OMP provider YAML.")
                .with_details(err.to_string())
        })?;
        let parsed_root = parsed
            .as_mapping()
            .ok_or_else(|| BridgeError::internal("Could not build the OMP provider YAML."))?;
        let provider_mapping = parsed_root
            .get_mapping("providers")
            .and_then(|providers| providers.get_mapping(provider_id))
            .ok_or_else(|| BridgeError::internal("Could not build the OMP provider YAML."))?;

        let base = match raw.filter(|text| !text.trim().is_empty()) {
            Some(text) => text.to_string(),
            None => "providers: {}\n".to_string(),
        };
        let doc = Document::from_str(&base).map_err(|err| {
            BridgeError::config_invalid("Eclink could not edit models.yml safely.")
                .with_details(err.to_string())
        })?;
        let root = doc.as_mapping().ok_or_else(|| {
            BridgeError::config_invalid("Eclink could not edit models.yml safely.")
                .with_hint("The top level of models.yml must be a mapping.")
        })?;

        if let Some(providers) = root.get_mapping("providers") {
            providers.set(provider_id, provider_mapping);
        } else if root.get("providers").is_some() {
            if !key_is_explicit_null(raw, "providers") {
                return Err(BridgeError::config_invalid(
                    "The `providers` key in models.yml is not a mapping, so Eclink did not change it.",
                ));
            }
            root.set("providers", parsed_root.get_mapping("providers").unwrap());
        } else {
            root.set("providers", parsed_root.get_mapping("providers").unwrap());
        }

        let output = doc.to_string();
        // Validate before the caller writes anything: a lossless editor can
        // still produce invalid YAML if the input had duplicate keys or a
        // non-mapping value we did not anticipate.
        self.parse_models(Some(&output))?;
        if !output.ends_with('\n') {
            Ok(format!("{output}\n"))
        } else {
            Ok(output)
        }
    }

    /// Remove only the provider Eclink owns.
    fn remove_provider(&self, raw: &str, provider_id: &str) -> Result<String, BridgeError> {
        let doc = Document::from_str(raw).map_err(|err| {
            BridgeError::config_invalid("Eclink could not edit models.yml safely.")
                .with_details(err.to_string())
        })?;
        let Some(root) = doc.as_mapping() else {
            return Err(BridgeError::config_invalid(
                "Eclink could not edit models.yml safely.",
            ));
        };
        if let Some(providers) = root.get_mapping("providers") {
            providers.remove(provider_id);
        }
        Ok(doc.to_string())
    }

    /// Apply role assignments, preserving every role not in `roles`.
    fn edit_roles(
        &self,
        raw: Option<&str>,
        roles: &[(String, String)],
    ) -> Result<String, BridgeError> {
        let base = match raw.filter(|text| !text.trim().is_empty()) {
            Some(text) => text.to_string(),
            None => "modelRoles: {}\n".to_string(),
        };
        let doc = Document::from_str(&base).map_err(|err| {
            BridgeError::config_invalid("Eclink could not edit config.yml safely.")
                .with_details(err.to_string())
        })?;
        let root = doc.as_mapping().ok_or_else(|| {
            BridgeError::config_invalid("Eclink could not edit config.yml safely.")
                .with_hint("The top level of config.yml must be a mapping.")
        })?;
        if let Some(model_roles) = root.get_mapping("modelRoles") {
            for (role, selector) in roles {
                model_roles.set(role, selector);
            }
        } else if root.get("modelRoles").is_some() {
            if !key_is_explicit_null(raw, "modelRoles") {
                return Err(BridgeError::config_invalid(
                    "The `modelRoles` key in config.yml is not a mapping, so Eclink did not change it.",
                ));
            }
            let mapping = yaml_edit::Mapping::new();
            for (role, selector) in roles {
                mapping.set(role, selector);
            }
            root.set("modelRoles", mapping);
        } else {
            let mapping = yaml_edit::Mapping::new();
            for (role, selector) in roles {
                mapping.set(role, selector);
            }
            root.set("modelRoles", mapping);
        }
        let output = doc.to_string();
        self.parse_config(Some(&output))?;
        if !output.ends_with('\n') {
            Ok(format!("{output}\n"))
        } else {
            Ok(output)
        }
    }

    /// Build the preview without touching disk.
    pub fn preview(&self, profile: &Profile) -> Result<OmpPreview, BridgeError> {
        if !profile.targets_agent(AgentId::Omp) {
            return Err(BridgeError::validation(
                "This profile is not enabled for Oh My Pi.",
            ));
        }
        profile.validate().map_err(BridgeError::validation)?;
        let paths = self.paths()?;
        let (models_raw, config_raw) = self.read_pair(&paths)?;
        let models = self.parse_models(models_raw.as_deref())?;
        let config = self.parse_config(config_raw.as_deref())?;

        let provider_id = omp_managed_provider_id(profile);
        let provider_exists = models.providers.contains_key(&provider_id);
        let entry = self.ledger_entry_for(profile, &provider_id)?;
        let owned = provider_exists
            && entry
                .as_ref()
                .is_some_and(|entry| entry.omp_profile == paths.profile);
        let conflict = if provider_exists && !owned {
            Some(format!(
                "The provider ID `{provider_id}` already exists in models.yml and is not recorded \
                 as Eclink-owned. Eclink will not overwrite it. Rename or duplicate the Eclink \
                 profile to get a new provider ID."
            ))
        } else {
            None
        };

        let roles = self
            .roles_for(profile, &provider_id)
            .into_iter()
            .map(|(role, selector)| OmpRolePreview {
                previous: config.model_roles.get(&role).cloned(),
                role,
                selector,
            })
            .collect();

        let mut notes = Vec::new();
        if paths.from_env {
            notes.push(format!(
                "Using Oh My Pi profile “{}” at {}.",
                paths.profile,
                paths.config_dir.display()
            ));
        }
        if !paths.config_dir.exists() {
            notes.push("The OMP config directory will be created.".to_string());
        }
        if !models_raw
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
        {
            notes.push("models.yml does not exist yet; Eclink will create it.".to_string());
        }
        if models.providers.get(&provider_id).is_some_and(|provider| {
            !provider
                .models
                .iter()
                .any(|model| model.id == profile.wire_model())
        }) {
            notes.push(
                "The existing Eclink provider entry does not list this model; activation will replace \
                 that entry while leaving every other provider untouched."
                    .to_string(),
            );
        }

        Ok(OmpPreview {
            profile: paths.profile.clone(),
            config_dir: paths.config_dir.display().to_string(),
            models_path: paths.models_path.display().to_string(),
            config_path: paths.config_path.display().to_string(),
            provider_id,
            selector: self.selector_for(profile, &omp_managed_provider_id(profile)),
            api: profile.omp.protocol.as_omp_str().to_string(),
            base_url: crate::profiles::normalize_base_url(&profile.base_url),
            model: profile.wire_model().to_string(),
            roles,
            provider_exists,
            owned,
            conflict,
            notes,
        })
    }

    fn find_omp() -> Option<PathBuf> {
        // Prefer PATH: it is the installation the user's own shell and agent
        // launcher will use. Known install locations are fallbacks for a GUI
        // process that inherited a PATH without the Bun or installer entry.
        let output = Command::new("where.exe")
            .arg("omp.exe")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags_no_window()
            .output()
            .ok();
        if let Some(output) = output {
            if let Some(path) = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .find(|path| path.is_file())
            {
                return Some(path);
            }
        }

        let mut candidates = Vec::new();
        if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
            candidates.push(home.join(".bun").join("bin").join("omp.exe"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            candidates.push(local.join("omp").join("omp.exe"));
            candidates.push(local.join("Programs").join("omp").join("omp.exe"));
        }
        candidates.into_iter().find(|path| path.is_file())
    }

    fn version_of(executable: &Path) -> Result<String, BridgeError> {
        let output = Command::new(executable)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags_no_window()
            .output()
            .map_err(|err| {
                BridgeError::new(
                    ErrorKind::AgentUnavailable,
                    "Eclink could not run Oh My Pi.",
                )
                .with_details(err.to_string())
            })?;
        if !output.status.success() {
            return Err(BridgeError::new(
                ErrorKind::AgentUnavailable,
                "Oh My Pi did not report its version.",
            ));
        }
        let version = clean_version(&String::from_utf8_lossy(&output.stdout));
        if crate::adapters::detect::parse_version(&version).is_none() {
            return Err(BridgeError::new(
                ErrorKind::AgentUnavailable,
                "Oh My Pi returned an unreadable version.",
            ));
        }
        Ok(version)
    }

    /// Verify the provider and selector through OMP's own registry.
    fn verify_registry(&self, provider_id: &str, model: &str) -> Result<(), BridgeError> {
        let Some(executable) = Self::find_omp() else {
            return Ok(());
        };
        let paths = self.paths()?;
        // Point OMP at the exact agent directory being edited. This works for
        // both the default and a named profile and avoids combining --profile
        // with an already-resolved directory.
        let output = Command::new(&executable)
            .args(["models", "--json"])
            .env("PI_CODING_AGENT_DIR", &paths.config_dir)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags_no_window()
            .output()
            .map_err(|err| {
                BridgeError::new(
                    ErrorKind::AgentUnavailable,
                    "Eclink could not ask Oh My Pi to validate the new provider.",
                )
                .with_details(err.to_string())
            })?;
        if !output.status.success() {
            return Err(BridgeError::new(
                ErrorKind::ConfigInvalid,
                "Oh My Pi rejected the updated model configuration.",
            )
            .with_hint("Restore the original configuration and check the YAML by hand.")
            .with_details(String::from_utf8_lossy(&output.stderr).to_string()));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let listed = serde_json::from_str::<OmpModelsList>(&stdout).map_err(|err| {
            BridgeError::new(
                ErrorKind::ConfigInvalid,
                "Oh My Pi returned a model list Eclink could not read.",
            )
            .with_details(err.to_string())
        })?;
        let found = listed
            .models
            .iter()
            .any(|entry| entry.provider == provider_id && entry.id == model);
        if !found {
            return Err(BridgeError::new(
                ErrorKind::ConfigInvalid,
                "Oh My Pi wrote the provider but did not list the selected model.",
            )
            .with_hint(
                "Eclink rolled the change back. Check that the model ID matches the gateway's \
                 /models response exactly.",
            )
            .with_code("OMP_MODEL_NOT_REGISTERED"));
        }
        Ok(())
    }

    fn snapshot_pair(
        &self,
        paths: &OmpPaths,
        reason: MutationReason,
        profile_id: Option<&str>,
    ) -> Result<Snapshot, BridgeError> {
        let (models, config) = self.read_pair(paths)?;
        let files = vec![
            SnapshotBundleFile {
                path: paths.models_path.display().to_string(),
                content: models,
            },
            SnapshotBundleFile {
                path: paths.config_path.display().to_string(),
                content: config,
            },
        ];
        self.backups.snapshots().create_bundle_sealed(
            AgentId::Omp.as_str(),
            &files,
            reason.code(),
            profile_id,
            &[],
        )
    }

    fn restore_files(
        &self,
        paths: &OmpPaths,
        changes: &[(PathBuf, Option<String>)],
    ) -> Result<(), BridgeError> {
        if let Some(parent) = paths.config_dir.parent() {
            let _ = parent;
        }
        std::fs::create_dir_all(&paths.config_dir)?;
        let mut written: Vec<(PathBuf, Option<String>)> = Vec::new();
        for (path, after) in changes {
            let before = read_to_string_opt(path)?;
            let result = match after {
                Some(content) => write_atomic(path, content.as_bytes()),
                None => {
                    if path.exists() {
                        std::fs::remove_file(path).map_err(BridgeError::from)
                    } else {
                        Ok(())
                    }
                }
            };
            if let Err(err) = result {
                for (path, content) in written.into_iter().rev() {
                    let _ = match content {
                        Some(content) => write_atomic(&path, content.as_bytes()),
                        None => std::fs::remove_file(&path).map_err(BridgeError::from),
                    };
                }
                return Err(err);
            }
            written.push((path.clone(), before));
        }
        Ok(())
    }

    fn outcome(
        &self,
        paths: &OmpPaths,
        snapshot_id: String,
        created_original_backup: bool,
    ) -> Result<ApplyOutcome, BridgeError> {
        Ok(ApplyOutcome {
            agent: AgentId::Omp,
            config_path: paths.models_path.display().to_string(),
            snapshot_id,
            created_original_backup,
            managed_keys: vec![format!(
                "models.yml:providers:{}",
                self.selected_provider_marker()
            )],
            active: self.get_active_profile()?,
        })
    }

    fn selected_provider_marker(&self) -> String {
        self.read_ledger()
            .ok()
            .and_then(|ledger| {
                ledger
                    .entries
                    .into_iter()
                    .find(|entry| entry.active && entry.omp_profile == self.selected_profile())
                    .map(|entry| entry.provider_id)
            })
            .unwrap_or_else(|| "managed".to_string())
    }

    #[allow(clippy::too_many_arguments)]
    fn update_ledger_after_apply(
        &self,
        profile: &Profile,
        paths: &OmpPaths,
        provider_id: &str,
        selector: &str,
        roles: &[(String, String)],
        config: &ConfigDoc,
        had_entry: Option<&OmpLedgerEntry>,
    ) -> Result<(), BridgeError> {
        let mut ledger = self.read_ledger()?;
        ledger.version = 1;
        let now = chrono::Local::now().to_rfc3339();
        let existing_index = ledger
            .entries
            .iter()
            .position(|entry| entry.profile_id == profile.id && entry.provider_id == provider_id);

        let mut previous_roles = had_entry
            .map(|entry| entry.previous_roles.clone())
            .unwrap_or_default();
        let mut assigned_roles = BTreeMap::new();
        for (role, value) in roles {
            previous_roles
                .entry(role.clone())
                .or_insert_with(|| config.model_roles.get(role).cloned());
            assigned_roles.insert(role.clone(), value.clone());
        }

        let entry = OmpLedgerEntry {
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            provider_id: provider_id.to_string(),
            omp_profile: paths.profile.clone(),
            config_dir: paths.config_dir.display().to_string(),
            models_path: paths.models_path.display().to_string(),
            config_path: paths.config_path.display().to_string(),
            selector: selector.to_string(),
            active: true,
            models_sha256: sha256_hex(&std::fs::read(&paths.models_path).unwrap_or_default()),
            config_sha256: sha256_hex(&std::fs::read(&paths.config_path).unwrap_or_default()),
            updated_at: now,
            previous_roles,
            assigned_roles,
        };
        // Only one entry per OMP profile may be active.
        for other in &mut ledger.entries {
            if other.omp_profile == paths.profile {
                other.active = false;
            }
        }
        if let Some(index) = existing_index {
            ledger.entries[index] = entry;
        } else {
            ledger.entries.push(entry);
        }
        self.write_ledger(&ledger)
    }

    /// Remove only the role values Eclink wrote, never a later user edit.
    fn restore_roles(
        &self,
        raw: Option<&str>,
        entry: &OmpLedgerEntry,
    ) -> Result<String, BridgeError> {
        let Some(raw) = raw else {
            return Ok(String::new());
        };
        let doc = Document::from_str(raw).map_err(|err| {
            BridgeError::config_invalid("Eclink could not edit config.yml safely.")
                .with_details(err.to_string())
        })?;
        let Some(root) = doc.as_mapping() else {
            return Err(BridgeError::config_invalid(
                "Eclink could not edit config.yml safely.",
            ));
        };
        let Some(roles) = root.get_mapping("modelRoles") else {
            return Ok(doc.to_string());
        };
        for (role, previous) in &entry.previous_roles {
            let current = roles
                .get(role)
                .and_then(|node| node.as_scalar().map(|scalar| scalar.as_string()));
            let assigned = entry.assigned_roles.get(role);
            if assigned.is_some() && current.as_deref() != assigned.map(String::as_str) {
                // The user changed this role after Eclink wrote it. Leave it.
                continue;
            }
            match previous {
                Some(value) => roles.set(role, value.as_str()),
                None => {
                    roles.remove(role);
                }
            }
        }
        Ok(doc.to_string())
    }

    fn owner_entry_for_active(
        &self,
        paths: &OmpPaths,
    ) -> Result<Option<OmpLedgerEntry>, BridgeError> {
        Ok(self
            .read_ledger()?
            .entries
            .into_iter()
            .find(|entry| entry.omp_profile == paths.profile && entry.active))
    }
}

fn indent_yaml(text: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    text.lines()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(windows)]
trait CommandNoWindow {
    fn creation_flags_no_window(&mut self) -> &mut Self;
}

#[cfg(windows)]
impl CommandNoWindow for Command {
    fn creation_flags_no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(0x0800_0000)
    }
}

#[cfg(not(windows))]
trait CommandNoWindow {
    fn creation_flags_no_window(&mut self) -> &mut Self;
}

#[cfg(not(windows))]
impl CommandNoWindow for Command {
    fn creation_flags_no_window(&mut self) -> &mut Self {
        self
    }
}

impl AgentAdapter for OmpAdapter {
    fn id(&self) -> AgentId {
        AgentId::Omp
    }

    fn detect(&self) -> Result<AgentDetection, BridgeError> {
        let paths = self.paths()?;
        let executable = Self::find_omp();
        let version = executable
            .as_deref()
            .and_then(|path| Self::version_of(path).ok());
        let mut note = None;
        if let Some(version) = version.as_deref() {
            if !version_at_least(version, MIN_OMP_VERSION) {
                note = Some(format!(
                    "Oh My Pi {version} is older than the tested {MIN_OMP_VERSION_LABEL}. Update OMP before activating."
                ));
            }
        }
        if let Some(executable) = executable.as_deref() {
            if !paths.config_dir.exists() {
                note = Some(match note {
                    Some(existing) => format!(
                        "{existing} The config directory will be created at {}.",
                        paths.config_dir.display()
                    ),
                    None => format!(
                        "The config directory will be created at {}.",
                        paths.config_dir.display()
                    ),
                });
            }
            let _ = executable;
        }
        Ok(AgentDetection {
            agent: AgentId::Omp,
            display_name: AgentId::Omp.display_name().to_string(),
            installed: executable.is_some(),
            executable: executable.map(|path| path.display().to_string()),
            version,
            config_path: Some(paths.models_path.display().to_string()),
            config_dir: Some(paths.config_dir.display().to_string()),
            config_dir_from_env: paths.from_env,
            note,
            install_guide_url: Some("https://github.com/can1357/oh-my-pi".to_string()),
        })
    }

    fn locate_config(&self) -> Result<PathBuf, BridgeError> {
        Ok(self.paths()?.models_path)
    }

    fn read_config(&self) -> Result<ConfigSnapshot, BridgeError> {
        let paths = self.paths()?;
        let (models, config) = self.read_pair(&paths)?;
        if models.is_none() && config.is_none() {
            return Ok(ConfigSnapshot {
                path: paths.models_path,
                raw: None,
                sha256: None,
                state: ConfigParseState::Missing,
                parse_error: None,
            });
        }
        let models_state = self.parse_models(models.as_deref());
        let config_state = self.parse_config(config.as_deref());
        let (state, parse_error) = match (models_state, config_state) {
            (Ok(_), Ok(_)) => (ConfigParseState::Valid, None),
            (Err(err), _) | (_, Err(err)) => (ConfigParseState::Invalid, Some(err.message)),
        };
        let raw = models.clone().or(config);
        // `apply_profile` guards `models.yml` specifically, so the hash must
        // describe that file even when only `config.yml` exists.
        let sha256 = Some(sha256_hex(models.as_deref().unwrap_or_default().as_bytes()));
        Ok(ConfigSnapshot {
            path: paths.models_path,
            raw,
            sha256,
            state,
            parse_error,
        })
    }

    fn validate_config(&self, _snapshot: &ConfigSnapshot) -> Result<(), BridgeError> {
        let paths = self.paths()?;
        let (models, config) = self.read_pair(&paths)?;
        self.parse_models(models.as_deref())?;
        self.parse_config(config.as_deref())?;
        Ok(())
    }

    fn get_active_profile(&self) -> Result<ActiveConfiguration, BridgeError> {
        let paths = self.paths()?;
        let (models_raw, config_raw) = self.read_pair(&paths)?;
        let models = self.parse_models(models_raw.as_deref())?;
        let config = self.parse_config(config_raw.as_deref())?;
        let Some(default) = config.model_roles.get("default") else {
            return Ok(ActiveConfiguration::default_config("Oh My Pi Default"));
        };
        let (provider_id, model) = default
            .split_once('/')
            .map(|(provider, model)| (provider.to_string(), model.to_string()))
            .unwrap_or_else(|| (default.clone(), String::new()));
        if let Some(entry) = self.owner_entry_for_active(&paths)? {
            if entry.provider_id == provider_id
                && models.providers.contains_key(&provider_id)
                && entry.selector == *default
            {
                return Ok(ActiveConfiguration {
                    kind: crate::adapters::ActiveConfigurationKind::Managed,
                    profile_id: Some(entry.profile_id),
                    profile_name: Some(entry.profile_name),
                    label: format!("{model} via Oh My Pi"),
                    provider_id: Some(provider_id),
                    model: Some(model),
                });
            }
        }
        Ok(ActiveConfiguration::external(
            Some(provider_id),
            (!model.is_empty()).then_some(model),
        ))
    }

    fn check_profile_compatibility(&self, profile: &Profile) -> CompatibilityStatus {
        if !profile.targets_agent(AgentId::Omp) {
            return CompatibilityStatus::Untested;
        }
        profile.compatibility_for(AgentId::Omp)
    }

    fn check_managed_policy(&self) -> Result<(), BridgeError> {
        Ok(())
    }

    fn apply_profile(
        &self,
        profile: &Profile,
        options: &ApplyOptions<'_>,
    ) -> Result<ApplyOutcome, BridgeError> {
        if !profile.targets_agent(AgentId::Omp) {
            return Err(BridgeError::validation(
                "This profile is not enabled for Oh My Pi.",
            ));
        }
        profile.validate().map_err(BridgeError::validation)?;
        let _lock = self.lock()?;
        let detection = self.detect()?;
        if !detection.installed {
            return Err(BridgeError::new(
                ErrorKind::AgentUnavailable,
                "Oh My Pi is not installed.",
            )
            .with_hint("Install Oh My Pi, then press Refresh."));
        }
        if let Some(version) = detection.version.as_deref() {
            if !version_at_least(version, MIN_OMP_VERSION) {
                return Err(BridgeError::new(
                    ErrorKind::Incompatible,
                    "This Oh My Pi version is older than the version Eclink supports.",
                )
                .with_hint(format!(
                    "Update Oh My Pi to {MIN_OMP_VERSION_LABEL} or newer."
                )));
            }
        }
        let helper = options.helper_path.as_deref().ok_or_else(|| {
            BridgeError::secret(
                "Eclink's credential helper is missing, so it cannot give Oh My Pi a credential safely.",
            )
            .with_hint("Reinstall Eclink so eclink-helper.exe is available, then try again.")
        })?;
        if !helper.is_file() {
            return Err(BridgeError::secret(
                "Eclink's credential helper is missing, so it cannot give Oh My Pi a credential safely.",
            )
            .with_hint("Reinstall Eclink so eclink-helper.exe is available, then try again."));
        }

        let paths = self.paths()?;
        let (models_raw, config_raw) = self.read_pair(&paths)?;
        if let Some(expected) = options.expected_sha256.as_deref() {
            let actual = match models_raw.as_deref() {
                Some(text) => sha256_hex(text.as_bytes()),
                None => sha256_hex(b""),
            };
            if expected != actual {
                return Err(BridgeError::config_conflict(
                    "models.yml changed after Eclink read it, so Eclink did not overwrite the newer file.",
                )
                .with_hint("Review the file and activate again; Eclink will re-read it first."));
            }
        }
        let models = self.parse_models(models_raw.as_deref())?;
        self.parse_config(config_raw.as_deref())?;
        let provider_id = omp_managed_provider_id(profile);
        let existing_entry = self.ledger_entry_for(profile, &provider_id)?;
        if models.providers.contains_key(&provider_id)
            && existing_entry
                .as_ref()
                .is_none_or(|entry| entry.omp_profile != paths.profile)
        {
            return Err(BridgeError::config_conflict(format!(
                "The OMP provider `{provider_id}` already exists and is not recorded as Eclink-owned."
            ))
            .with_hint(
                "Eclink will not overwrite it. Rename or duplicate the Eclink profile to create a \
                 new provider ID, or remove the conflicting entry from models.yml yourself.",
            ));
        }

        let template = self.provider_template(profile, helper)?;
        let selector = self.selector_for(profile, &provider_id);
        let roles = self.roles_for(profile, &provider_id);
        for (role, value) in &roles {
            validate_omp_selector(value).map_err(|reason| {
                BridgeError::validation(format!("The {role} model selector is invalid: {reason}"))
            })?;
        }

        let mut new_models = self.edit_models(models_raw.as_deref(), &provider_id, &template)?;
        // Activating profile B in the same OMP profile must not leave profile
        // A's provider behind. Retire every other Eclink-owned provider and
        // restore the roles that entry overwrote before applying the new roles.
        let previous_entries: Vec<OmpLedgerEntry> = self
            .read_ledger()?
            .entries
            .into_iter()
            .filter(|entry| {
                entry.omp_profile == paths.profile
                    && entry.active
                    && entry.provider_id != provider_id
            })
            .collect();
        let mut config_text = config_raw.clone();
        for previous in &previous_entries {
            new_models = self.remove_provider(&new_models, &previous.provider_id)?;
            if let Some(text) = config_text.as_deref() {
                config_text = Some(self.restore_roles(Some(text), previous)?);
            }
        }
        // The ledger records the values as they are *after* retiring the previous
        // Eclink profile and *before* the new roles, so Restore Default returns to
        // the user's own roles, not to a removed provider's selectors.
        let config_before_new_roles = self.parse_config(config_text.as_deref())?;
        let new_config = if config_text.is_none() && roles.is_empty() {
            // No roles to assign and no existing config: leave the file absent
            // instead of creating an empty modelRoles section.
            None
        } else {
            Some(self.edit_roles(config_text.as_deref(), &roles)?)
        };
        // Parse the result before writing so a malformed edit never reaches disk.
        self.parse_models(Some(&new_models))?;
        if let Some(text) = new_config.as_deref() {
            self.parse_config(Some(text))?;
        }

        let snapshot = self.snapshot_pair(
            &paths,
            MutationReason::ActivateProfile {
                profile_id: profile.id.clone(),
                profile_name: profile.name.clone(),
            },
            Some(&profile.id),
        )?;
        let created_original = self.backups.ensure_original_bundle(
            &original_key(&paths.profile),
            &[
                (paths.models_path.clone(), models_raw.clone()),
                (paths.config_path.clone(), config_raw.clone()),
            ],
            &[format!("providers:{provider_id}")],
        )?;

        let changes = vec![
            (paths.models_path.clone(), Some(new_models)),
            (paths.config_path.clone(), new_config),
        ];
        self.restore_files(&paths, &changes)?;

        if let Err(err) = self.verify_registry(&provider_id, profile.wire_model()) {
            // Put both files back exactly as they were before the attempt.
            let _ = self.restore_files(
                &paths,
                &[
                    (paths.models_path.clone(), models_raw),
                    (paths.config_path.clone(), config_raw),
                ],
            );
            return Err(err);
        }

        self.update_ledger_after_apply(
            profile,
            &paths,
            &provider_id,
            &selector,
            &roles,
            &config_before_new_roles,
            existing_entry.as_ref(),
        )?;
        self.outcome(&paths, snapshot.meta.id, created_original)
    }

    fn restore_default(&self, _options: &ApplyOptions<'_>) -> Result<ApplyOutcome, BridgeError> {
        let _lock = self.lock()?;
        let paths = self.paths()?;
        let (models_raw, config_raw) = self.read_pair(&paths)?;
        let Some(models_raw_ref) = models_raw.as_deref() else {
            return Err(BridgeError::validation(
                "Eclink has no Oh My Pi configuration to restore.",
            ));
        };
        let entry = self.owner_entry_for_active(&paths)?.ok_or_else(|| {
            BridgeError::validation(
                "Eclink has no active Oh My Pi profile it can restore for this OMP profile.",
            )
        })?;
        let models = self.parse_models(Some(models_raw_ref))?;
        if !models.providers.contains_key(&entry.provider_id) {
            return Err(BridgeError::config_conflict(
                "The Eclink provider is no longer present in models.yml, so nothing was changed.",
            ));
        }
        let snapshot = self.snapshot_pair(&paths, MutationReason::RestoreDefault, None)?;
        let new_models = self.remove_provider(models_raw_ref, &entry.provider_id)?;
        let new_config = match config_raw.as_deref() {
            Some(text) => Some(self.restore_roles(Some(text), &entry)?),
            None => None,
        };
        self.restore_files(
            &paths,
            &[
                (paths.models_path.clone(), Some(new_models)),
                (paths.config_path.clone(), new_config),
            ],
        )?;
        let mut ledger = self.read_ledger()?;
        for other in &mut ledger.entries {
            if other.omp_profile == paths.profile {
                other.active = false;
            }
        }
        self.write_ledger(&ledger)?;
        self.outcome(&paths, snapshot.meta.id, false)
    }

    fn reset_to_stock_defaults(
        &self,
        _options: &ApplyOptions<'_>,
    ) -> Result<ApplyOutcome, BridgeError> {
        Err(BridgeError::new(
            ErrorKind::Incompatible,
            "A full stock reset is not available for Oh My Pi.",
        )
        .with_hint(
            "Use Restore default to remove only Eclink's provider and roles, or Restore original \
             to return to the files that existed before Eclink first changed them.",
        ))
    }

    fn restore_original(&self, _options: &ApplyOptions<'_>) -> Result<ApplyOutcome, BridgeError> {
        let _lock = self.lock()?;
        let paths = self.paths()?;
        let key = original_key(&paths.profile);
        let Some((_meta, files)) = self.backups.read_original_bundle(&key)? else {
            return Err(BridgeError::validation(
                "There is no original Oh My Pi configuration recorded for this OMP profile.",
            )
            .with_hint("Eclink creates the original on the first activation."));
        };
        let mut expected = std::collections::BTreeMap::new();
        for path in paths.bundle_paths() {
            expected.insert(path.display().to_string(), path);
        }
        let snapshot = self.snapshot_pair(&paths, MutationReason::RestoreOriginal, None)?;
        let mut changes = Vec::new();
        for (file, content) in files {
            let target = expected.get(&file.source_path).ok_or_else(|| {
                BridgeError::config_conflict(
                    "That original backup belongs to a different Oh My Pi profile.",
                )
                .with_hint("Switch to the OMP profile that was active when Eclink recorded it.")
            })?;
            changes.push((target.clone(), content));
        }
        self.restore_files(&paths, &changes)?;
        let mut ledger = self.read_ledger()?;
        ledger
            .entries
            .retain(|entry| entry.omp_profile != paths.profile);
        self.write_ledger(&ledger)?;
        self.outcome(&paths, snapshot.meta.id, false)
    }

    fn restore_snapshot(
        &self,
        snapshot: &Snapshot,
        _options: &ApplyOptions<'_>,
    ) -> Result<ApplyOutcome, BridgeError> {
        let _lock = self.lock()?;
        let paths = self.paths()?;
        let bundle = SnapshotBundle::parse(&snapshot.content).ok_or_else(|| {
            BridgeError::config_invalid("This snapshot is not a valid Oh My Pi snapshot.")
        })?;
        let mut expected = std::collections::BTreeMap::new();
        for path in paths.bundle_paths() {
            expected.insert(path.display().to_string(), path);
        }
        let mut changes = Vec::new();
        for file in bundle.files {
            let target = expected.get(&file.path).ok_or_else(|| {
                BridgeError::config_conflict(
                    "That snapshot belongs to a different Oh My Pi profile.",
                )
                .with_hint("Switch to the OMP profile that was active when the snapshot was taken.")
            })?;
            changes.push((target.clone(), file.content));
        }
        let current = self.snapshot_pair(
            &paths,
            MutationReason::RestoreSnapshot {
                snapshot_id: snapshot.meta.id.clone(),
            },
            None,
        )?;
        self.restore_files(&paths, &changes)?;
        let mut ledger = self.read_ledger()?;
        for entry in &mut ledger.entries {
            if entry.omp_profile == paths.profile {
                entry.active = false;
            }
        }
        self.write_ledger(&ledger)?;
        self.outcome(&paths, current.meta.id, false)
    }

    fn launch_argv(&self, project_dir: &Path) -> Result<LaunchPlan, BridgeError> {
        let detection = self.detect()?;
        let executable = detection.executable.ok_or_else(|| {
            BridgeError::new(ErrorKind::AgentUnavailable, "Oh My Pi is not installed.")
                .with_hint("Install Oh My Pi, then press Refresh.")
        })?;
        let paths = self.paths()?;
        let mut extra = Vec::new();
        if paths.profile != DEFAULT_PROFILE {
            extra.push("--profile".to_string());
            extra.push(paths.profile.clone());
        }
        if let Ok(config) = self.get_active_profile() {
            if config.kind == crate::adapters::ActiveConfigurationKind::Managed {
                if let (Some(provider), Some(model)) = (config.provider_id, config.model) {
                    extra.push("--model".to_string());
                    extra.push(format!("{provider}/{model}"));
                }
            }
        }
        crate::process::build_launch_plan_with_args(
            AgentId::Omp,
            Path::new(&executable),
            project_dir,
            &extra,
        )
    }

    fn launch_env(&self, _secret: Option<&str>) -> Result<Vec<(String, String)>, BridgeError> {
        // The credential is resolved by OMP from DPAPI through the helper
        // command in models.yml. Nothing needs to be placed in the child's
        // environment, so there is no environment-only fallback to leak.
        //
        // Pin the profile so a shell-level OMP_PROFILE cannot silently select a
        // different config directory than the one Eclink just activated.
        let paths = self.paths()?;
        Ok(vec![("OMP_PROFILE".to_string(), paths.profile)])
    }

    fn has_original_backup(&self) -> bool {
        self.paths()
            .map(|paths| self.backups.has_original(&original_key(&paths.profile)))
            .unwrap_or(false)
    }

    fn diagnostics(&self) -> Result<AgentDiagnostics, BridgeError> {
        let detection = self.detect()?;
        let snapshot = self.read_config()?;
        let active = self.get_active_profile()?;
        Ok(AgentDiagnostics {
            agent: AgentId::Omp,
            display_name: AgentId::Omp.display_name().to_string(),
            installed: detection.installed,
            version: detection.version,
            config_path: Some(snapshot.path.display().to_string()),
            config_state: snapshot.state,
            active_profile_id: active.profile_id,
            backups_present: self.backups.root().join(AgentId::Omp.as_str()).exists(),
            original_backup_present: self
                .paths()
                .map(|paths| self.backups.has_original(&original_key(&paths.profile)))
                .unwrap_or(false),
            snapshot_count: self.backups.snapshot_count(AgentId::Omp.as_str()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_snapshot_bundle_round_trips_and_hides_the_secret() {
        let root = std::env::temp_dir().join(format!(
            "eclink-omp-sealed-snapshot-{}",
            uuid::Uuid::new_v4()
        ));
        let store = crate::backup::SnapshotStore::new(&root);
        let plain = "providers:\n  mine:\n    apiKey: sk-never-store-this\n";
        let snapshot = store
            .create_bundle_sealed(
                "omp",
                &[SnapshotBundleFile {
                    path: "C:\\config\\models.yml".into(),
                    content: Some(plain.into()),
                }],
                "test",
                None,
                &[],
            )
            .unwrap();
        assert!(snapshot.meta.sealed);
        assert!(!snapshot.content.contains("sk-never-store-this"));
        let parsed = SnapshotBundle::parse(&snapshot.content).unwrap();
        assert_eq!(parsed.files[0].content.as_deref(), Some(plain));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn backups_seal_literal_credentials() {
        let plain = "providers:\n  mine:\n    apiKey: sk-must-not-appear\n";
        let sealed = crate::backup::seal_backup_text(plain).unwrap();
        assert!(!sealed.contains("sk-must-not-appear"), "{sealed}");
        assert!(sealed.starts_with(crate::backup::SEALED_PREFIX));
        assert_eq!(crate::backup::open_backup_text(&sealed).unwrap(), plain);
    }

    #[test]
    fn profile_names_match_omp_rules() {
        assert_eq!(normalize_profile_name("work").unwrap(), "work");
        assert_eq!(normalize_profile_name("Default").unwrap(), "default");
        assert!(normalize_profile_name("../escape").is_err());
        assert!(normalize_profile_name("Work").is_err());
        assert!(normalize_profile_name("con").is_err());
        assert!(normalize_profile_name("-bad").is_err());
        assert!(normalize_profile_name("").is_err());
    }

    #[test]
    fn provider_edit_preserves_comments_and_other_providers() {
        let adapter = OmpAdapter::new(
            BackupManager::new(std::env::temp_dir().join("eclink-omp-test")),
            std::env::temp_dir().join("eclink-omp-test/ledger.json"),
            OmpSelection::new("default"),
        );
        let original = "# my providers\nproviders:\n  mine: # keep me\n    baseUrl: https://mine.example/v1\n    apiKey: MINE_KEY\n    models:\n      - id: mine-model\n";
        let template = ProviderTemplate {
            base_url: "https://gateway.example/v1".into(),
            api: "openai-completions".into(),
            api_key: "!\"C:\\Program Files\\Eclink\\eclink-helper.exe\" secret resolve abc".into(),
            auth_header: true,
            models: vec![ModelTemplate {
                id: "model-1".into(),
                name: "Model 1".into(),
                context_window: None,
                max_tokens: None,
            }],
        };
        let out = adapter
            .edit_models(Some(original), "eclink-omp-test-abc", &template)
            .unwrap();
        assert!(out.contains("# my providers"), "{out}");
        assert!(out.contains("# keep me"), "{out}");
        assert!(out.contains("eclink-omp-test-abc"), "{out}");
        assert!(out.contains("mine-model"), "{out}");
        let parsed: ModelsDoc = serde_yaml::from_str(&out).unwrap();
        assert_eq!(parsed.providers.len(), 2);
        assert!(parsed.providers["eclink-omp-test-abc"]
            .api_key
            .as_deref()
            .unwrap()
            .starts_with('!'));
    }

    #[test]
    fn role_edit_preserves_unrelated_roles() {
        let adapter = OmpAdapter::new(
            BackupManager::new(std::env::temp_dir().join("eclink-omp-test")),
            std::env::temp_dir().join("eclink-omp-test/ledger.json"),
            OmpSelection::new("default"),
        );
        let out = adapter
            .edit_roles(
                Some("modelRoles:\n  default: old/model\n  vision: vision/model\n"),
                &[
                    ("default".into(), "new/model".into()),
                    ("smol".into(), "new/smol".into()),
                ],
            )
            .unwrap();
        assert!(out.contains("vision: vision/model"), "{out}");
        assert!(out.contains("default: new/model"), "{out}");
        let parsed: ConfigDoc = serde_yaml::from_str(&out).unwrap();
        assert_eq!(parsed.model_roles.len(), 3);
    }

    #[test]
    fn missing_models_file_is_created_from_empty_base() {
        let adapter = OmpAdapter::new(
            BackupManager::new(std::env::temp_dir().join("eclink-omp-test")),
            std::env::temp_dir().join("eclink-omp-test/ledger.json"),
            OmpSelection::new("default"),
        );
        let template = ProviderTemplate {
            base_url: "https://gateway.example/v1".into(),
            api: "openai-completions".into(),
            api_key: "!\"C:\\Eclink\\helper.exe\" secret resolve abc".into(),
            auth_header: true,
            models: vec![ModelTemplate {
                id: "model-1".into(),
                name: "Model 1".into(),
                context_window: None,
                max_tokens: None,
            }],
        };
        let out = adapter
            .edit_models(None, "eclink-omp-new-abc", &template)
            .unwrap();
        let parsed: ModelsDoc = serde_yaml::from_str(&out).unwrap();
        assert_eq!(parsed.providers.len(), 1);
        assert!(parsed.providers.contains_key("eclink-omp-new-abc"), "{out}");
    }

    #[test]
    fn empty_key_placeholders_are_replaced_without_losing_comments() {
        let adapter = OmpAdapter::new(
            BackupManager::new(std::env::temp_dir().join("eclink-omp-test")),
            std::env::temp_dir().join("eclink-omp-test/ledger.json"),
            OmpSelection::new("default"),
        );
        let template = ProviderTemplate {
            base_url: "https://gateway.example/v1".into(),
            api: "openai-completions".into(),
            api_key: "!cmd".into(),
            auth_header: true,
            models: vec![ModelTemplate {
                id: "model-1".into(),
                name: "Model 1".into(),
                context_window: None,
                max_tokens: None,
            }],
        };
        let out = adapter
            .edit_models(
                Some("# keep\nproviders:\n"),
                "eclink-omp-empty-abc",
                &template,
            )
            .unwrap();
        assert!(out.contains("# keep"), "{out}");
        let parsed: ModelsDoc = serde_yaml::from_str(&out).unwrap();
        assert_eq!(parsed.providers.len(), 1);
    }

    #[test]
    fn missing_config_file_gets_model_roles() {
        let adapter = OmpAdapter::new(
            BackupManager::new(std::env::temp_dir().join("eclink-omp-test")),
            std::env::temp_dir().join("eclink-omp-test/ledger.json"),
            OmpSelection::new("default"),
        );
        let out = adapter
            .edit_roles(None, &[("default".into(), "prov/model".into())])
            .unwrap();
        let parsed: ConfigDoc = serde_yaml::from_str(&out).unwrap();
        assert_eq!(
            parsed.model_roles.get("default").map(String::as_str),
            Some("prov/model")
        );
    }

    #[test]
    fn provider_model_ids_with_colons_are_not_rejected() {
        assert!(validate_omp_selector("router/kr/claude-sonnet-5.5:high").is_ok());
        assert!(validate_omp_selector("router/model:thinking").is_ok());
        assert!(validate_omp_selector("provider").is_err());
        assert!(validate_omp_selector("/model").is_err());
    }
}
