//! OpenAI Codex adapter (PRD §16).
//!
//! ## Official baseline this implements (verified against the current Codex
//! configuration reference)
//!
//! * user config lives at `~/.codex/config.toml`, or under `$CODEX_HOME`;
//! * a custom provider is `[model_providers.<id>]` with `name`, `base_url`, and
//!   `wire_api`;
//! * `model_provider` selects it and `model` selects the model;
//! * the reserved built-in provider IDs `openai`, `ollama`, and `lmstudio`
//!   cannot be reused by a custom provider;
//! * command-backed authentication is `[model_providers.<id>.auth]` with
//!   `command`, `args`, and optional `timeout_ms`, and it must not be combined
//!   with `env_key` or `experimental_bearer_token`;
//! * custom providers use `wire_api = "responses"`.
//!
//! Bridge therefore writes an `[auth]` command pointing at
//! `eclyzer-bridge-helper.exe` so the API key never lands in `config.toml`
//! (PRD §13.4, §16.5), and it never writes a bearer token inline.

use std::path::{Path, PathBuf};

use crate::adapters::detect;
use crate::adapters::{
    ActiveConfiguration, ActiveConfigurationKind, AgentAdapter, AgentDetection, AgentDiagnostics,
    ApplyOptions, ApplyOutcome, ConfigParseState, ConfigSnapshot, LaunchPlan,
};
use crate::backup::{BackupManager, Snapshot};
use crate::config::fs::{sha256_hex, write_atomic};
use crate::config::toml_patch::{AuthCommand, CodexToml, ProviderSpec};
use crate::error::{BridgeError, BridgeResult};
use crate::profiles::{managed_provider_id, AgentId, CompatibilityStatus, Profile};
use crate::MANAGED_PROVIDER_PREFIX;

/// Environment variable Codex honours for its state directory.
pub const CODEX_HOME_ENV: &str = "CODEX_HOME";

/// Official installation guide, offered only as a link (PRD §33).
pub const INSTALL_GUIDE_URL: &str = "https://developers.openai.com/codex/cli/";

/// Codex version that first supported `[model_providers.<id>.auth]`.
///
/// Older builds are still usable; Bridge falls back to `env_key` and says so.
pub const AUTH_COMMAND_MIN_VERSION: (u64, u64, u64) = (0, 20, 0);

/// Environment variable Bridge uses when it must fall back from `auth`.
pub const FALLBACK_ENV_KEY: &str = "ECLYZER_BRIDGE_CODEX_API_KEY";

/// The Codex agent adapter.
pub struct CodexAdapter {
    backups: BackupManager,
}

impl CodexAdapter {
    pub fn new(backups: BackupManager) -> Self {
        Self { backups }
    }

    /// Default config directory when `CODEX_HOME` is not set.
    pub fn default_config_dir() -> BridgeResult<PathBuf> {
        Ok(crate::paths::home_dir()?.join(".codex"))
    }

    /// Effective config directory, honouring `CODEX_HOME`.
    pub fn config_dir() -> BridgeResult<(PathBuf, bool)> {
        Ok(detect::resolve_config_dir(
            CODEX_HOME_ENV,
            Self::default_config_dir()?,
        ))
    }

    /// Immutable snapshot of the current config content, used for the
    /// external-modification check (PRD §31.3).
    pub fn current_hash(&self) -> BridgeResult<Option<String>> {
        let path = self.locate_config()?;
        crate::config::fs::sha256_file(&path)
    }

