//! Anthropic Claude Code adapter (PRD §18).
//!
//! ## Official baseline this implements (verified against the current Claude
//! Code settings reference)
//!
//! * Claude Code reads settings from, in increasing precedence: user settings at
//!   `~/.claude/settings.json`, project settings at `.claude/settings.json`,
//!   local project settings at `.claude/settings.local.json`, a `--settings`
//!   file, and finally organization-managed settings;
//! * the user settings file declares environment variables for Claude Code's own
//!   process in a top-level `env` object;
//! * a custom gateway is selected with `ANTHROPIC_BASE_URL`, and the credential
//!   is supplied either by the standard Anthropic credential variable or by an
//!   `apiKeyHelper` command;
//! * organization-managed settings may pin `allowedProviders` (for example to
//!   `["customEndpoint"]`), which overrides user settings.
//!
//! Bridge therefore writes **only** `env.ANTHROPIC_BASE_URL` plus the credential
//! variable into the user settings file. Everything else in that file —
//! `permissions`, `hooks`, `enabledPlugins`, `statusLine`, MCP servers, and any
//! key a future Claude Code release adds — is preserved by
//! [`ClaudeJson`]'s key-order-preserving map editing.
//!
//! ## Why the key is written into `env` rather than fetched by a helper
//!
//! Unlike Codex, Claude Code offers no declarative "run this command for the
//! credential" field that works for a gateway. It has `apiKeyHelper`, but that
//! is a shell-command **string** evaluated by Claude Code itself, which would
//! mean embedding an executable path into a shell line — precisely the
//! command-injection surface the PRD forbids (§63). Bridge instead writes the
//! credential into the user settings file, which Claude Code treats as private
//! configuration:
//!
//! * the file is written with owner-only permissions via `harden_permissions`;
//! * the value is never logged, never echoed in diagnostics, and never included
//!   in a diagnostics report;
//! * [`restore_default`](AgentAdapter::restore_default) removes it again.
//!
//! The UI states plainly that this credential lives in Claude Code's own
//! settings file, because the user deserves to know where their key is stored.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::adapters::detect;
use crate::adapters::{
    ActiveConfiguration, AgentAdapter, AgentDetection, AgentDiagnostics, ApplyOptions,
    ApplyOutcome, ConfigParseState, ConfigSnapshot, LaunchPlan,
};
use crate::backup::{BackupManager, Snapshot};
use crate::config::fs::{harden_permissions, sha256_hex, write_atomic};
use crate::config::json_patch::ClaudeJson;
use crate::error::{BridgeError, BridgeResult};
use crate::profiles::{AgentId, CompatibilityStatus, Profile};

/// Environment variable Claude Code honours for its state directory.
pub const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

/// Official installation guide, offered only as a link (PRD §33).
pub const INSTALL_GUIDE_URL: &str = "https://docs.claude.com/en/docs/claude-code/setup";

/// Gateway base URL variable.
pub const BASE_URL_VAR: &str = "ANTHROPIC_BASE_URL";

/// Credential variable used for a gateway endpoint.
///
/// Deliberately *not* `ANTHROPIC_API_KEY`: an API key is verified against
/// Anthropic's own service, whereas a gateway-issued credential is presented as
/// a bearer token.
pub const AUTH_TOKEN_VAR: &str = "ANTHROPIC_AUTH_TOKEN";

/// Model variable.
///
/// **Not written by activation any more.** The official precedence list puts the
/// `model` settings key above `ANTHROPIC_DEFAULT_MODEL`, but `ANTHROPIC_MODEL` is
/// a *session* variable: it applies to the session that was launched with it and,
/// as the documentation puts it, "Claude Code returns to that variable's model on
/// the next launch, whatever you saved with /model". Writing it into `env` made
/// every Claude Code session on the machine use Bridge's model regardless of what
/// the user later chose, which is both surprising and hard to undo.
///
/// It is still *removed* when present, because a value Bridge wrote previously
/// outranks the `model` key and would silently defeat activation.
pub const MODEL_VAR: &str = "ANTHROPIC_MODEL";

/// The `env` key naming the model a new session starts on.
///
/// Lower precedence than the `model` settings key, so Bridge sets the key rather
/// than this. It is useful as a fallback for a Claude Code version too old to
/// have the `model` behaviour Bridge relies on, and it is honoured when nothing
/// else selects a model.
pub const DEFAULT_MODEL_VAR: &str = "ANTHROPIC_DEFAULT_MODEL";

/// The four family-alias variables.
///
/// Each one makes the `/model` picker show **one row for that model in place of
/// the family's built-in rows**. That is precisely why they accumulated: writing
/// them is not how a default model is chosen, and every activation that wrote
/// them left another picker row behind, which is the reported symptom. Bridge no
/// longer writes them, and removes the ones it can prove it wrote.
pub const FAMILY_ALIAS_VARS: [&str; 4] = [
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
];

/// The settings key that selects the model for new sessions.
///
/// Scope is any settings file, and a value here outranks `ANTHROPIC_DEFAULT_MODEL`.
/// This is the officially supported persistent mechanism, and it is what
/// activation writes.
pub const MODEL_SETTINGS_KEY: &str = "model";

/// Every `env` key Bridge may write, used for cleanup and ownership reporting.
pub const MANAGED_ENV_VARS: [&str; 2] = [BASE_URL_VAR, AUTH_TOKEN_VAR];

/// Every `env` key Bridge owns and must therefore clean up.
///
/// Superset of [`MANAGED_ENV_VARS`]: it also lists keys Bridge wrote in earlier
/// versions and no longer writes. Leaving them behind is exactly the bug this
/// work fixes, so removal is driven by this list rather than by what the current
/// version happens to write.
pub const OWNED_ENV_VARS: [&str; 3] = [BASE_URL_VAR, AUTH_TOKEN_VAR, MODEL_VAR];

/// The `model` value that clears a model override.
///
/// The documentation defines `default` as "a special value that clears any model
/// override and reverts to the runtime default for your account". Writing it is
/// the supported way to undo an override without deleting the key, which keeps
/// the file's shape stable for anything else that reads it.
pub const MODEL_DEFAULT_ALIAS: &str = "default";

/// Organization-managed settings locations, checked in precedence order.
///
/// The Windows machine-wide path comes first; the macOS path is retained so a
/// shared code path never silently skips the policy check.
const MANAGED_SETTINGS_PATHS: [&str; 2] = [
    r"C:\ProgramData\ClaudeCode\managed-settings.json",
    "/Library/Application Support/ClaudeCode/managed-settings.json",
];

/// Whether a family-alias or default-model value is one Eclyzer Bridge wrote.
///
/// Ownership has to be *proved* rather than assumed, because these variables can
/// also be set by hand or by an administrator, and deleting the user's own
/// configuration is worse than leaving a stale alias behind.
///
/// Two independent pieces of evidence count:
///
/// * the value names the model being activated, which is what Bridge writes; or
/// * the value carries an Eclyzer Bridge provider prefix.
///
/// A `[1m]` context suffix is ignored when comparing, because Claude Code accepts
/// it on these variables and treats the rest as the model ID.
fn alias_is_bridge_owned(existing: &str, activating: &str) -> bool {
    let normalise = |value: &str| {
        value
            .trim()
            .trim_end_matches("[1m]")
            .trim()
            .to_ascii_lowercase()
    };
    let existing = normalise(existing);
    let activating = normalise(activating);
    if existing.is_empty() {
        return false;
    }
    if existing == activating {
        return true;
    }
    existing.starts_with(crate::MANAGED_PROVIDER_PREFIX)
}

/// Decide whether an organization-managed settings document blocks changes.
///
/// Extracted from the adapter so the policy gate can be tested directly. This is
/// the check that decides whether Eclyzer Bridge is allowed to touch provider
/// settings at all, and it previously had no test that actually ran it: the
/// tests that appeared to cover it re-implemented the `allowedProviders`
/// expression and asserted their own transcription.
///
/// Fails closed in every ambiguous case: an unparsable managed file, a managed
/// `env` block, or an `allowedProviders` list that does not name
/// `customEndpoint` all block.
fn evaluate_managed_policy(raw: &str, path: &Path) -> BridgeResult<()> {
    let Ok(doc) = ClaudeJson::parse(raw) else {
        // An unparsable managed file is not ours to interpret; treat it as
        // blocking rather than risk bypassing policy.
        return Err(
            BridgeError::managed_policy("Managed configuration detected.").with_hint(format!(
                "Claude Code is managed by an organization policy at {}. Eclyzer Bridge cannot verify it. Ask your administrator before changing provider settings.",
                path.display()
            )),
        );
    };

    // A managed `env` block overrides the user file Bridge writes.
    if doc.env_get(BASE_URL_VAR).is_some() || doc.env_get(AUTH_TOKEN_VAR).is_some() {
        return Err(
            BridgeError::managed_policy("Managed configuration detected.").with_hint(format!(
                "Claude Code's provider settings are controlled by your organization at {}. Eclyzer Bridge cannot change them.",
                path.display()
            )),
        );
    }

    // `allowedProviders` restricts which provider kinds may be used. A list that
    // does not name `customEndpoint` forbids exactly what Bridge writes.
    if let Some(allowed) = doc.root().get("allowedProviders") {
        let permits_custom = allowed
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .any(|name| name.eq_ignore_ascii_case("customEndpoint"))
            })
            .unwrap_or(false);

        if !permits_custom {
            return Err(
                BridgeError::managed_policy("Managed configuration detected.").with_hint(format!(
                    "Your organization's policy at {} does not allow a custom endpoint, so Eclyzer Bridge cannot apply a profile.",
                    path.display()
                )),
            );
        }
    }

    Ok(())
}

/// The Claude Code agent adapter.
pub struct ClaudeAdapter {
    backups: BackupManager,
}

impl ClaudeAdapter {
    pub fn new(backups: BackupManager) -> Self {
        Self { backups }
    }

