//! Agent adapter interface (PRD §15).
//!
//! Core code must not contain Codex-specific or Claude-specific logic. Every
//! agent implements [`AgentAdapter`]; adding a third agent means adding one
//! module here and one registration line, never touching the core.
//!
//! The trait deliberately mirrors the required conceptual contract:
//! `detect`, `get_version`, `locate_config`, `read_config`, `validate_config`,
//! `backup_config`, `get_active_profile`, `check_profile_compatibility`,
//! `apply_profile`, `restore_snapshot`, `restore_original`, `restore_default`,
//! `launch`, `diagnostics`.

pub mod claude;
pub mod detect;
pub mod codex;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backup::Snapshot;
use crate::error::BridgeResult;

// Re-exported so sibling modules (`process`, `app`, `commands`) can name these
// without reaching into `profiles` directly.
pub use crate::profiles::{AgentId, CompatibilityStatus, Profile};
pub use claude::ClaudeAdapter;
pub use codex::CodexAdapter;

/// Directory name under `%LOCALAPPDATA%` for all Eclyzer Bridge data.
pub const APP_DATA_DIR_NAME: &str = "EclyzerBridge";

/// Result of probing for an installed agent (PRD §14).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDetection {
    pub agent: AgentId,
    pub display_name: String,
    pub installed: bool,
    /// Executable path resolved from PATH or a known location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    /// Version string as reported by the agent itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Effective config file path, honouring the agent's env override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    /// Effective config directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
    /// Whether the config directory came from an environment override.
    #[serde(default)]
    pub config_dir_from_env: bool,
    /// Short, actionable note, e.g. version too old for a feature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Official installation guide, offered only as a link (PRD §33).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_guide_url: Option<String>,
}

impl AgentDetection {
    /// A "not installed" result with the guide link attached.
    pub fn not_installed(agent: AgentId, guide: &str) -> Self {
        Self {
            agent,
            display_name: agent.display_name().to_string(),
            installed: false,
            executable: None,
            version: None,
            config_path: None,
            config_dir: None,
            config_dir_from_env: false,
            note: None,
            install_guide_url: Some(guide.to_string()),
        }
    }
}

/// Parse state of the agent's configuration file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigParseState {
    /// No file exists yet; Bridge will create one on first apply.
    Missing,
    /// Exists and parses cleanly.
    Valid,
    /// Exists but cannot be parsed; Bridge refuses to modify it.
    Invalid,
}

/// Raw + parsed view of an agent configuration file.
#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    pub path: PathBuf,
    pub raw: Option<String>,
    pub sha256: Option<String>,
    pub state: ConfigParseState,
    /// Parse error detail (already redacted) when `state == Invalid`.
    pub parse_error: Option<String>,
}

/// Which provider/model an agent is currently configured to use (PRD §49).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveConfiguration {
    /// `"default"`, `"external"`, or `"managed"`.
    pub kind: ActiveConfigurationKind,
    /// Profile name when Bridge recognizes the active config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    /// Human label for the UI, e.g. "OpenAI Default" or "DeepSeek Flash".
    pub label: String,
    /// Provider id present in the agent config, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Model id present in the agent config, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveConfigurationKind {
    /// No Bridge-managed override and no obvious user override.
    Default,
    /// Bridge recognizes its own managed provider as active.
    Managed,
    /// The user has a custom provider Bridge does not manage.
    External,
}

impl ActiveConfiguration {
    pub fn managed(profile: &Profile, provider_id: String, model: String) -> Self {
        Self {
            kind: ActiveConfigurationKind::Managed,
            profile_id: Some(profile.id.clone()),
            profile_name: Some(profile.name.clone()),
            label: profile.name.clone(),
            provider_id: Some(provider_id),
            model: Some(model),
        }
    }

    pub fn default_config(label: &str) -> Self {
        Self {
            kind: ActiveConfigurationKind::Default,
            profile_id: None,
            profile_name: None,
            label: label.to_string(),
            provider_id: None,
            model: None,
        }
    }

    pub fn external(provider_id: Option<String>, model: Option<String>) -> Self {
        Self {
            kind: ActiveConfigurationKind::External,
            profile_id: None,
            profile_name: None,
            label: "External Configuration".to_string(),
            provider_id,
            model,
        }
    }
}