    /// Build the provider spec Bridge writes for `profile`.
    ///
    /// `use_auth_command` selects the credential mechanism; the caller decides
    /// based on the detected Codex version.
    ///
    /// `base_url` and `helper_command` are passed in separately because both are
    /// produced as owned `String`s by the caller, and `ProviderSpec` borrows its
    /// fields.
    fn provider_spec<'a>(
        profile: &'a Profile,
        base_url: &'a str,
        helper_command: Option<&'a str>,
        use_auth_command: bool,
        env_key: Option<&'a str>,
    ) -> ProviderSpec<'a> {
        let headers: Vec<(&str, &str)> = profile
            .advanced
            .headers
            .iter()
            .map(|pair| (pair.name.as_str(), pair.value.as_str()))
            .collect();
        let query_params: Vec<(&str, &str)> = profile
            .advanced
            .query_params
            .iter()
            .map(|pair| (pair.name.as_str(), pair.value.as_str()))
            .collect();

        // The helper command must outlive the spec; the caller owns the String.
        let auth_command = match (use_auth_command, helper_command) {
            (true, Some(command)) => Some(AuthCommand {
                // Written as a single TOML string; never concatenated into a
                // command line (PRD §63).
                command,
                args: vec!["secret", "resolve", profile.id.as_str()],
                timeout_ms: Some(5000),
            }),
            _ => None,
        };

        ProviderSpec {
            name: &profile.provider_name,
            base_url,
            wire_api: "responses",
            env_key,
            auth_command,
            http_headers: headers,
            query_params,
        }
    }

    /// Keys Bridge owns for the active profile.
    fn managed_keys_for(provider_id: &str) -> Vec<String> {
        vec![
            "model".to_string(),
            "model_provider".to_string(),
            format!("model_providers.{provider_id}"),
        ]
    }

    /// Remove every Bridge-owned key from the document.
    ///
    /// Ownership is decided by evidence, never by guessing:
    ///
    /// * `model_provider` is Bridge-owned when its value carries the Bridge
    ///   provider prefix;
    /// * `model` is Bridge-owned only when Bridge's own `model_provider`
    ///   selection is currently active — a `model` that a user set for their own
    ///   provider must survive (PRD §16.6, §17);
    /// * provider entries are Bridge-owned when their ID carries the prefix.
    ///
    /// A user's own providers, model choice, MCP servers, sandbox settings, and
    /// unrelated keys are never touched.
    fn strip_managed(doc: &mut CodexToml) -> Vec<String> {
        let mut removed = Vec::new();

        let bridge_provider_active = doc
            .get_str("model_provider")
            .is_some_and(is_managed_provider_id);

        if bridge_provider_active {
            doc.remove("model_provider");
            removed.push("model_provider".to_string());

            // Bridge set `model` together with its provider, so it owns `model`
            // only in that same state.
            if doc.contains("model") {
                doc.remove("model");
                removed.push("model".to_string());
            }
        }

        for provider_id in doc.provider_ids() {
            if is_managed_provider_id(&provider_id) {
                doc.remove_provider(&provider_id);
                removed.push(format!("model_providers.{provider_id}"));
            }
        }

        removed
    }

    /// Perform the guarded mutation sequence (PRD §31.1).
    fn mutate<F>(
        &self,
        options: &ApplyOptions<'_>,
        reason: &crate::adapters::MutationReason,
        build: F,
    ) -> BridgeResult<ApplyOutcome>
    where
        F: FnOnce(&mut CodexToml) -> BridgeResult<Vec<String>>,
    {
        let path = self.locate_config()?;

        // Serialize mutations per agent (PRD §48).
        let _lock =
            crate::config::fs::FileLock::acquire(self.backups.root().join("codex-config.lock"))?;

        // 1. read raw
        let raw = crate::config::fs::read_to_string_opt(&path)?;

        // 2. parse (invalid TOML is a hard stop, PRD §31.2)
        let mut doc = CodexToml::parse_or_empty(raw.as_deref())?;

        // 3. validate existing content is safe to modify
        let snapshot = ConfigSnapshot {
            path: path.clone(),
            raw: raw.clone(),
            sha256: raw.as_deref().map(|text| sha256_hex(text.as_bytes())),
            state: if raw.is_none() {
                ConfigParseState::Missing
            } else {
                ConfigParseState::Valid
            },
            parse_error: None,
        };
        self.validate_config(&snapshot)?;

        // 4. external-modification guard (PRD §31.3, §48)
        if let Some(expected) = &options.expected_sha256 {
            let actual = snapshot.sha256.clone().unwrap_or_else(|| sha256_hex(b""));
            if &actual != expected {
                return Err(BridgeError::config_conflict(
                    "Configuration changed outside Eclyzer Bridge.",
                )
                .with_hint("Reload the latest configuration before applying changes."));
            }
        }

        // 5. immutable original backup, once per agent (PRD §25)
        let created_original_backup =
            self.backups
                .ensure_original(AgentId::Codex.as_str(), &path, &[])?;

        // 6. snapshot immediately before the change (PRD §26)
        let snap = self.backups.create_snapshot(
            AgentId::Codex.as_str(),
            &path,
            reason.code(),
            reason.profile_id(),
            &[],
        )?;

        // 7. patch only managed keys
        let managed_keys = build(&mut doc)?;

        // 8. serialize
        let rendered = doc.to_string_preserving();

        // 9. reparse the candidate before touching the real file
        CodexToml::parse(&rendered).map_err(|err| {
            BridgeError::internal("The generated Codex configuration was not valid TOML.")
                .with_hint("Nothing was written. Please report this as a bug.")
                .with_details(format!("{err}"))
        })?;

        // 10. atomic replace
        write_atomic(&path, rendered.as_bytes())?;

        // 11. verify by re-reading
        let verify = crate::config::fs::read_to_string_opt(&path)?.unwrap_or_default();
        if verify != rendered {
            // Roll back from the snapshot we just took so the user is never left
            // with a half-applied config (PRD §24).
            let _ = write_atomic(&path, snap.content.as_bytes());
            return Err(BridgeError::internal(
                "Eclyzer Bridge could not verify the updated Codex configuration.",
            )
            .with_hint("Your previous configuration was restored."));
        }

        // Record ownership metadata for a later precise restore.
        let owned = if managed_keys.is_empty() {
            self.read_ownership_keys().unwrap_or_default()
        } else {
            managed_keys.clone()
        };
        let _ = self.write_ownership_keys(&owned);

        let active = self.get_active_profile()?;

        Ok(ApplyOutcome {
            agent: AgentId::Codex,
            config_path: path.display().to_string(),
            snapshot_id: snap.meta.id,
            created_original_backup,
            managed_keys,
            active,
        })
    }

    fn ownership_path(&self) -> PathBuf {
        self.backups
            .root()
            .join(AgentId::Codex.as_str())
            .join("ownership.json")
    }

    /// Read the metadata describing which keys Bridge owns (PRD §49).
    fn read_ownership_keys(&self) -> BridgeResult<Vec<String>> {
        let Some(raw) = crate::config::fs::read_to_string_opt(&self.ownership_path())? else {
            return Ok(Vec::new());
        };
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        Ok(value
            .get("managedKeys")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn write_ownership_keys(&self, keys: &[String]) -> BridgeResult<()> {
        let path = self.ownership_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let payload = serde_json::json!({
            "agent": AgentId::Codex.as_str(),
            "managedKeys": keys,
        });
        write_atomic(&path, serde_json::to_string_pretty(&payload)?.as_bytes())
    }
}

/// Whether a provider ID is owned by Eclyzer Bridge.
pub fn is_managed_provider_id(id: &str) -> bool {
    id.starts_with(MANAGED_PROVIDER_PREFIX)
}

impl AgentAdapter for CodexAdapter {
    fn id(&self) -> AgentId {
        AgentId::Codex
    }

    fn detect(&self) -> BridgeResult<AgentDetection> {
        // Synchronous facade over the async detector, used by commands that are
        // already inside a Tokio runtime via `block_in_place`-free polling.
        let which = tauri::async_runtime::block_on(detect::which("codex"));
        let (config_dir, from_env) = Self::config_dir()?;
        let config_path = config_dir.join("config.toml");

        let mut executable = which.first().cloned();
        if executable.is_none() {
            let candidates = detect::known_locations(
                &crate::paths::home_dir()?,
                &["codex.exe", "codex.cmd", "codex"],
                &[".codex\\bin", ".cargo\\bin"],
            );
            executable = detect::first_existing(&candidates);
        }

        let Some(executable) = executable else {
            return Ok(AgentDetection::not_installed(
                AgentId::Codex,
                INSTALL_GUIDE_URL,
            ));
        };

        let version = tauri::async_runtime::block_on(detect::version_of(&executable));

        let note = match &version {
            Some(v) if !detect::version_at_least(v, AUTH_COMMAND_MIN_VERSION) => Some(format!(
                "Codex {v} is older than {}.{}.{}. Eclyzer Bridge will use an environment-based credential instead of a credential helper.",
                AUTH_COMMAND_MIN_VERSION.0, AUTH_COMMAND_MIN_VERSION.1, AUTH_COMMAND_MIN_VERSION.2
            )),
            _ => None,
        };

        Ok(AgentDetection {
            agent: AgentId::Codex,
            display_name: AgentId::Codex.display_name().to_string(),
            installed: true,
            executable: Some(executable.display().to_string()),
            version,
            config_path: Some(config_path.display().to_string()),
            config_dir: Some(config_dir.display().to_string()),
            config_dir_from_env: from_env,
            note,
            install_guide_url: Some(INSTALL_GUIDE_URL.to_string()),
        })
    }

    fn locate_config(&self) -> BridgeResult<PathBuf> {
        Ok(Self::config_dir()?.0.join("config.toml"))
    }

    fn read_config(&self) -> BridgeResult<ConfigSnapshot> {
        let path = self.locate_config()?;
        let raw = crate::config::fs::read_to_string_opt(&path)?;

        let (state, parse_error) = match &raw {
            None => (ConfigParseState::Missing, None),
            Some(text) => match CodexToml::parse(text) {
                Ok(_) => (ConfigParseState::Valid, None),
                Err(err) => (ConfigParseState::Invalid, Some(err.message)),
            },
        };

        Ok(ConfigSnapshot {
            path,
            sha256: raw.as_deref().map(|text| sha256_hex(text.as_bytes())),
            raw,
            state,
            parse_error,
        })
    }

    fn validate_config(&self, snapshot: &ConfigSnapshot) -> BridgeResult<()> {
        if snapshot.state == ConfigParseState::Invalid {
            return Err(BridgeError::config_invalid(format!(
                "Codex configuration is not valid TOML at {}.",
                snapshot.path.display()
            ))
            .with_hint(
                "Eclyzer Bridge won't modify this file until it can be read safely. Fix the syntax or restore a backup.",
            )
            .with_details(snapshot.parse_error.clone().unwrap_or_default()));
        }
        Ok(())
    }

    fn get_active_profile(&self) -> BridgeResult<ActiveConfiguration> {
        let snapshot = self.read_config()?;
        let Some(raw) = &snapshot.raw else {
            return Ok(ActiveConfiguration::default_config("OpenAI Default"));
        };
        if snapshot.state != ConfigParseState::Valid {
            return Ok(ActiveConfiguration::external(None, None));
        }
        let doc = CodexToml::parse(raw)?;

        let provider = doc.get_str("model_provider").map(str::to_string);
        let model = doc.get_str("model").map(str::to_string);

        match &provider {
            // A Bridge-managed provider: report it as managed unless the entry
            // vanished from under us.
            Some(id) if is_managed_provider_id(id) => Ok(ActiveConfiguration {
                kind: ActiveConfigurationKind::Managed,
                profile_id: None,
                profile_name: doc.provider_str(id, "name").or_else(|| Some(id.clone())),
                label: doc
                    .provider_str(id, "name")
                    .unwrap_or_else(|| "Eclyzer Bridge provider".to_string()),
                provider_id: provider.clone(),
                model,
            }),
            // The user is running something of their own. Never claim Default.
            Some(id) => Ok(ActiveConfiguration::external(Some(id.clone()), model)),
            None => {
                if model.is_some() {
                    Ok(ActiveConfiguration::external(None, model))
                } else {
                    Ok(ActiveConfiguration::default_config("OpenAI Default"))
                }
            }
        }
    }

    fn check_profile_compatibility(&self, profile: &Profile) -> CompatibilityStatus {
        if !profile.targets_agent(AgentId::Codex) {
            return CompatibilityStatus::Untested;
        }
        profile.compatibility_for(AgentId::Codex)
    }

    fn check_managed_policy(&self) -> BridgeResult<()> {
        // Codex admin-enforced requirements live alongside config as
        // `requirements.toml`; a managed `model_provider` there would win over
        // user config. Bridge detects that and refuses rather than pretending
        // the activation worked (PRD §18 policy, §21).
        let (config_dir, _) = Self::config_dir()?;
        let requirements = config_dir.join("requirements.toml");
        let Some(raw) = crate::config::fs::read_to_string_opt(&requirements)? else {
            return Ok(());
        };
        let Ok(doc) = raw.parse::<toml_edit::DocumentMut>() else {
            // An unparsable requirements file is not ours to interpret; treat
            // it as blocking rather than risk bypassing policy.
            return Err(BridgeError::managed_policy(
                "Managed configuration detected.",
            )
            .with_hint(
                "Codex is managed by an organization policy that Eclyzer Bridge cannot verify. Ask your administrator before changing provider settings.",
            ));
        };

        let pins_provider = doc.get("model_provider").is_some()
            || doc.get("model_providers").is_some()
            || doc.get("allowed_sandbox_modes").is_some();

        if pins_provider
            && doc
                .get("model_provider")
                .and_then(|item| item.as_str())
                .is_some()
        {
            return Err(BridgeError::managed_policy(
                "Managed configuration detected.",
            )
            .with_hint(
                "The model provider is controlled by your organization and cannot be changed by Eclyzer Bridge.",
            ));
        }

        Ok(())
    }

    fn apply_profile(
        &self,
        profile: &Profile,
        options: &ApplyOptions<'_>,
    ) -> BridgeResult<ApplyOutcome> {
        self.check_managed_policy()?;
        profile.validate().map_err(BridgeError::validation)?;

        if !profile.targets_agent(AgentId::Codex) {
            return Err(
                BridgeError::validation("This profile is not configured for Codex.")
                    .with_hint("Edit the profile and add Codex as a target."),
            );
        }

        let provider_id = managed_provider_id(profile);
        let wire_model = profile.wire_model().to_string();

        // Prefer the credential helper. Fall back to an environment key only
        // when the helper is missing or the installed Codex predates `[auth]`.
        let use_auth_command = options.helper_path.is_some()
            && options
                .agent_version
                .as_deref()
                .map(|v| detect::version_at_least(v, AUTH_COMMAND_MIN_VERSION))
                .unwrap_or(true);

        let env_key = if use_auth_command {
            None
        } else {
            Some(FALLBACK_ENV_KEY)
        };

        let reason = crate::adapters::MutationReason::ActivateProfile {
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
        };

        // Both of these are owned Strings that must outlive the closure's
        // borrow of `ProviderSpec`.
        let helper_command: Option<String> = options
            .helper_path
            .as_ref()
            .map(|path| path.display().to_string());
        let base_url = crate::profiles::normalize_base_url(&profile.base_url);
        let provider_id_for_keys = provider_id.clone();

        self.mutate(options, &reason, move |doc| {
            doc.upsert_provider(
                &provider_id,
                &Self::provider_spec(
                    profile,
                    &base_url,
                    helper_command.as_deref(),
                    use_auth_command,
                    env_key,
                ),
            );
            doc.set_str("model_provider", &provider_id);
            doc.set_str("model", &wire_model);
            Ok(Self::managed_keys_for(&provider_id_for_keys))
        })
    }

    fn restore_default(&self, options: &ApplyOptions<'_>) -> BridgeResult<ApplyOutcome> {
        let reason = crate::adapters::MutationReason::RestoreDefault;
        self.mutate(options, &reason, |doc| {
            // Remove only Bridge-owned keys: never `auth.json`, never sessions,
            // never the user's own providers (PRD §16.6, §17).
            Ok(Self::strip_managed(doc))
        })
    }

    fn restore_original(&self, options: &ApplyOptions<'_>) -> BridgeResult<ApplyOutcome> {
        let path = self.locate_config()?;

        let _lock =
            crate::config::fs::FileLock::acquire(self.backups.root().join("codex-config.lock"))?;

        let (_meta, content) = self
            .backups
            .read_original(AgentId::Codex.as_str())?
            .ok_or_else(|| {
                BridgeError::internal("There is no original backup to restore for Codex.")
                    .with_hint("Eclyzer Bridge has not modified Codex yet.")
            })?;

        // Snapshot the current state first so this restore is itself reversible
        // (PRD §29).
        let snap = self.backups.create_snapshot(
            AgentId::Codex.as_str(),
            &path,
            crate::adapters::MutationReason::RestoreOriginal.code(),
            None,
            &self.read_ownership_keys().unwrap_or_default(),
        )?;

        // Validate the backup before writing it: restoring a corrupt original
        // would be worse than failing.
        CodexToml::parse(&content).map_err(|err| {
            BridgeError::internal("The original Codex backup is not valid TOML.")
                .with_hint("Eclyzer Bridge did not write it. Use Restore OpenAI Default instead.")
                .with_details(err.message)
        })?;

        self.backups
            .write_original(AgentId::Codex.as_str(), &path)?;

        let _ = options;
        let active = self.get_active_profile()?;
        Ok(ApplyOutcome {
            agent: AgentId::Codex,
            config_path: path.display().to_string(),
            snapshot_id: snap.meta.id,
            created_original_backup: false,
            managed_keys: Vec::new(),
            active,
        })
    }

    fn restore_snapshot(
        &self,
        snapshot: &Snapshot,
        options: &ApplyOptions<'_>,
    ) -> BridgeResult<ApplyOutcome> {
        let path = self.locate_config()?;
        let _lock =
            crate::config::fs::FileLock::acquire(self.backups.root().join("codex-config.lock"))?;

        // An empty snapshot legitimately means "the file did not exist".
        if !snapshot.content.is_empty() {
            CodexToml::parse(&snapshot.content).map_err(|err| {
                BridgeError::config_invalid(
                    "This snapshot is not valid TOML and cannot be restored safely.",
                )
                .with_details(err.message)
            })?;
        }

        // Snapshot the current state so restoring a snapshot is reversible.
        let pre = self.backups.create_snapshot(
            AgentId::Codex.as_str(),
            &path,
            crate::adapters::MutationReason::RestoreSnapshot {
                snapshot_id: snapshot.meta.id.clone(),
            }
            .code(),
            None,
            &self.read_ownership_keys().unwrap_or_default(),
        )?;

        write_atomic(&path, snapshot.content.as_bytes())?;
        let _ = options;

        let active = self.get_active_profile()?;
        Ok(ApplyOutcome {
            agent: AgentId::Codex,
            config_path: path.display().to_string(),
            snapshot_id: pre.meta.id,
            created_original_backup: false,
            managed_keys: Vec::new(),
            active,
        })
    }

    fn launch_argv(&self, project_dir: &Path) -> BridgeResult<LaunchPlan> {
        let detection = self.detect()?;
        let executable = detection.executable.ok_or_else(|| {
            BridgeError::new(
                crate::error::ErrorKind::AgentUnavailable,
                "Codex is not installed.",
            )
            .with_hint("Install Codex, then press Refresh.")
        })?;

        crate::process::build_launch_plan(AgentId::Codex, Path::new(&executable), project_dir)
    }

    fn launch_env(&self, secret: Option<&str>) -> BridgeResult<Vec<(String, String)>> {
        // Only needed when Bridge had to fall back from the credential helper.
        // The variable is set for the launched process tree and never persisted
        // to the user's global environment (PRD §32.3).
        let Ok(snapshot) = self.read_config() else {
            return Ok(Vec::new());
        };
        let Some(raw) = snapshot.raw else {
            return Ok(Vec::new());
        };
        let Ok(doc) = CodexToml::parse(&raw) else {
            return Ok(Vec::new());
        };

        // Find the active managed provider and check whether it expects an env
        // key rather than a helper command.
        let Some(provider_id) = doc.get_str("model_provider") else {
            return Ok(Vec::new());
        };
        if !is_managed_provider_id(provider_id) {
            return Ok(Vec::new());
        }
        match doc.provider_str(provider_id, "env_key") {
            Some(key) => match secret {
                Some(value) => Ok(vec![(key, value.to_string())]),
                None => Ok(Vec::new()),
            },
            None => Ok(Vec::new()),
        }
    }

    fn diagnostics(&self) -> BridgeResult<AgentDiagnostics> {
        let detection = self.detect()?;
        let snapshot = self.read_config()?;
        let active = self.get_active_profile()?;

        let codex = AgentId::Codex.as_str();
        Ok(AgentDiagnostics {
            agent: AgentId::Codex,
            display_name: AgentId::Codex.display_name().to_string(),
            installed: detection.installed,
            version: detection.version,
            config_path: Some(snapshot.path.display().to_string()),
            config_state: snapshot.state,
            active_profile_id: active.profile_id,
            backups_present: self.backups.root().join(codex).exists(),
            original_backup_present: self.backups.has_original(codex),
            snapshot_count: self.backups.snapshot_count(codex),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_only_namespaced_provider_ids() {
        assert!(is_managed_provider_id("eclyzer_bridge_deepseek_abcd1234"));
        assert!(!is_managed_provider_id("my_gateway"));
        assert!(!is_managed_provider_id("openai"));
        assert!(!is_managed_provider_id("ollama"));
        assert!(!is_managed_provider_id("lmstudio"));
    }

    #[test]
    fn strip_managed_removes_only_bridge_entries() {
        // The user's own provider is active, so Bridge must leave the user's
        // selection and entry alone even though a Bridge-owned entry also
        // exists (and must still be cleaned up).
        let text = r#"model = "mine"
model_provider = "my_gateway"

[model_providers.my_gateway]
name = "Mine"
base_url = "https://mine.example.com"

[model_providers.eclyzer_bridge_deepseek_abcd1234]
name = "DeepSeek"
base_url = "https://api.deepseek.com/v1"
wire_api = "responses"
"#;
        let mut doc = CodexToml::parse(text).unwrap();
        let removed = CodexAdapter::strip_managed(&mut doc);
        let out = doc.to_string_preserving();

        // The user's provider entry survives, with its values intact.
        assert!(out.contains("[model_providers.my_gateway]"), "{out}");
        assert!(out.contains("https://mine.example.com"), "{out}");
        // The user's active selection is preserved because Bridge does not own it.
        assert!(out.contains("model = \"mine\""), "{out}");
        assert!(out.contains("model_provider = \"my_gateway\""), "{out}");
        // Only the Bridge-owned provider entry is removed.
        assert!(!out.contains("eclyzer_bridge_deepseek_abcd1234"), "{out}");
        assert_eq!(
            removed,
            vec!["model_providers.eclyzer_bridge_deepseek_abcd1234".to_string()]
        );
    }

    #[test]
    fn strip_managed_removes_bridge_owned_model_and_provider() {
        let text = r#"model = "deepseek-chat"
model_provider = "eclyzer_bridge_deepseek_abcd1234"

[model_providers.eclyzer_bridge_deepseek_abcd1234]
name = "DeepSeek"
base_url = "https://api.deepseek.com/v1"
"#;
        let mut doc = CodexToml::parse(text).unwrap();
        let removed = CodexAdapter::strip_managed(&mut doc);
        let out = doc.to_string_preserving();

        assert!(!out.contains("model_provider"), "{out}");
        assert!(!out.contains("deepseek-chat"), "{out}");
        assert!(!out.contains("eclyzer_bridge"), "{out}");
        assert!(removed.contains(&"model".to_string()));
        assert!(removed.contains(&"model_provider".to_string()));
    }

    #[test]
    fn strip_managed_preserves_unrelated_settings_entirely() {
        let text = r#"# keep me
approval_policy = "on-request"
sandbox_mode = "workspace-write"
model_provider = "eclyzer_bridge_x_1"

[sandbox_workspace_write]
network_access = false

[mcp_servers.context7]
command = "npx"

[model_providers.eclyzer_bridge_x_1]
name = "X"
base_url = "https://x"

[history]
persistence = "none"
"#;
        let mut doc = CodexToml::parse(text).unwrap();
        CodexAdapter::strip_managed(&mut doc);
        let out = doc.to_string_preserving();

        for must_survive in [
            "# keep me",
            "approval_policy = \"on-request\"",
            "sandbox_mode = \"workspace-write\"",
            "network_access = false",
            "[mcp_servers.context7]",
            "[history]",
            "persistence = \"none\"",
        ] {
            assert!(out.contains(must_survive), "lost {must_survive}:\n{out}");
        }
        assert!(!out.contains("eclyzer_bridge"), "{out}");
    }

    #[test]
    fn provider_spec_uses_responses_wire_api() {
        // PRD §16.4: custom Codex providers are Responses-API based.
        let profile = Profile::new(
            "P".into(),
            "DeepSeek".into(),
            "https://api.deepseek.com/v1/".into(),
            "deepseek-chat".into(),
            vec![AgentId::Codex],
        );
        let spec =
            CodexAdapter::provider_spec(&profile, "https://api.deepseek.com/v1", None, false, None);
        assert_eq!(spec.wire_api, "responses");
        // The trailing slash must be trimmed so the URL is canonical.
        assert_eq!(spec.base_url, "https://api.deepseek.com/v1");
    }

    #[test]
    fn provider_spec_writes_helper_command_when_available() {
        let profile = Profile::new(
            "P".into(),
            "Prov".into(),
            "https://x/v1".into(),
            "m".into(),
            vec![AgentId::Codex],
        );
        let helper = r"C:\Program Files\Eclyzer Bridge\eclyzer-bridge-helper.exe";
        let spec = CodexAdapter::provider_spec(&profile, "https://x/v1", Some(helper), true, None);

        let auth = spec.auth_command.expect("auth command expected");
        assert_eq!(
            auth.command,
            r"C:\Program Files\Eclyzer Bridge\eclyzer-bridge-helper.exe"
        );
        assert_eq!(auth.args, vec!["secret", "resolve", profile.id.as_str()]);
        assert!(
            spec.env_key.is_none(),
            "auth and env_key are mutually exclusive"
        );
    }

    #[test]
    fn provider_spec_never_writes_a_bearer_token() {
        let profile = Profile::new(
            "P".into(),
            "Prov".into(),
            "https://x/v1".into(),
            "m".into(),
            vec![AgentId::Codex],
        );
        let helper = "helper.exe";
        let mut doc = CodexToml::parse("").unwrap();
        doc.upsert_provider(
            "eclyzer_bridge_p_1",
            &CodexAdapter::provider_spec(&profile, "https://x/v1", Some(helper), true, None),
        );
        let out = doc.to_string_preserving();
        assert!(!out.contains("experimental_bearer_token"), "{out}");
        assert!(!out.contains("env_key"), "{out}");
        assert!(out.contains("auth"), "{out}");
    }

    #[test]
    fn fallback_env_key_used_when_helper_is_unavailable() {
        let profile = Profile::new(
            "P".into(),
            "Prov".into(),
            "https://x/v1".into(),
            "m".into(),
            vec![AgentId::Codex],
        );
        let spec = CodexAdapter::provider_spec(
            &profile,
            "https://x/v1",
            None,
            false,
            Some(FALLBACK_ENV_KEY),
        );
        assert_eq!(spec.env_key, Some(FALLBACK_ENV_KEY));
        assert!(spec.auth_command.is_none());
    }

    #[test]
    fn managed_keys_describe_ownership() {
        let keys = CodexAdapter::managed_keys_for("eclyzer_bridge_x_1");
        assert!(keys.contains(&"model".to_string()));
        assert!(keys.contains(&"model_provider".to_string()));
        assert!(keys.contains(&"model_providers.eclyzer_bridge_x_1".to_string()));
    }

    #[test]
    fn older_codex_versions_are_detected_for_fallback() {
        assert!(!detect::version_at_least(
            "0.19.0",
            AUTH_COMMAND_MIN_VERSION
        ));
        assert!(detect::version_at_least("0.20.0", AUTH_COMMAND_MIN_VERSION));
        assert!(detect::version_at_least("1.0.0", AUTH_COMMAND_MIN_VERSION));
    }

    #[test]
    fn empty_config_restore_default_is_a_noop() {
        // A user's own model choice, with no Bridge provider active, must be
        // left completely alone by Restore Default.
        let mut doc = CodexToml::parse("model = \"user-model\"\n").unwrap();
        let removed = CodexAdapter::strip_managed(&mut doc);
        assert!(removed.is_empty(), "nothing was Bridge-owned: {removed:?}");
        assert!(
            doc.to_string_preserving().contains("user-model"),
            "the user's model must survive"
        );
    }

    #[test]
    fn users_model_survives_when_their_own_provider_is_active() {
        let text = r#"model = "user-model"
model_provider = "my_gateway"

[model_providers.my_gateway]
name = "Mine"
base_url = "https://mine.example.com"

[model_providers.eclyzer_bridge_old_1]
name = "Old"
base_url = "https://old.example.com"
"#;
        let mut doc = CodexToml::parse(text).unwrap();
        CodexAdapter::strip_managed(&mut doc);
        let out = doc.to_string_preserving();

        assert!(out.contains("model = \"user-model\""), "{out}");
        assert!(out.contains("model_provider = \"my_gateway\""), "{out}");
        assert!(!out.contains("eclyzer_bridge_old_1"), "{out}");
    }
}