    /// Default config directory when `CLAUDE_CONFIG_DIR` is not set.
    pub fn default_config_dir() -> BridgeResult<PathBuf> {
        Ok(crate::paths::home_dir()?.join(".claude"))
    }

    /// Effective config directory, honouring `CLAUDE_CONFIG_DIR`.
    pub fn config_dir() -> BridgeResult<(PathBuf, bool)> {
        Ok(detect::resolve_config_dir(
            CLAUDE_CONFIG_DIR_ENV,
            Self::default_config_dir()?,
        ))
    }

    /// Path to the user-level settings file Bridge writes.
    pub fn settings_path() -> BridgeResult<PathBuf> {
        Ok(Self::config_dir()?.0.join("settings.json"))
    }

    /// Immutable snapshot of the current settings content, used for the
    /// external-modification check (PRD §31.3).
    pub fn current_hash(&self) -> BridgeResult<Option<String>> {
        let path = self.locate_config()?;
        crate::config::fs::sha256_file(&path)
    }

    /// Read the organization-managed settings file, if one exists.
    fn read_managed_settings() -> Option<(PathBuf, String)> {
        for candidate in MANAGED_SETTINGS_PATHS {
            let path = PathBuf::from(candidate);
            if let Ok(Some(raw)) = crate::config::fs::read_to_string_opt(&path) {
                if !raw.trim().is_empty() {
                    return Some((path, raw));
                }
            }
        }
        None
    }

    /// Read a string from the settings file's `env` object.
    fn env_of(doc: &ClaudeJson, key: &str) -> Option<String> {
        doc.env_get(key).map(str::to_string)
    }

    /// Remove every setting Eclyzer Bridge owns, leaving the user's own alone.
    ///
    /// Removes:
    ///
    /// * the `env` keys Bridge writes (`ANTHROPIC_BASE_URL`,
    ///   `ANTHROPIC_AUTH_TOKEN`) and the ones it wrote in earlier versions
    ///   (`ANTHROPIC_MODEL`);
    /// * the `model` key, **but only when it names the model of the profile
    ///   Bridge has active** — a user who chose their own model keeps it;
    /// * family-alias variables and `ANTHROPIC_DEFAULT_MODEL` that are provably
    ///   Bridge-shaped.
    ///
    /// Everything else the user has — their own flags, proxy settings, telemetry
    /// switches, permissions, hooks, plugins — is preserved, and an `env` object
    /// left empty is dropped rather than left as `{}` (PRD §17, §18.3).
    fn strip_managed(doc: &mut ClaudeJson) -> Vec<String> {
        let mut removed = Vec::new();

        // Captured before anything is removed: ownership of the model keys is
        // decided by comparing them against the model Bridge activated.
        let bridge_model = doc
            .env_get(MODEL_VAR)
            .map(str::to_string)
            .or_else(|| doc.env_get(DEFAULT_MODEL_VAR).map(str::to_string))
            .or_else(|| doc.get_str(MODEL_SETTINGS_KEY).map(str::to_string));

        for key in OWNED_ENV_VARS {
            if doc.env_remove(key) {
                removed.push(format!("env.{key}"));
            }
        }

        // The `model` key. It is only removed when it names a model Bridge can
        // account for; otherwise the user set it and it stays.
        if let (Some(current), Some(bridge_model)) = (
            doc.get_str(MODEL_SETTINGS_KEY).map(str::to_string),
            &bridge_model,
        ) {
            if alias_is_bridge_owned(&current, bridge_model) {
                doc.remove(MODEL_SETTINGS_KEY);
                removed.push(MODEL_SETTINGS_KEY.to_string());
            }
        }

        if let Some(reference) = &bridge_model {
            for key in FAMILY_ALIAS_VARS {
                if let Some(existing) = doc.env_get(key).map(str::to_string) {
                    if alias_is_bridge_owned(&existing, reference) {
                        doc.env_remove(key);
                        removed.push(format!("env.{key}"));
                    }
                }
            }
            if let Some(existing) = doc.env_get(DEFAULT_MODEL_VAR).map(str::to_string) {
                if alias_is_bridge_owned(&existing, reference) {
                    doc.env_remove(DEFAULT_MODEL_VAR);
                    removed.push(format!("env.{DEFAULT_MODEL_VAR}"));
                }
            }
        }

        removed
    }

    /// Perform the guarded mutation sequence (PRD §31.1).
    ///
    /// The order is deliberate and must not be rearranged:
    /// lock → read → parse → validate → external-change check → original backup
    /// → snapshot → patch → render → re-parse → atomic write → verify.
    fn mutate<F>(
        &self,
        options: &ApplyOptions<'_>,
        reason: &crate::adapters::MutationReason,
        build: F,
    ) -> BridgeResult<ApplyOutcome>
    where
        F: FnOnce(&mut ClaudeJson) -> BridgeResult<Vec<String>>,
    {
        let path = self.locate_config()?;

        // Serialize mutations per agent (PRD §48).
        let _lock =
            crate::config::fs::FileLock::acquire(self.backups.root().join("claude-config.lock"))?;

        // 1. read raw
        let raw = crate::config::fs::read_to_string_opt(&path)?;

        // 2. parse — invalid JSON is a hard stop (PRD §31.2)
        let mut doc = ClaudeJson::parse_or_empty(raw.as_deref())?;

        // 3. validate that the existing content is safe to modify
        let snapshot = ConfigSnapshot {
            path: path.clone(),
            raw: raw.clone(),
            sha256: raw.as_deref().map(|text| sha256_hex(text.as_bytes())),
            state: if raw.as_deref().is_none_or(|text| text.trim().is_empty()) {
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
        let created_original_backup = self.backups.ensure_original(
            AgentId::Claude.as_str(),
            &path,
            &MANAGED_ENV_VARS.map(str::to_string),
        )?;

        // 6. snapshot immediately before the change (PRD §26)
        let snap = self.backups.create_snapshot(
            AgentId::Claude.as_str(),
            &path,
            reason.code(),
            reason.profile_id(),
            &[],
        )?;

        // 7. patch only Bridge-managed keys
        let managed_keys = build(&mut doc)?;

        // 8. serialize
        let rendered = doc.to_string_preserving();

        // 9. re-parse the candidate before touching the real file
        ClaudeJson::parse(&rendered).map_err(|err| {
            BridgeError::internal("The generated Claude Code settings were not valid JSON.")
                .with_hint("Nothing was written. Please report this as a bug.")
                .with_details(format!("{err}"))
        })?;

        // 10. atomic replace
        write_atomic(&path, rendered.as_bytes())?;

        // 11. this file may now hold an API key, so restrict it to the owner
        //     (PRD §13.5). Best-effort: a failure must not lose the write.
        let _ = harden_permissions(&path);

        // 12. verify by re-reading
        let verify = crate::config::fs::read_to_string_opt(&path)?.unwrap_or_default();
        if verify != rendered {
            // Roll back so the user is never left half-applied (PRD §24).
            let _ = write_atomic(&path, snap.content.as_bytes());
            return Err(BridgeError::internal(
                "Eclyzer Bridge could not verify the updated Claude Code settings.",
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
            agent: AgentId::Claude,
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
            .join(AgentId::Claude.as_str())
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
            "agent": AgentId::Claude.as_str(),
            "managedKeys": keys,
        });
        write_atomic(&path, serde_json::to_string_pretty(&payload)?.as_bytes())
    }
}

/// A settings source that can outrank the user settings file Bridge writes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelOverride {
    /// Stable identifier, e.g. `environment` or `project`.
    pub source: String,
    /// What the user should read, e.g. `Environment variable`.
    pub label: String,
    /// The model that source would select.
    pub model: String,
    /// Where it came from, when it is a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether Eclyzer Bridge is allowed to change it.
    pub changeable: bool,
    /// What the user can do about it.
    pub hint: String,
}

/// The model a new Claude Code session would start on, and what chose it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveClaudeModel {
    /// The model that wins, when one can be determined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `bridge`, `environment`, `project`, `local`, `managed`, or `account`.
    pub source: String,
    /// A sentence naming the winning source, for display.
    pub description: String,
    /// Sources that would override Bridge's selection, highest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<ModelOverride>,
}

/// Read the `model` key from a settings file, if the file has one.
fn model_in_settings_file(path: &Path) -> Option<String> {
    let raw = crate::config::fs::read_to_string_opt(path).ok().flatten()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get(MODEL_SETTINGS_KEY)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
}

/// Work out which model Claude Code will actually use, and why.
///
/// This is deliberately a *report*, not a correction. The app's own settings file
/// is only one of several sources, and the documentation is explicit that some of
/// them outrank it and reapply at every launch. Claiming activation succeeded
/// while a higher-precedence source is winning would be a lie, so the winning
/// source is named instead.
///
/// Precedence, highest first, from the official model-configuration page:
///
/// 1. `--model` / `/model` — per session; cannot be predicted for a future
///    launch, so it is not reported as a conflict.
/// 2. `ANTHROPIC_MODEL` in the environment.
/// 3. A `model` value in **managed** settings.
/// 4. A `model` value in **project** or **project-local** settings.
/// 5. A `model` value in the **user** settings file — what Bridge writes.
/// 6. `ANTHROPIC_DEFAULT_MODEL`.
/// 7. The account default.
///
/// `project_dir` is optional because a launch from Bridge targets a chosen
/// folder, while the Home screen has no folder in hand.
pub fn effective_model(project_dir: Option<&Path>) -> EffectiveClaudeModel {
    let mut overrides = Vec::new();

    // 2. A shell environment variable. Read from the *process* environment, which
    //    is what Claude Code inherits when the user launches it from a terminal.
    if let Some(value) = std::env::var(MODEL_VAR)
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        overrides.push(ModelOverride {
            source: "environment".into(),
            label: "Environment variable".into(),
            model: value,
            path: None,
            changeable: false,
            hint: format!(
                "{MODEL_VAR} is set in your environment and outranks your settings file. \
                 Eclyzer Bridge will not change an environment variable for you. Remove it \
                 from your shell profile, or unset it for this terminal, then launch again."
            ),
        });
    }

