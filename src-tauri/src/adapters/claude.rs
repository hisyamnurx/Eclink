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

/// Model variable, written only when the profile pins a model alias.
pub const MODEL_VAR: &str = "ANTHROPIC_MODEL";

/// Every `env` key Bridge may write, used for cleanup and ownership reporting.
pub const MANAGED_ENV_VARS: [&str; 3] = [BASE_URL_VAR, AUTH_TOKEN_VAR, MODEL_VAR];

/// Organization-managed settings locations, checked in precedence order.
///
/// The Windows machine-wide path comes first; the macOS path is retained so a
/// shared code path never silently skips the policy check.
const MANAGED_SETTINGS_PATHS: [&str; 2] = [
    r"C:\ProgramData\ClaudeCode\managed-settings.json",
    "/Library/Application Support/ClaudeCode/managed-settings.json",
];

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

    /// Remove every Bridge-written `env` entry.
    ///
    /// Only the variables Bridge itself writes are removed. Any other `env`
    /// entry the user has (their own flags, proxy settings, telemetry switches)
    /// is preserved, and an `env` object left empty is dropped entirely rather
    /// than left behind as `{}` (PRD §17, §18.3).
    fn strip_managed(doc: &mut ClaudeJson) -> Vec<String> {
        let mut removed = Vec::new();
        for key in MANAGED_ENV_VARS {
            if doc.env_remove(key) {
                removed.push(format!("env.{key}"));
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

        let Ok(doc) = ClaudeJson::parse(&raw) else {
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

        // `allowedProviders` restricts which provider kinds may be used.
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
                    BridgeError::managed_policy("Managed configuration detected.").with_hint(
                        format!(
                            "Your organization's policy at {} does not allow a custom endpoint, so Eclyzer Bridge cannot apply a profile.",
                            path.display()
                        ),
                    ),
                );
            }
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
        let has_alias = profile.advanced.model_alias.is_some();

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

            // Only pin a model when the profile asked for one; leaving it unset
            // lets Claude Code use its own default model.
            if has_alias {
                doc.env_set(MODEL_VAR, &wire_model);
                managed.push(format!("env.{MODEL_VAR}"));
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
    fn managed_settings_are_detected_by_env_block() {
        // Mirrors the shape of an organization-managed file.
        let managed = r#"{ "env": { "ANTHROPIC_BASE_URL": "https://corp.example.com" } }"#;
        let doc = ClaudeJson::parse(managed).unwrap();
        assert!(doc.env_get(BASE_URL_VAR).is_some());
    }

    #[test]
    fn allowed_providers_without_custom_endpoint_blocks_activation() {
        let doc = ClaudeJson::parse(r#"{ "allowedProviders": ["anthropic"] }"#).unwrap();
        let permits_custom = doc
            .root()
            .get("allowedProviders")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .any(|name| name.eq_ignore_ascii_case("customEndpoint"))
            })
            .unwrap_or(false);
        assert!(!permits_custom);
    }

    #[test]
    fn allowed_providers_with_custom_endpoint_permits_activation() {
        let doc = ClaudeJson::parse(r#"{ "allowedProviders": ["customEndpoint"] }"#).unwrap();
        let permits_custom = doc
            .root()
            .get("allowedProviders")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .any(|name| name.eq_ignore_ascii_case("customEndpoint"))
            })
            .unwrap_or(false);
        assert!(permits_custom);
    }

    #[test]
    fn a_codex_only_profile_is_not_targeted_here() {
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
        assert!(!p.targets_agent(AgentId::Claude));
    }

    #[test]
    fn managed_env_vars_cover_every_key_bridge_writes() {
        // Keeps cleanup in sync with what apply writes; a new variable added to
        // apply without being listed here would be left behind on restore.
        assert!(MANAGED_ENV_VARS.contains(&BASE_URL_VAR));
        assert!(MANAGED_ENV_VARS.contains(&AUTH_TOKEN_VAR));
        assert!(MANAGED_ENV_VARS.contains(&MODEL_VAR));
    }
}