/// What an apply operation intends to do. Used for snapshot reasons and the
/// history feed (PRD §27).
#[derive(Debug, Clone)]
pub enum MutationReason {
    ActivateProfile { profile_id: String, profile_name: String },
    RestoreDefault,
    RestoreOriginal,
    RestoreSnapshot { snapshot_id: String },
    UndoLastChange,
}

impl MutationReason {
    /// Stable machine code stored in snapshot metadata.
    pub fn code(&self) -> &'static str {
        match self {
            MutationReason::ActivateProfile { .. } => "activate_profile",
            MutationReason::RestoreDefault => "restore_default",
            MutationReason::RestoreOriginal => "restore_original",
            MutationReason::RestoreSnapshot { .. } => "restore_snapshot",
            MutationReason::UndoLastChange => "undo",
        }
    }

    /// Human sentence for the history list.
    pub fn label(&self) -> String {
        match self {
            MutationReason::ActivateProfile { profile_name, .. } => {
                format!("Activated {profile_name}")
            }
            MutationReason::RestoreDefault => "Restored default configuration".to_string(),
            MutationReason::RestoreOriginal => "Restored original configuration".to_string(),
            MutationReason::RestoreSnapshot { .. } => "Restored a snapshot".to_string(),
            MutationReason::UndoLastChange => "Undid last change".to_string(),
        }
    }

    pub fn profile_id(&self) -> Option<&str> {
        match self {
            MutationReason::ActivateProfile { profile_id, .. } => Some(profile_id),
            _ => None,
        }
    }
}

/// Outcome of a successful apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutcome {
    pub agent: AgentId,
    pub config_path: String,
    /// Snapshot created immediately before the mutation.
    pub snapshot_id: String,
    /// Whether this was the first mutation, which also creates the immutable
    /// original backup (PRD §25).
    pub created_original_backup: bool,
    /// Keys Bridge now owns for this agent.
    pub managed_keys: Vec<String>,
    pub active: ActiveConfiguration,
}

/// Diagnostics surfaced in Settings and the support bundle (PRD §38).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDiagnostics {
    pub agent: AgentId,
    pub display_name: String,
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    pub config_state: ConfigParseState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_id: Option<String>,
    /// Paths Bridge has backed up for this agent.
    pub backups_present: bool,
    pub original_backup_present: bool,
    pub snapshot_count: usize,
}

/// Options controlling how an apply writes the agent config.
#[derive(Debug, Clone)]
pub struct ApplyOptions<'a> {
    /// Resolved API key for the profile, injected only where the agent has no
    /// credential-helper mechanism (PRD §15, §32.3).
    pub secret: Option<&'a str>,
    /// Path to `eclyzer-bridge-helper.exe`, when bundled.
    pub helper_path: Option<PathBuf>,
    /// Hash the caller last observed for the config file. A mismatch aborts the
    /// write so external edits are never clobbered (PRD §31.3, §48).
    pub expected_sha256: Option<String>,
    /// Resolved agent version, used for version gates.
    pub agent_version: Option<String>,
}

/// The contract every coding agent integration implements.
pub trait AgentAdapter: Send + Sync {
    fn id(&self) -> AgentId;

    // ---- detection -------------------------------------------------------

    /// Probe for the agent without mutating anything (PRD §14).
    fn detect(&self) -> BridgeResult<AgentDetection>;

    /// Effective config file path, honouring the agent's env override.
    fn locate_config(&self) -> BridgeResult<PathBuf>;

    // ---- reading ---------------------------------------------------------

    /// Read the config file plus its parse state.
    fn read_config(&self) -> BridgeResult<ConfigSnapshot>;

    /// Validate that the config can be safely modified.
    ///
    /// Returns `Err` with actionable copy when it cannot.
    fn validate_config(&self, snapshot: &ConfigSnapshot) -> BridgeResult<()>;

    /// Report which provider/model the agent currently uses (PRD §49).
    fn get_active_profile(&self) -> BridgeResult<ActiveConfiguration>;

    // ---- compatibility ---------------------------------------------------