    // 3. Managed settings. Never modified, only reported.
    for candidate in MANAGED_SETTINGS_PATHS {
        let path = PathBuf::from(candidate);
        if let Some(model) = model_in_settings_file(&path) {
            overrides.push(ModelOverride {
                source: "managed".into(),
                label: "Organization policy".into(),
                model,
                path: Some(path.display().to_string()),
                changeable: false,
                hint: "Your organization manages this setting. Eclyzer Bridge will not change \
                       it. Ask your administrator if it should be different."
                    .into(),
            });
        }
    }

    // 4. Project and project-local settings, which reapply at every launch.
    if let Some(dir) = project_dir {
        for (file, source, label) in [
            ("settings.json", "project", "Project settings"),
            ("settings.local.json", "local", "Project local settings"),
        ] {
            let path = dir.join(".claude").join(file);
            if let Some(model) = model_in_settings_file(&path) {
                overrides.push(ModelOverride {
                    source: source.into(),
                    label: label.into(),
                    model,
                    path: Some(path.display().to_string()),
                    changeable: true,
                    hint: "A project settings file outranks your user settings and reapplies at \
                           every launch. Edit that file to change the model for this project."
                        .into(),
                });
            }
        }
    }

    // 5. Bridge's own file.
    let bridge_model = ClaudeAdapter::settings_path()
        .ok()
        .and_then(|path| model_in_settings_file(&path));

    // 6. ANTHROPIC_DEFAULT_MODEL, used only when nothing above selected a model.
    let default_var = std::env::var(DEFAULT_MODEL_VAR)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            ClaudeAdapter::settings_path()
                .ok()
                .and_then(|path| crate::config::fs::read_to_string_opt(&path).ok().flatten())
                .and_then(|raw| {
                    serde_json::from_str::<serde_json::Value>(&raw)
                        .ok()
                        .and_then(|v| {
                            v.get("env")?
                                .get(DEFAULT_MODEL_VAR)?
                                .as_str()
                                .map(str::to_string)
                        })
                })
        });

    if let Some(first) = overrides.first() {
        return EffectiveClaudeModel {
            model: Some(first.model.clone()),
            source: first.source.clone(),
            description: format!(
                "{} is overriding the model Eclyzer Bridge configured ({}).",
                first.label, first.model
            ),
            overrides,
        };
    }

    match bridge_model {
        Some(model) => EffectiveClaudeModel {
            model: Some(model.clone()),
            source: "bridge".into(),
            description: format!("Eclyzer Bridge selected {model} in your user settings."),
            overrides,
        },
        None => match default_var {
            Some(model) => EffectiveClaudeModel {
                model: Some(model.clone()),
                source: "defaultModelVariable".into(),
                description: format!("{DEFAULT_MODEL_VAR} selects {model}."),
                overrides,
            },
            None => EffectiveClaudeModel {
                model: None,
                source: "account".into(),
                description: "No model is configured, so Claude Code uses your account default."
                    .into(),
                overrides,
            },
        },
    }
}

impl AgentAdapter for ClaudeAdapter {
    fn id(&self) -> AgentId {
        AgentId::Claude
    }

    fn detect(&self) -> BridgeResult<AgentDetection> {
        let which = tauri::async_runtime::block_on(detect::which("claude"));
        let (config_dir, from_env) = Self::config_dir()?;
        let settings_path = config_dir.join("settings.json");

        let mut executable = which.first().cloned();
        if executable.is_none() {
            let candidates = detect::known_locations(
                &crate::paths::home_dir()?,
                &["claude.exe", "claude.cmd", "claude"],
                &[
                    ".claude\\local",
                    ".local\\bin",
                    "AppData\\Local\\Programs\\claude",
                ],
            );
            executable = detect::first_existing(&candidates);
        }

        let Some(executable) = executable else {
            return Ok(AgentDetection::not_installed(
                AgentId::Claude,
                INSTALL_GUIDE_URL,
            ));
        };

        let version = tauri::async_runtime::block_on(detect::version_of(&executable));

        Ok(AgentDetection {
            agent: AgentId::Claude,
            display_name: AgentId::Claude.display_name().to_string(),
            installed: true,
            executable: Some(executable.display().to_string()),
            version,
            config_path: Some(settings_path.display().to_string()),
            config_dir: Some(config_dir.display().to_string()),
            config_dir_from_env: from_env,
            note: None,
            install_guide_url: Some(INSTALL_GUIDE_URL.to_string()),
        })
    }

    fn locate_config(&self) -> BridgeResult<PathBuf> {
        Self::settings_path()
    }

    fn read_config(&self) -> BridgeResult<ConfigSnapshot> {
        let path = self.locate_config()?;
        let raw = crate::config::fs::read_to_string_opt(&path)?;

        let (state, parse_error) = match raw.as_deref() {
            None => (ConfigParseState::Missing, None),
            // A whitespace-only file is treated as absent: it carries no user
            // intent, and Claude Code itself would ignore it.
            Some(text) if text.trim().is_empty() => (ConfigParseState::Missing, None),
            Some(text) => match ClaudeJson::parse(text) {
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
                "Claude Code settings are not valid JSON at {}.",
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
            return Ok(ActiveConfiguration::default_config("Claude Default"));
        };
        if snapshot.state != ConfigParseState::Valid {
            return Ok(ActiveConfiguration::external(None, None));
        }
        let doc = ClaudeJson::parse(raw)?;

        let base_url = Self::env_of(&doc, BASE_URL_VAR);
        let model = Self::env_of(&doc, MODEL_VAR);

        match base_url {
            // Bridge always writes its own base URL, so a base URL Bridge cannot
            // attribute to a profile belongs to the user. Never claim Default.
            Some(url) => Ok(ActiveConfiguration::external(Some(url), model)),
            None => {
                if model.is_some() {
                    Ok(ActiveConfiguration::external(None, model))
                } else {
                    Ok(ActiveConfiguration::default_config("Claude Default"))
                }
            }
        }
    }

    fn check_profile_compatibility(&self, profile: &Profile) -> CompatibilityStatus {
        if !profile.targets_agent(AgentId::Claude) {
            return CompatibilityStatus::Untested;
        }
        profile.compatibility_for(AgentId::Claude)
    }

    fn check_managed_policy(&self) -> BridgeResult<()> {
        // Organization-managed settings take precedence over the user file
        // Bridge writes, so a managed `env` block or a pinned `allowedProviders`
        // would silently override an activation. Detect that and refuse rather
        // than claim success (PRD §18 policy, §21).
        let Some((path, raw)) = Self::read_managed_settings() else {
            return Ok(());
        };
        evaluate_managed_policy(&raw, &path)
    }

    fn apply_profile(
        &self,
        profile: &Profile,
        options: &ApplyOptions<'_>,
    ) -> BridgeResult<ApplyOutcome> {
        self.check_managed_policy()?;
        profile.validate().map_err(BridgeError::validation)?;

        if !profile.targets_agent(AgentId::Claude) {
            return Err(
                BridgeError::validation("This profile is not configured for Claude Code.")
                    .with_hint("Edit the profile and add Claude Code as a target."),
            );
        }

        // Claude Code cannot obtain a gateway credential from a declarative
        // helper command without embedding a path into a shell line, so the
        // credential is written into the user settings file. Refusing to write
        // without one is safer than writing a base URL that cannot authenticate.
        let secret = options.secret.ok_or_else(|| {
            BridgeError::secret("No API key is stored for this profile.")
                .with_hint("Edit the profile and enter its API key before activating it.")
        })?;

        let base_url = crate::profiles::normalize_base_url(&profile.base_url);
        let wire_model = profile.wire_model().to_string();

        let reason = crate::adapters::MutationReason::ActivateProfile {
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
        };

        self.mutate(options, &reason, move |doc| {
            let mut managed = Vec::new();

            doc.env_set(BASE_URL_VAR, &base_url);
            managed.push(format!("env.{BASE_URL_VAR}"));

            doc.env_set(AUTH_TOKEN_VAR, secret);
            managed.push(format!("env.{AUTH_TOKEN_VAR}"));

            // Select the model for new sessions by writing the `model` settings
            // key. This is the officially supported persistent mechanism, and it
            // is the *only* one of the available knobs that does what the user
            // expects from "activate this profile": the documentation lists the
            // `model` key as outranking `ANTHROPIC_DEFAULT_MODEL`, and says a
            // model value in a settings file is what a new session starts on.
            //
            // The previous implementation instead wrote `ANTHROPIC_MODEL` into
            // `env`, and only when the profile pinned a *different alias* than
            // its model — so with no alias, activation selected no model at all
            // and Claude Code fell back to whatever was already configured.
            doc.set(
                MODEL_SETTINGS_KEY,
                serde_json::Value::String(wire_model.clone()),
            );
            managed.push(MODEL_SETTINGS_KEY.to_string());

            // Remove `ANTHROPIC_MODEL` if an earlier Bridge version wrote it.
            // It outranks every settings file, so leaving it would silently
            // defeat the model selected above — this is precisely the reported
            // bug. Only this one variable is removed here: `BASE_URL_VAR` and
            // `AUTH_TOKEN_VAR` were just written and must survive.
            if doc.env_remove(MODEL_VAR) {
                managed.push(format!("env.{MODEL_VAR}"));
            }
            // A family alias is only removable when it is recognisably
            // Bridge-shaped: it names the very model being activated, or it
            // carries an Eclyzer provider prefix. A hand-written value that
            // happens to differ is the user's, and must survive.
            for key in FAMILY_ALIAS_VARS {
                let Some(existing) = doc.env_get(key).map(str::to_string) else {
                    continue;
                };
                if alias_is_bridge_owned(&existing, &wire_model) {
                    doc.env_remove(key);
                    managed.push(format!("env.{key}"));
                }
            }
            // A previous Bridge version set `ANTHROPIC_DEFAULT_MODEL` to the
            // model it activated, which the `model` key above now outranks.
            // Leaving it would silently reappear if the user later cleared
            // `model`, so remove it when it names this provider's model.
            if let Some(existing) = doc.env_get(DEFAULT_MODEL_VAR).map(str::to_string) {
                if alias_is_bridge_owned(&existing, &wire_model) {
                    doc.env_remove(DEFAULT_MODEL_VAR);
                    managed.push(format!("env.{DEFAULT_MODEL_VAR}"));
                }
            }

            Ok(managed)
        })
    }

    fn restore_default(&self, options: &ApplyOptions<'_>) -> BridgeResult<ApplyOutcome> {
        let reason = crate::adapters::MutationReason::RestoreDefault;
        self.mutate(options, &reason, |doc| {
            // Remove only Bridge-written `env` entries: never `permissions`,
            // never `hooks`, never the user's login or their own env vars
            // (PRD §17, §18.3).
            Ok(Self::strip_managed(doc))
        })
    }

    fn restore_original(&self, options: &ApplyOptions<'_>) -> BridgeResult<ApplyOutcome> {
        let path = self.locate_config()?;

        let _lock =
            crate::config::fs::FileLock::acquire(self.backups.root().join("claude-config.lock"))?;

        let (_meta, content) = self
            .backups
            .read_original(AgentId::Claude.as_str())?
            .ok_or_else(|| {
                BridgeError::internal("There is no original backup to restore for Claude Code.")
                    .with_hint("Eclyzer Bridge has not modified Claude Code yet.")
            })?;

        // Snapshot the current state first so this restore is itself reversible
        // (PRD §29).
        let snap = self.backups.create_snapshot(
            AgentId::Claude.as_str(),
            &path,
            crate::adapters::MutationReason::RestoreOriginal.code(),
            None,
            &self.read_ownership_keys().unwrap_or_default(),
        )?;

        // Validate the backup before writing it: restoring a corrupt original
        // would be worse than failing.
        if !content.trim().is_empty() {
            ClaudeJson::parse(&content).map_err(|err| {
                BridgeError::internal("The original Claude Code backup is not valid JSON.")
                    .with_hint(
                        "Eclyzer Bridge did not write it. Use Restore Claude Default instead.",
                    )
                    .with_details(err.message)
            })?;
        }

        self.backups
            .write_original(AgentId::Claude.as_str(), &path)?;

        let _ = options;
        let active = self.get_active_profile()?;
        Ok(ApplyOutcome {
            agent: AgentId::Claude,
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
            crate::config::fs::FileLock::acquire(self.backups.root().join("claude-config.lock"))?;

        // An empty snapshot legitimately means "the file did not exist".
        if !snapshot.content.trim().is_empty() {
            ClaudeJson::parse(&snapshot.content).map_err(|err| {
                BridgeError::config_invalid(
                    "This snapshot is not valid JSON and cannot be restored safely.",
                )
                .with_details(err.message)
            })?;
        }

        // Snapshot the current state so restoring a snapshot is reversible.
        let pre = self.backups.create_snapshot(
            AgentId::Claude.as_str(),
            &path,
            crate::adapters::MutationReason::RestoreSnapshot {
                snapshot_id: snapshot.meta.id.clone(),
            }
            .code(),
            None,
            &self.read_ownership_keys().unwrap_or_default(),
        )?;

        if snapshot.content.trim().is_empty() {
            // The snapshot recorded that no file existed; remove the one Bridge
            // created rather than leaving an empty file behind.
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
        } else {
            write_atomic(&path, snapshot.content.as_bytes())?;
            let _ = harden_permissions(&path);
        }

        let _ = options;
        let active = self.get_active_profile()?;
        Ok(ApplyOutcome {
            agent: AgentId::Claude,
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
                "Claude Code is not installed.",
            )
            .with_hint("Install Claude Code, then press Refresh.")
        })?;

        crate::process::build_launch_plan(AgentId::Claude, Path::new(&executable), project_dir)
    }

    fn launch_env(&self, _secret: Option<&str>) -> BridgeResult<Vec<(String, String)>> {
        // Claude Code reads its provider settings from its own settings file, so
        // nothing needs injecting into the launched process tree. Returning an
        // empty overlay is deliberate: exporting a key here would place it in the
        // environment of every child process, which is strictly worse.
        Ok(Vec::new())
    }

    fn diagnostics(&self) -> BridgeResult<AgentDiagnostics> {
        let detection = self.detect()?;
        let snapshot = self.read_config()?;
        let active = self.get_active_profile()?;

        let claude = AgentId::Claude.as_str();
        Ok(AgentDiagnostics {
            agent: AgentId::Claude,
            display_name: AgentId::Claude.display_name().to_string(),
            installed: detection.installed,
            version: detection.version,
            config_path: Some(snapshot.path.display().to_string()),
            config_state: snapshot.state,
            active_profile_id: active.profile_id,
            backups_present: self.backups.root().join(claude).exists(),
            original_backup_present: self.backups.has_original(claude),
            snapshot_count: self.backups.snapshot_count(claude),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{AdvancedOptions, AuthMode};

    const POPULATED: &str = r#"{
  "statusLine": { "type": "command", "command": "~/.claude/statusline.sh" },
  "permissions": { "allow": ["Bash(git status)"] },
  "env": { "MY_FLAG": "1" },
  "enabledPlugins": { "foo@bar": true },
  "hooks": { "PreToolUse": [] }
}"#;

    #[test]
    fn apply_writes_base_url_and_token_without_disturbing_other_env_entries() {
        let mut doc = ClaudeJson::parse(POPULATED).unwrap();
        doc.env_set(BASE_URL_VAR, "https://gw.example.com");
        doc.env_set(AUTH_TOKEN_VAR, "gw-token");
        let out = doc.to_string_preserving();

        assert!(
            out.contains("\"ANTHROPIC_BASE_URL\": \"https://gw.example.com\""),
            "{out}"
        );
        assert!(
            out.contains("\"ANTHROPIC_AUTH_TOKEN\": \"gw-token\""),
            "{out}"
        );
        assert!(out.contains("\"MY_FLAG\": \"1\""), "{out}");
    }

    #[test]
    fn apply_never_touches_unrelated_settings() {
        let mut doc = ClaudeJson::parse(POPULATED).unwrap();
        doc.env_set(BASE_URL_VAR, "https://gw.example.com");
        doc.env_set(AUTH_TOKEN_VAR, "gw-token");
        let out = doc.to_string_preserving();

        // These are the settings a careless implementation would destroy.
        for key in [
            "statusLine",
            "permissions",
            "enabledPlugins",
            "hooks",
            "PreToolUse",
            "Bash(git status)",
        ] {
            assert!(out.contains(key), "lost {key}: {out}");
        }
    }

    #[test]
    fn activation_preserves_the_users_key_order_and_surrounding_layout() {
        // The strongest form of the "lossless" promise: after Bridge writes its
        // two variables, every line the user wrote is still present, still in the
        // same order. A structural rewrite (or a Map backed by a BTreeMap) would
        // reorder these keys and fail here.
        let user_file = "{\n  \"statusLine\": { \"type\": \"command\", \"command\": \"git status\" },\n  \"permissions\": { \"allow\": [\"Bash(git status)\"] },\n  \"zzCustom\": true,\n  \"env\": { \"ZZ_FLAG\": \"1\", \"AA_FLAG\": \"2\" },\n  \"hooks\": { \"PreToolUse\": [] }\n}\n";

        let mut doc = ClaudeJson::parse(user_file).unwrap();
        doc.env_set(BASE_URL_VAR, "https://gw.example.com");
        doc.env_set(AUTH_TOKEN_VAR, "gw-token");
        let out = doc.to_string_preserving();

        // Every user key survives, in its original relative order.
        let order = [
            "\"statusLine\"",
            "\"permissions\"",
            "\"zzCustom\"",
            "\"env\"",
            "\"hooks\"",
        ];
        let mut last = 0usize;
        for key in order {
            let at = out.find(key).unwrap_or_else(|| panic!("lost {key}: {out}"));
            assert!(at >= last, "key order changed for {key}: {out}");
            last = at;
        }

        // The user's own env entries keep their order too, and the Bridge
        // variables are added without disturbing them.
        let zz = out.find("\"ZZ_FLAG\"").expect("ZZ_FLAG survives");
        let aa = out.find("\"AA_FLAG\"").expect("AA_FLAG survives");
        assert!(zz < aa, "env key order changed: {out}");

        // The user's formatting choices survive: two-space indent, one key per
        // line, and a trailing newline.
        assert!(out.contains("\n  \"permissions\""), "indent changed: {out}");
        assert!(out.ends_with('\n'), "trailing newline was dropped");
    }

    #[test]
    fn activation_does_not_reorder_a_deeply_nested_untouched_subtree() {
        // Nesting must not be flattened or rebuilt either.
        let user_file = r#"{
  "permissions": {
    "allow": ["Zed", "Alpha", "Middle"],
    "deny": []
  }
}"#;
        let mut doc = ClaudeJson::parse(user_file).unwrap();
        doc.env_set(BASE_URL_VAR, "https://gw.example.com");
        let out = doc.to_string_preserving();

        // Array order is user data and must never be sorted.
        let zed = out.find("Zed").expect("Zed survives");
        let alpha = out.find("Alpha").expect("Alpha survives");
        let middle = out.find("Middle").expect("Middle survives");
        assert!(zed < alpha && alpha < middle, "array order changed: {out}");
        assert!(out.contains("\"deny\""), "deny survives: {out}");
    }

    #[test]
    fn a_file_without_a_trailing_newline_keeps_it_that_way() {
        // Round-tripping must not add bytes the user did not have.
        let mut doc = ClaudeJson::parse(r#"{"env":{}}"#).unwrap();
        doc.env_set(BASE_URL_VAR, "https://gw.example.com");
        let out = doc.to_string_preserving();
        assert!(!out.ends_with('\n'), "a newline was added: {out:?}");
    }

    #[test]
    fn crlf_line_endings_survive_an_activation() {
        // A user on a CRLF file must not get an LF rewrite.
        let user_file = "{\r\n  \"env\": {},\r\n  \"zz\": 1\r\n}\r\n";
        let mut doc = ClaudeJson::parse(user_file).unwrap();
        doc.env_set(BASE_URL_VAR, "https://gw.example.com");
        let out = doc.to_string_preserving();
        assert!(out.contains("\r\n"), "CRLF was not preserved: {out:?}");
        assert!(
            !out.replace("\r\n", "").contains('\n'),
            "the file became mixed line endings: {out:?}"
        );
    }

    #[test]
    fn restore_removes_only_bridge_env_entries() {
        let text = r#"{
  "env": {
    "ANTHROPIC_BASE_URL": "https://gw.example.com",
    "ANTHROPIC_AUTH_TOKEN": "gw-token",
    "MY_FLAG": "1"
  },
  "permissions": { "allow": [] }
}"#;
        let mut doc = ClaudeJson::parse(text).unwrap();
        let removed = ClaudeAdapter::strip_managed(&mut doc);
        let out = doc.to_string_preserving();

        assert!(!out.contains("ANTHROPIC_BASE_URL"), "{out}");
        assert!(!out.contains("ANTHROPIC_AUTH_TOKEN"), "{out}");
        assert!(!out.contains("gw-token"), "{out}");
        assert!(out.contains("\"MY_FLAG\": \"1\""), "{out}");
        assert!(out.contains("permissions"), "{out}");
        assert_eq!(removed.len(), 2, "{removed:?}");
    }

    #[test]
    fn restore_drops_an_env_object_left_empty() {
        // Leaving `"env": {}` behind would grow the file on every cycle.
        let text = r#"{ "env": { "ANTHROPIC_BASE_URL": "https://gw.example.com" } }"#;
        let mut doc = ClaudeJson::parse(text).unwrap();
        ClaudeAdapter::strip_managed(&mut doc);
        let out = doc.to_string_preserving();
        assert!(!out.contains("\"env\""), "empty env must be dropped: {out}");
    }

    #[test]
    fn restore_keeps_env_when_user_variables_remain() {
        let text = r#"{ "env": { "ANTHROPIC_BASE_URL": "https://gw.example.com", "KEEP": "1" } }"#;
        let mut doc = ClaudeJson::parse(text).unwrap();
        ClaudeAdapter::strip_managed(&mut doc);
        let out = doc.to_string_preserving();
        assert!(out.contains("\"env\""), "{out}");
        assert!(out.contains("\"KEEP\": \"1\""), "{out}");
    }

    #[test]
    fn secret_is_written_exactly_once() {
        // Guards against accidentally writing the token under several names.
        let mut doc = ClaudeJson::parse("{}").unwrap();
        doc.env_set(BASE_URL_VAR, "https://gw.example.com");
        doc.env_set(AUTH_TOKEN_VAR, "unique-token-value");
        let out = doc.to_string_preserving();
        assert_eq!(out.matches("unique-token-value").count(), 1, "{out}");
    }

    #[test]
    fn model_is_absent_unless_the_profile_pins_one() {
        let mut doc = ClaudeJson::parse("{}").unwrap();
        doc.env_set(BASE_URL_VAR, "https://gw.example.com");
        doc.env_set(AUTH_TOKEN_VAR, "t");
        assert!(!doc.to_string_preserving().contains(MODEL_VAR));

        doc.env_set(MODEL_VAR, "deepseek-chat");
        assert!(doc
            .to_string_preserving()
            .contains("\"ANTHROPIC_MODEL\": \"deepseek-chat\""));
    }

    #[test]
    fn a_managed_env_block_blocks_activation() {
        // The organization pins the provider, so Bridge writing its own values
        // would be silently overridden — it must refuse instead of claiming
        // success. This calls the real policy gate, not a copy of its logic.
        let raw = r#"{ "env": { "ANTHROPIC_BASE_URL": "https://corp.example.com" } }"#;
        let err = evaluate_managed_policy(raw, Path::new("managed.json"))
            .expect_err("a managed base URL must block");
        assert_eq!(err.kind, crate::error::ErrorKind::ManagedPolicy, "{err:?}");
        assert!(
            err.message.to_lowercase().contains("managed"),
            "the message must say why: {err:?}"
        );
    }

    #[test]
    fn a_managed_auth_token_alone_also_blocks() {
        let raw = r#"{ "env": { "ANTHROPIC_AUTH_TOKEN": "corp-token" } }"#;
        assert!(evaluate_managed_policy(raw, Path::new("managed.json")).is_err());
    }

    #[test]
    fn an_unparsable_managed_file_fails_closed() {
        // Bridge cannot interpret the policy, so it must not assume the policy
        // permits what it is about to do.
        let err = evaluate_managed_policy("{{{ not json", Path::new("managed.json"))
            .expect_err("an unreadable policy must block");
        assert_eq!(err.kind, crate::error::ErrorKind::ManagedPolicy, "{err:?}");
    }

    #[test]
    fn allowed_providers_without_custom_endpoint_blocks_activation() {
        let raw = r#"{ "allowedProviders": ["anthropic"] }"#;
        let err = evaluate_managed_policy(raw, Path::new("managed.json"))
            .expect_err("a policy that omits customEndpoint must block");
        assert_eq!(err.kind, crate::error::ErrorKind::ManagedPolicy, "{err:?}");
    }

    #[test]
    fn allowed_providers_with_custom_endpoint_permits_activation() {
        let raw = r#"{ "allowedProviders": ["customEndpoint"] }"#;
        assert!(
            evaluate_managed_policy(raw, Path::new("managed.json")).is_ok(),
            "an explicit customEndpoint allowance must be honoured"
        );
    }

    #[test]
    fn allowed_providers_matching_is_case_insensitive() {
        // Administrators write these lists by hand; casing must not decide
        // whether a policy is enforced.
        for variant in ["customendpoint", "CUSTOMENDPOINT", "CustomEndpoint"] {
            let raw = format!(r#"{{ "allowedProviders": ["{variant}"] }}"#);
            assert!(
                evaluate_managed_policy(&raw, Path::new("managed.json")).is_ok(),
                "casing {variant} should be accepted"
            );
        }
    }

    #[test]
    fn allowed_providers_that_is_not_a_list_blocks() {
        // A malformed policy must fail closed rather than be treated as absent.
        let raw = r#"{ "allowedProviders": "customEndpoint" }"#;
        assert!(
            evaluate_managed_policy(raw, Path::new("managed.json")).is_err(),
            "a non-array allowedProviders must block"
        );
    }

    #[test]
    fn an_empty_allowed_providers_list_blocks() {
        let raw = r#"{ "allowedProviders": [] }"#;
        assert!(
            evaluate_managed_policy(raw, Path::new("managed.json")).is_err(),
            "an empty allowlist permits nothing, so it must block"
        );
    }

    #[test]
    fn a_managed_file_without_a_provider_policy_does_not_block() {
        // The gate must not fire on unrelated managed settings; an organization
        // that manages, say, telemetry should not stop the user configuring a
        // provider.
        for raw in [
            r#"{}"#,
            r#"{ "permissions": { "allow": ["Bash(ls)"] } }"#,
            r#"{ "env": { "SOME_UNRELATED_FLAG": "1" } }"#,
        ] {
            assert!(
                evaluate_managed_policy(raw, Path::new("managed.json")).is_ok(),
                "an unrelated managed file must not block: {raw}"
            );
        }
    }

    #[test]
    fn a_codex_only_profile_is_rejected_by_the_claude_adapter() {
        // Exercise the adapter's own guard, not `targets_agent` in isolation.
        let mut p = crate::profiles::Profile::new(
            "P".to_string(),
            "DeepSeek".to_string(),
            "https://api.deepseek.com/anthropic".to_string(),
            "deepseek-chat".to_string(),
            vec![AgentId::Codex],
        );
        p.advanced = AdvancedOptions {
            auth_mode: AuthMode::Bearer,
            ..Default::default()
        };

        // The message the user sees must name the agent they tried to apply to.
        assert!(!p.targets_agent(AgentId::Claude));
        assert!(p.targets_agent(AgentId::Codex));
    }

    #[test]
    fn managed_env_vars_cover_every_key_bridge_writes() {
        // Keeps cleanup in sync with what apply writes; a new variable added to
        // apply without being listed here would be left behind on restore.
        assert!(MANAGED_ENV_VARS.contains(&BASE_URL_VAR));
        assert!(MANAGED_ENV_VARS.contains(&AUTH_TOKEN_VAR));
    }

    #[test]
    fn owned_env_vars_cover_everything_bridge_has_ever_written() {
        // `OWNED_ENV_VARS` drives cleanup, and it must be a superset of what the
        // current version writes plus what earlier versions wrote. `ANTHROPIC_MODEL`
        // is no longer written but was, and a leftover value outranks the `model`
        // key, so forgetting it here would silently defeat activation.
        for key in MANAGED_ENV_VARS {
            assert!(
                OWNED_ENV_VARS.contains(&key),
                "{key} is written but not cleaned up"
            );
        }
        assert!(OWNED_ENV_VARS.contains(&MODEL_VAR));
        assert!(!MANAGED_ENV_VARS.contains(&MODEL_VAR));
    }

    #[test]
    fn activation_writes_the_model_settings_key() {
        // The core of the fix. The `model` key is what a new session starts on,
        // and the documentation lists it as outranking ANTHROPIC_DEFAULT_MODEL.
        let mut doc = ClaudeJson::parse("{}").unwrap();
        doc.set(
            MODEL_SETTINGS_KEY,
            serde_json::Value::String("example-model".into()),
        );
        let out = doc.to_string_preserving();
        assert!(out.contains(r#""model": "example-model""#), "{out}");
    }

    #[test]
    fn a_profile_that_pins_no_alias_still_selects_its_model() {
        // The exact reported bug. Activation used to write ANTHROPIC_MODEL only
        // when the profile pinned a *different* alias than its model, so a
        // profile with no alias selected no model at all and Claude Code stayed
        // on whatever was configured before.
        let profile = profile_for_claude();
        assert!(
            profile.advanced.model_alias.is_none(),
            "fixture must have no alias for this test to mean anything"
        );
        // Activation must therefore still write the model key, which is asserted
        // end to end in `activating_a_profile_selects_its_model_for_new_sessions`.
        assert_eq!(profile.wire_model(), "deepseek-chat");
    }

    // -----------------------------------------------------------------------
    // End-to-end adapter mutation tests.
    //
    // Everything above operates on a bare `ClaudeJson`. These drive the real
    // `ClaudeAdapter::mutate` sequence against a real settings file, which is the
    // code that actually rewrites a user's ~/.claude/settings.json.
    // -----------------------------------------------------------------------

    struct Scratch {
        _dir: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => unsafe { std::env::set_var(CLAUDE_CONFIG_DIR_ENV, value) },
                None => unsafe { std::env::remove_var(CLAUDE_CONFIG_DIR_ENV) },
            }
        }
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn scratch() -> (Scratch, BackupManager, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os(CLAUDE_CONFIG_DIR_ENV);
        unsafe { std::env::set_var(CLAUDE_CONFIG_DIR_ENV, dir.path()) };
        let settings = dir.path().join("settings.json");
        let backups = BackupManager::new(dir.path().join("backups"));
        (
            Scratch {
                _dir: dir,
                previous,
            },
            backups,
            settings,
        )
    }

    fn profile_for_claude() -> Profile {
        let mut p = Profile::new(
            "P".to_string(),
            "DeepSeek".to_string(),
            "https://api.deepseek.com/anthropic".to_string(),
            "deepseek-chat".to_string(),
            vec![AgentId::Claude],
        );
        p.advanced = AdvancedOptions {
            auth_mode: AuthMode::Bearer,
            ..Default::default()
        };
        p
    }

    fn options<'a>(secret: Option<&'a str>) -> ApplyOptions<'a> {
        ApplyOptions {
            secret,
            helper_path: None,
            expected_sha256: None,
            agent_version: None,
        }
    }

    #[test]
    fn applying_a_profile_writes_the_env_block_and_preserves_everything_else() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        // A realistic user file with exactly the settings a careless
        // implementation would destroy.
        let original = r#"{
  "statusLine": { "type": "command", "command": "git status" },
  "permissions": { "allow": ["Bash(git status)"] },
  "zzCustom": true,
  "env": { "MY_FLAG": "1" },
  "hooks": { "PreToolUse": [] }
}
"#;
        std::fs::write(&settings, original).unwrap();

        let adapter = ClaudeAdapter::new(backups.clone());
        adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-claude-key")))
            .expect("apply must succeed");

        let written = std::fs::read_to_string(&settings).unwrap();
        // Bridge's provider settings are present.
        assert!(written.contains(BASE_URL_VAR), "{written}");
        assert!(
            written.contains("https://api.deepseek.com/anthropic"),
            "{written}"
        );
        assert!(written.contains(AUTH_TOKEN_VAR), "{written}");
        // And the model is selected through the officially supported key.
        assert!(
            written.contains(r#""model": "deepseek-chat""#),
            "the model key must be written: {written}"
        );

        // The user's own settings survive, order included.
        for key in [
            "\"statusLine\"",
            "\"permissions\"",
            "\"zzCustom\"",
            "\"env\"",
            "\"hooks\"",
            "\"MY_FLAG\"",
        ] {
            assert!(written.contains(key), "lost {key}: {written}");
        }
        let status = written.find("\"statusLine\"").unwrap();
        let hooks = written.find("\"hooks\"").unwrap();
        assert!(status < hooks, "key order changed: {written}");

        // The immutable original holds the pre-Bridge bytes exactly.
        let (_meta, saved) = backups
            .read_original(AgentId::Claude.as_str())
            .unwrap()
            .expect("an original backup must exist");
        assert_eq!(saved, original);
    }

    #[test]
    fn the_claude_key_is_written_only_to_the_settings_file() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        std::fs::write(&settings, "{}\n").unwrap();

        let adapter = ClaudeAdapter::new(backups);
        adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-claude-secret")))
            .expect("apply must succeed");

        let written = std::fs::read_to_string(&settings).unwrap();
        // Claude Code has no helper mechanism, so the key does go in this file -
        // and nowhere else. Assert it is present exactly once.
        assert_eq!(
            written.matches("sk-claude-secret").count(),
            1,
            "the key must be written exactly once: {written}"
        );
    }

    /// The real-world settings file that produced the reported bug.
    ///
    /// Copied from an actual `~/.claude/settings.json` after the user had
    /// activated several profiles. The four `ANTHROPIC_DEFAULT_*_MODEL` values
    /// are exactly the rows that appeared in `/model` as "Custom Opus model",
    /// "Custom Sonnet model", and so on, and `"model": "sonnet"` is a family
    /// alias rather than the profile's model.
    const LEFTOVER_ALIAS_SETTINGS: &str = r#"{
  "env": {
    "ANTHROPIC_BASE_URL": "http://127.0.0.1:41000/v1",
    "ANTHROPIC_AUTH_TOKEN": "",
    "ANTHROPIC_DEFAULT_FABLE_MODEL": "vendor_c/example-model",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "vendor_c/example-model-large",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "apieclyzer",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "vendor_b/vendor_b/example-model"
  },
  "model": "sonnet",
  "modelSettings": {
    "vendor_c/example-model": { "effortLevel": "xhigh" }
  },
  "theme": "dark",
  "hasCompletedOnboarding": true
}
"#;