    /// Decide whether `profile` can be applied for this agent.
    ///
    /// This is a cached-verdict lookup plus static checks; the real protocol
    /// test lives in [`crate::testing`].
    fn check_profile_compatibility(&self, profile: &Profile) -> CompatibilityStatus;

    /// Whether organization-managed policy blocks Bridge from applying
    /// (PRD §17.2, §18). `Err` carries the managed-policy message.
    fn check_managed_policy(&self) -> BridgeResult<()>;

    // ---- mutation --------------------------------------------------------

    /// Apply `profile` using the safe mutation sequence (PRD §31).
    fn apply_profile(
        &self,
        profile: &Profile,
        options: &ApplyOptions<'_>,
    ) -> BridgeResult<ApplyOutcome>;

    /// Remove every Bridge-owned setting, returning the agent to its own
    /// default provider while preserving login and unrelated settings
    /// (PRD §17, §30).
    fn restore_default(&self, options: &ApplyOptions<'_>) -> BridgeResult<ApplyOutcome>;

    /// Restore the immutable pre-Bridge original config (PRD §29).
    fn restore_original(&self, options: &ApplyOptions<'_>) -> BridgeResult<ApplyOutcome>;

    /// Restore a specific snapshot (PRD §26).
    fn restore_snapshot(
        &self,
        snapshot: &Snapshot,
        options: &ApplyOptions<'_>,
    ) -> BridgeResult<ApplyOutcome>;

    // ---- launch ----------------------------------------------------------

    /// Build the argv used to launch the agent in `project_dir`.
    ///
    /// Returning argv (never a joined shell string) is what keeps model IDs and
    /// provider names from becoming commands (PRD §63).
    fn launch_argv(&self, project_dir: &Path) -> BridgeResult<LaunchPlan>;

    /// Environment overlay for the launched process tree only. Never persisted
    /// to the global environment (PRD §32.3).
    fn launch_env(&self, secret: Option<&str>) -> BridgeResult<Vec<(String, String)>>;

    // ---- diagnostics -----------------------------------------------------

    fn diagnostics(&self) -> BridgeResult<AgentDiagnostics>;
}

/// A prepared launch, expressed as argv so no shell interpretation occurs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPlan {
    pub agent: AgentId,
    /// Terminal host executable (Windows Terminal or PowerShell).
    pub program: String,
    /// Full argv: terminal flags, then the agent path, then agent args.
    pub args: Vec<String>,
    /// Working directory for the terminal.
    pub working_dir: String,
    /// Human-readable command preview shown in the UI.
    pub display: String,
}

/// Environment variables injected for the launched agent only.
#[derive(Debug, Clone, Default)]
pub struct EnvOverlay(pub Vec<(String, String)>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_reason_codes_are_stable() {
        // History entries and snapshot metadata persist these codes; changing
        // them silently would break restore lookup.
        assert_eq!(
            MutationReason::ActivateProfile {
                profile_id: "p".into(),
                profile_name: "P".into()
            }
            .code(),
            "activate_profile"
        );
        assert_eq!(MutationReason::RestoreDefault.code(), "restore_default");
        assert_eq!(MutationReason::RestoreOriginal.code(), "restore_original");
        assert_eq!(MutationReason::UndoLastChange.code(), "undo");
    }

    #[test]
    fn mutation_reason_labels_are_human() {
        assert_eq!(
            MutationReason::ActivateProfile {
                profile_id: "p".into(),
                profile_name: "DeepSeek Flash".into()
            }
            .label(),
            "Activated DeepSeek Flash"
        );
    }

    #[test]
    fn active_configuration_kinds() {
        let external = ActiveConfiguration::external(Some("my_gw".into()), Some("m".into()));
        assert_eq!(external.kind, ActiveConfigurationKind::External);
        assert_eq!(external.label, "External Configuration");

        let default = ActiveConfiguration::default_config("OpenAI Default");
        assert_eq!(default.kind, ActiveConfigurationKind::Default);
    }

    #[test]
    fn not_installed_detection_carries_guide() {
        let detection = AgentDetection::not_installed(AgentId::Codex, "https://example.com/guide");
        assert!(!detection.installed);
        assert!(detection.install_guide_url.is_some());
        assert_eq!(detection.display_name, "Codex");
    }
}