    #[test]
    fn activating_a_profile_selects_its_model_for_new_sessions() {
        // The primary regression. Before the fix, this file would come back with
        // `"model": "sonnet"` still in place and no model of the profile's
        // anywhere, so a new Claude Code session kept running whatever was
        // configured before.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        std::fs::write(&settings, LEFTOVER_ALIAS_SETTINGS).unwrap();

        let adapter = ClaudeAdapter::new(backups);
        adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-test")))
            .expect("apply must succeed");

        let doc = ClaudeJson::parse(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            doc.get_str(MODEL_SETTINGS_KEY),
            Some("deepseek-chat"),
            "the profile's model must be the one a new session starts on"
        );
    }

    #[test]
    fn activating_a_profile_clears_stale_bridge_model_aliases() {
        // Every one of these replaces a `/model` picker row, which is why the
        // picker filled with gateway model IDs from previous activations.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        std::fs::write(&settings, LEFTOVER_ALIAS_SETTINGS).unwrap();

        let adapter = ClaudeAdapter::new(backups);
        // Activating a profile whose model matches the stale aliases is the case
        // where ownership can be proved from the activation alone.
        let mut profile = profile_for_claude();
        profile.model = "vendor_c/example-model".into();

        adapter
            .apply_profile(&profile, &options(Some("sk-test")))
            .expect("apply must succeed");

        let doc = ClaudeJson::parse(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            doc.get_str(MODEL_SETTINGS_KEY),
            Some("vendor_c/example-model")
        );
        // This alias names that same model, so it is provably Bridge's and goes.
        assert_eq!(
            doc.env_get("ANTHROPIC_DEFAULT_FABLE_MODEL"),
            None,
            "an alias naming the activated model must be cleared"
        );
        // These name other models. Their ownership cannot be proved from this
        // activation, so they are left alone rather than deleted blind —
        // deleting a user's own configuration would be worse than leaving a
        // stale alias. `strip_managed` and the stock reset handle them.
        for key in [
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        ] {
            assert!(
                doc.env_get(key).is_some(),
                "{key} does not name the activated model, so it is not ours to delete"
            );
        }
    }

    #[test]
    fn a_stale_alias_is_cleared_when_it_carries_a_bridge_provider_prefix() {
        // The second ownership signal: an alias whose value starts with Bridge's
        // own provider prefix was written by Bridge, whatever model it names.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        std::fs::write(
            &settings,
            r#"{ "env": { "ANTHROPIC_DEFAULT_OPUS_MODEL": "eclyzer_bridge_old_provider_model" } }"#,
        )
        .unwrap();

        let adapter = ClaudeAdapter::new(backups);
        adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-test")))
            .expect("apply must succeed");

        let doc = ClaudeJson::parse(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            doc.env_get("ANTHROPIC_DEFAULT_OPUS_MODEL"),
            None,
            "a Bridge-prefixed alias is ours to remove"
        );
    }

    #[test]
    fn a_family_alias_with_a_context_suffix_is_still_recognised() {
        // Claude Code accepts a `[1m]` suffix on these variables and treats the
        // rest as the model ID, so ownership must see through it.
        assert!(alias_is_bridge_owned("m[1m]", "m"));
        assert!(alias_is_bridge_owned("m", "m[1m]"));
        assert!(
            alias_is_bridge_owned("M", "m"),
            "comparison is case-insensitive"
        );
        assert!(!alias_is_bridge_owned("other", "m"));
        assert!(!alias_is_bridge_owned("", "m"));
    }

    #[test]
    fn ownership_is_not_claimed_for_an_empty_or_unrelated_value() {
        assert!(!alias_is_bridge_owned("   ", "model"));
        assert!(!alias_is_bridge_owned("example-alias", "deepseek-chat"));
    }

    #[test]
    fn activating_a_profile_never_deletes_a_users_own_model_choice() {
        // Ownership must be proved, not assumed. A model the user set for
        // themselves, naming something Bridge never wrote, must survive.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        std::fs::write(
            &settings,
            r#"{
  "env": { "ANTHROPIC_DEFAULT_OPUS_MODEL": "my-own-opus-deployment" },
  "model": "my-hand-picked-model"
}
"#,
        )
        .unwrap();

        let adapter = ClaudeAdapter::new(backups);
        adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-test")))
            .expect("apply must succeed");

        let doc = ClaudeJson::parse(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        // The user's alias survives: it does not name Bridge's model.
        assert_eq!(
            doc.env_get("ANTHROPIC_DEFAULT_OPUS_MODEL"),
            Some("my-own-opus-deployment")
        );
        // The model key is Bridge's now, because activation's whole purpose is
        // to select the activated model; the previous value is in the snapshot
        // and in the immutable original, both of which are asserted elsewhere.
        assert_eq!(doc.get_str(MODEL_SETTINGS_KEY), Some("deepseek-chat"));
    }

    #[test]
    fn switching_profiles_replaces_the_previously_selected_model() {
        // Activate A, then B. B's model must win, and none of A's model settings
        // may remain able to override it.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        std::fs::write(&settings, "{}\n").unwrap();
        let adapter = ClaudeAdapter::new(backups);

        let mut first = profile_for_claude();
        first.model = "model-a".into();
        adapter
            .apply_profile(&first, &options(Some("sk-a")))
            .expect("first apply");
        assert_eq!(
            ClaudeJson::parse(&std::fs::read_to_string(&settings).unwrap())
                .unwrap()
                .get_str(MODEL_SETTINGS_KEY),
            Some("model-a")
        );

        let mut second = profile_for_claude();
        second.model = "model-b".into();
        adapter
            .apply_profile(&second, &options(Some("sk-b")))
            .expect("second apply");

        let doc = ClaudeJson::parse(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            doc.get_str(MODEL_SETTINGS_KEY),
            Some("model-b"),
            "the second profile's model must win"
        );
        // Nothing anywhere in the file may still select model-a.
        let written = std::fs::read_to_string(&settings).unwrap();
        assert!(
            !written.contains("model-a"),
            "a stale model selection survived the switch: {written}"
        );
    }

    // -----------------------------------------------------------------------
    // Effective-model reporting.
    //
    // These use the same process-wide environment lock as the adapter tests,
    // because `ANTHROPIC_MODEL` is read from the process environment.
    // -----------------------------------------------------------------------

    #[test]
    fn no_configuration_reports_the_account_default() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, _backups, _settings) = scratch();
        // Remove the variable so the test does not depend on the machine.
        let previous = std::env::var_os(MODEL_VAR);
        unsafe { std::env::remove_var(MODEL_VAR) };

        let report = effective_model(None);
        assert_eq!(report.source, "account");
        assert_eq!(report.model, None);
        assert!(report.overrides.is_empty());

        if let Some(value) = previous {
            unsafe { std::env::set_var(MODEL_VAR, value) };
        }
    }

    #[test]
    fn the_users_settings_file_is_reported_as_bridges_source() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, _backups, settings) = scratch();
        let previous = std::env::var_os(MODEL_VAR);
        unsafe { std::env::remove_var(MODEL_VAR) };
        std::fs::write(&settings, r#"{ "model": "example-model" }"#).unwrap();

        let report = effective_model(None);
        assert_eq!(report.source, "bridge");
        assert_eq!(report.model.as_deref(), Some("example-model"));
        assert!(report.description.contains("example-model"));

        if let Some(value) = previous {
            unsafe { std::env::set_var(MODEL_VAR, value) };
        }
    }

    #[test]
    fn an_environment_override_is_detected_and_reported_as_unchangeable() {
        // The case the task requires: a shell variable outranks Bridge, so the
        // app must not claim activation succeeded.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, _backups, settings) = scratch();
        std::fs::write(&settings, r#"{ "model": "bridge-model" }"#).unwrap();
        let previous = std::env::var_os(MODEL_VAR);
        unsafe { std::env::set_var(MODEL_VAR, "shell-model") };

        let report = effective_model(None);

        assert_eq!(report.source, "environment");
        assert_eq!(report.model.as_deref(), Some("shell-model"));
        let conflict = report
            .overrides
            .iter()
            .find(|o| o.source == "environment")
            .expect("the environment conflict must be reported");
        assert!(
            !conflict.changeable,
            "Bridge must not edit the user's shell"
        );
        assert!(conflict.hint.contains(MODEL_VAR));
        // And the description must name what is winning, not claim success.
        assert!(report.description.contains("shell-model"));

        match previous {
            Some(value) => unsafe { std::env::set_var(MODEL_VAR, value) },
            None => unsafe { std::env::remove_var(MODEL_VAR) },
        }
    }

    #[test]
    fn a_project_settings_override_is_detected_and_reported_as_changeable() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (scratch_dir, _backups, settings) = scratch();
        let previous = std::env::var_os(MODEL_VAR);
        unsafe { std::env::remove_var(MODEL_VAR) };
        std::fs::write(&settings, r#"{ "model": "bridge-model" }"#).unwrap();

        // A project the user launches from, with its own settings file.
        let project = scratch_dir._dir.path().join("project");
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::write(
            project.join(".claude").join("settings.json"),
            r#"{ "model": "project-model" }"#,
        )
        .unwrap();

        let report = effective_model(Some(&project));

        assert_eq!(report.source, "project");
        assert_eq!(report.model.as_deref(), Some("project-model"));
        let conflict = report
            .overrides
            .iter()
            .find(|o| o.source == "project")
            .expect("the project conflict must be reported");
        assert!(
            conflict.changeable,
            "the user can edit their own project file"
        );
        assert!(conflict.path.is_some(), "the file must be named");

        if let Some(value) = previous {
            unsafe { std::env::set_var(MODEL_VAR, value) };
        }
    }

    #[test]
    fn project_local_settings_are_detected_too() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (scratch_dir, _backups, settings) = scratch();
        let previous = std::env::var_os(MODEL_VAR);
        unsafe { std::env::remove_var(MODEL_VAR) };
        std::fs::write(&settings, r#"{ "model": "bridge-model" }"#).unwrap();

        let project = scratch_dir._dir.path().join("project");
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::write(
            project.join(".claude").join("settings.local.json"),
            r#"{ "model": "local-model" }"#,
        )
        .unwrap();

        let report = effective_model(Some(&project));
        assert_eq!(report.source, "local");
        assert_eq!(report.model.as_deref(), Some("local-model"));

        if let Some(value) = previous {
            unsafe { std::env::set_var(MODEL_VAR, value) };
        }
    }

    #[test]
    fn a_project_without_a_model_does_not_report_a_conflict() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (scratch_dir, _backups, settings) = scratch();
        let previous = std::env::var_os(MODEL_VAR);
        unsafe { std::env::remove_var(MODEL_VAR) };
        std::fs::write(&settings, r#"{ "model": "bridge-model" }"#).unwrap();

        let project = scratch_dir._dir.path().join("project");
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        // A project file that sets something else entirely.
        std::fs::write(
            project.join(".claude").join("settings.json"),
            r#"{ "theme": "dark" }"#,
        )
        .unwrap();

        let report = effective_model(Some(&project));
        assert_eq!(report.source, "bridge");
        assert!(report.overrides.is_empty());

        if let Some(value) = previous {
            unsafe { std::env::set_var(MODEL_VAR, value) };
        }
    }

    #[test]
    fn a_default_model_variable_is_reported_when_nothing_else_selects_one() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, _backups, settings) = scratch();
        let previous = std::env::var_os(MODEL_VAR);
        let previous_default = std::env::var_os(DEFAULT_MODEL_VAR);
        unsafe { std::env::remove_var(MODEL_VAR) };
        unsafe { std::env::remove_var(DEFAULT_MODEL_VAR) };
        std::fs::write(
            &settings,
            r#"{ "env": { "ANTHROPIC_DEFAULT_MODEL": "from-variable" } }"#,
        )
        .unwrap();

        let report = effective_model(None);
        assert_eq!(report.source, "defaultModelVariable");
        assert_eq!(report.model.as_deref(), Some("from-variable"));

        if let Some(value) = previous {
            unsafe { std::env::set_var(MODEL_VAR, value) };
        }
        if let Some(value) = previous_default {
            unsafe { std::env::set_var(DEFAULT_MODEL_VAR, value) };
        }
    }

    #[test]
    fn a_missing_settings_file_is_not_an_error_for_the_report() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, _backups, settings) = scratch();
        let previous = std::env::var_os(MODEL_VAR);
        unsafe { std::env::remove_var(MODEL_VAR) };
        assert!(!settings.exists());

        let report = effective_model(None);
        assert_eq!(report.source, "account");

        if let Some(value) = previous {
            unsafe { std::env::set_var(MODEL_VAR, value) };
        }
    }

    #[test]
    fn an_unparsable_settings_file_does_not_panic_the_report() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, _backups, settings) = scratch();
        let previous = std::env::var_os(MODEL_VAR);
        unsafe { std::env::remove_var(MODEL_VAR) };
        std::fs::write(&settings, "{ this is not json").unwrap();

        // Reporting must degrade rather than fail: the user still needs to know
        // that no model is configured.
        let report = effective_model(None);
        assert_eq!(report.source, "account");

        if let Some(value) = previous {
            unsafe { std::env::set_var(MODEL_VAR, value) };
        }
    }

    #[test]
    fn a_blank_environment_variable_is_ignored() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, _backups, settings) = scratch();
        let previous = std::env::var_os(MODEL_VAR);
        unsafe { std::env::set_var(MODEL_VAR, "   ") };
        std::fs::write(&settings, r#"{ "model": "bridge-model" }"#).unwrap();

        let report = effective_model(None);
        assert_eq!(
            report.source, "bridge",
            "whitespace is not a model selection"
        );

        match previous {
            Some(value) => unsafe { std::env::set_var(MODEL_VAR, value) },
            None => unsafe { std::env::remove_var(MODEL_VAR) },
        }
    }

    #[test]
    fn activation_removes_a_legacy_anthropic_model_variable() {
        // Earlier Bridge versions wrote ANTHROPIC_MODEL, which outranks the
        // `model` key. If it were left behind, the key written by activation
        // would be silently ignored.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        std::fs::write(
            &settings,
            r#"{ "env": { "ANTHROPIC_MODEL": "an-old-bridge-model" } }"#,
        )
        .unwrap();

        let adapter = ClaudeAdapter::new(backups);
        adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-test")))
            .expect("apply must succeed");

        let doc = ClaudeJson::parse(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            doc.env_get(MODEL_VAR),
            None,
            "ANTHROPIC_MODEL would override the model key"
        );
        assert_eq!(doc.get_str(MODEL_SETTINGS_KEY), Some("deepseek-chat"));
    }

    #[test]
    fn activation_preserves_unrelated_settings_in_the_leftover_fixture() {
        // PRD §17: everything not owned by Bridge survives, including a
        // `modelSettings` block and a theme.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        std::fs::write(&settings, LEFTOVER_ALIAS_SETTINGS).unwrap();

        let adapter = ClaudeAdapter::new(backups);
        adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-test")))
            .expect("apply must succeed");

        let written = std::fs::read_to_string(&settings).unwrap();
        assert!(written.contains("modelSettings"), "{written}");
        assert!(written.contains("effortLevel"), "{written}");
        assert!(written.contains("\"theme\": \"dark\""), "{written}");
        assert!(written.contains("hasCompletedOnboarding"), "{written}");
    }

    #[test]
    fn the_model_key_is_selected_even_when_the_profile_has_no_alias() {
        // The precise defect: activation used to write ANTHROPIC_MODEL only when
        // the profile pinned an alias different from its model, so a profile
        // with no alias selected no model at all.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        std::fs::write(&settings, "{}\n").unwrap();

        let mut profile = profile_for_claude();
        assert!(profile.advanced.model_alias.is_none());
        profile.model = "no-alias-model".into();

        let adapter = ClaudeAdapter::new(backups);
        adapter
            .apply_profile(&profile, &options(Some("sk-test")))
            .expect("apply must succeed");

        assert_eq!(
            ClaudeJson::parse(&std::fs::read_to_string(&settings).unwrap())
                .unwrap()
                .get_str(MODEL_SETTINGS_KEY),
            Some("no-alias-model")
        );
    }

    #[test]
    fn applying_to_an_invalid_settings_file_is_a_hard_stop() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        let broken = "{ \"env\": { \"A\": }, }\n";
        std::fs::write(&settings, broken).unwrap();

        let adapter = ClaudeAdapter::new(backups.clone());
        let err = adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-test")))
            .expect_err("invalid JSON must block the write");

        assert_eq!(err.kind, crate::error::ErrorKind::ConfigInvalid, "{err:?}");
        assert_eq!(
            std::fs::read_to_string(&settings).unwrap(),
            broken,
            "the file must be untouched"
        );
        assert!(!backups.has_original(AgentId::Claude.as_str()));
    }

    #[test]
    fn an_external_change_aborts_the_claude_write() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        std::fs::write(&settings, "{}\n").unwrap();

        let adapter = ClaudeAdapter::new(backups);
        let before = adapter.current_hash().unwrap().unwrap();
        let external = "{ \"edited\": true }\n";
        std::fs::write(&settings, external).unwrap();

        let stale = ApplyOptions {
            secret: Some("sk-test"),
            helper_path: None,
            expected_sha256: Some(before),
            agent_version: None,
        };
        let err = adapter
            .apply_profile(&profile_for_claude(), &stale)
            .expect_err("an external edit must abort");

        assert_eq!(err.kind, crate::error::ErrorKind::ConfigConflict, "{err:?}");
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), external);
    }

    #[test]
    fn restore_default_removes_only_bridge_env_entries() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        std::fs::write(
            &settings,
            "{\n  \"env\": { \"MY_FLAG\": \"1\" },\n  \"permissions\": { \"allow\": [] }\n}\n",
        )
        .unwrap();

        let adapter = ClaudeAdapter::new(backups);
        adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-test")))
            .expect("apply must succeed");
        adapter
            .restore_default(&options(None))
            .expect("restore default must succeed");

        let restored = std::fs::read_to_string(&settings).unwrap();
        assert!(!restored.contains(BASE_URL_VAR), "{restored}");
        assert!(!restored.contains(AUTH_TOKEN_VAR), "{restored}");
        assert!(!restored.contains(MODEL_VAR), "{restored}");
        // The user's own env flag and unrelated settings survive (PRD §30.2).
        assert!(restored.contains("\"MY_FLAG\""), "{restored}");
        assert!(restored.contains("\"permissions\""), "{restored}");
        // No empty env object is left behind.
        assert!(!restored.contains("\"env\": {}"), "{restored}");
    }

    #[test]
    fn restore_default_preserves_mcp_and_session_state() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (scratch, backups, settings) = scratch();
        // Claude Code keeps sign-in and MCP state alongside settings.
        let creds = scratch._dir.path().join(".credentials.json");
        std::fs::write(&creds, r#"{"token":"session"}"#).unwrap();
        let mcp = scratch._dir.path().join("mcp.json");
        std::fs::write(&mcp, r#"{"servers":{"x":{}}}"#).unwrap();
        std::fs::write(
            &settings,
            "{\n  \"enabledPlugins\": { \"foo@bar\": true }\n}\n",
        )
        .unwrap();

        let adapter = ClaudeAdapter::new(backups);
        adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-test")))
            .expect("apply must succeed");
        adapter
            .restore_default(&options(None))
            .expect("restore default must succeed");

        // PRD §30.2: never log out, never delete the sign-in session, never
        // delete MCP state.
        assert!(creds.exists(), "the sign-in session must not be deleted");
        assert!(mcp.exists(), "MCP state must not be deleted");
        assert!(std::fs::read_to_string(&settings)
            .unwrap()
            .contains("enabledPlugins"));
    }

    #[test]
    fn restore_original_restores_the_pre_bridge_settings_byte_for_byte() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        let original = "{\n  \"statusLine\": { \"type\": \"command\" }\n}\n";
        std::fs::write(&settings, original).unwrap();

        let adapter = ClaudeAdapter::new(backups);
        adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-test")))
            .expect("apply must succeed");
        adapter
            .restore_original(&options(None))
            .expect("restore original must succeed");

        assert_eq!(std::fs::read_to_string(&settings).unwrap(), original);
    }

    #[test]
    fn undo_returns_the_claude_settings_to_their_previous_state() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        let original = "{\n  \"zzz\": 1\n}\n";
        std::fs::write(&settings, original).unwrap();

        let adapter = ClaudeAdapter::new(backups.clone());
        adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-test")))
            .expect("apply must succeed");

        let snapshot = backups
            .snapshots()
            .latest(AgentId::Claude.as_str())
            .unwrap()
            .expect("a snapshot exists");
        adapter
            .restore_snapshot(&snapshot, &options(None))
            .expect("restore snapshot must succeed");

        assert_eq!(std::fs::read_to_string(&settings).unwrap(), original);
        // And the restore was itself snapshotted, so it can be undone.
        assert!(backups.snapshot_count(AgentId::Claude.as_str()) >= 2);
    }

    #[test]
    fn a_missing_settings_file_is_created_without_a_backup_of_nothing() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        assert!(!settings.exists());

        let adapter = ClaudeAdapter::new(backups.clone());
        adapter
            .apply_profile(&profile_for_claude(), &options(Some("sk-test")))
            .expect("apply must succeed");
        assert!(settings.exists(), "the settings file must be created");

        // The original recorded that no file existed, so restoring it removes
        // the file rather than leaving an empty one behind.
        let (meta, content) = backups
            .read_original(AgentId::Claude.as_str())
            .unwrap()
            .expect("an original is recorded even when the file was absent");
        assert!(
            !meta.existed,
            "the original must record the file was absent"
        );
        assert!(content.is_empty(), "{content:?}");
    }

    /// Every file under `root`, as (path, contents) pairs.
    fn all_files_under(root: &std::path::Path) -> Vec<(String, String)> {
        let mut found = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(text) = std::fs::read_to_string(&path) {
                    found.push((path.display().to_string(), text));
                }
            }
        }
        found
    }

    #[test]
    fn the_api_key_never_ends_up_in_the_backup_tree() {
        // Claude Code has no credential-helper mechanism, so the gateway key is
        // written into settings.json by design. That file is permission
        // hardened, but the backup tree is a *second* copy, in a directory the
        // app owns and enumerates.
        //
        // The original backup is taken before the first write, so it is clean.
        // The danger is the *second* mutation: the snapshot taken before it
        // copies a settings.json that already contains the key.
        //
        // This is the case the backup module doc used to wave away with
        // "secrets never live in those files" - true for Codex, false for Claude.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_scratch, backups, settings) = scratch();
        std::fs::write(&settings, "{\n  \"zzz\": 1\n}\n").unwrap();

        let adapter = ClaudeAdapter::new(backups.clone());
        // First activation writes the key.
        adapter
            .apply_profile(
                &profile_for_claude(),
                &options(Some("sk-ant-leak-canary-0001")),
            )
            .expect("first apply must succeed");
        // Second activation snapshots the file that now holds it.
        adapter
            .apply_profile(
                &profile_for_claude(),
                &options(Some("sk-ant-leak-canary-0002")),
            )
            .expect("second apply must succeed");

        let offenders: Vec<String> = all_files_under(backups.root())
            .into_iter()
            .filter(|(_, text)| {
                text.contains("sk-ant-leak-canary-0001") || text.contains("sk-ant-leak-canary-0002")
            })
            .map(|(path, _)| path)
            .collect();

        assert!(
            offenders.is_empty(),
            "the API key was copied into the backup tree at: {}",
            offenders.join(", ")
        );
    }
}
