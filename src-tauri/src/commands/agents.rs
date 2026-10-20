//! Agent detection and status commands.

use serde::Serialize;

use crate::adapters::{AgentDetection, AgentDiagnostics, AgentId, ConfigParseState};
use crate::app::AppState;
use crate::commands::{parse_agent, CommandResult};
use crate::error::{BridgeError, BridgeResult};

/// One agent as the home screen needs it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummary {
    pub agent: AgentId,
    pub display_name: String,
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    pub config_state: ConfigParseState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_guide_url: Option<String>,
    /// What the agent is actually configured to use right now.
    pub active_label: String,
    /// `default`, `managed`, or `external`.
    pub active_kind: String,
    /// Profile name, when Bridge recognizes the active configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_name: Option<String>,
    /// Whether the configuration file can be modified safely.
    pub config_writable: bool,
}

/// Build a summary from a detection plus its config state.
fn summarise(
    detection: AgentDetection,
    state: ConfigParseState,
    active: Option<crate::adapters::ActiveConfiguration>,
) -> AgentSummary {
    AgentSummary {
        agent: detection.agent,
        display_name: detection.display_name,
        installed: detection.installed,
        version: detection.version,
        executable: detection.executable,
        config_path: detection.config_path,
        config_state: state,
        note: detection.note,
        install_guide_url: detection.install_guide_url,
        active_label: active
            .as_ref()
            .map(|a| a.label.clone())
            .unwrap_or_else(|| "Unknown".to_string()),
        active_kind: active
            .as_ref()
            .map(|a| match a.kind {
                crate::adapters::ActiveConfigurationKind::Default => "default",
                crate::adapters::ActiveConfigurationKind::Managed => "managed",
                crate::adapters::ActiveConfigurationKind::External => "external",
            })
            .unwrap_or("unknown")
            .to_string(),
        active_profile_name: active.as_ref().and_then(|a| a.profile_name.clone()),
        config_writable: state != ConfigParseState::Invalid,
    }
}

/// Detect every supported agent. Used at startup and by the Refresh button.
#[tauri::command]
pub async fn list_agents(state: tauri::State<'_, AppState>) -> CommandResult<Vec<AgentSummary>> {
    let state = state.inner().clone();
    let summaries = tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        for adapter in state.adapters() {
            let detection = match adapter.detect() {
                Ok(detection) => detection,
                Err(err) => {
                    // A detection failure for one agent must not blank the other.
                    tracing::warn!(
                        target: "eclink",
                        agent = adapter.id().as_str(),
                        error = %err,
                        "agent detection failed"
                    );
                    AgentDetection::not_installed(adapter.id(), "")
                }
            };
            let snapshot = adapter.read_config().ok();
            let active = if detection.installed {
                adapter.get_active_profile().ok()
            } else {
                None
            };
            out.push(summarise(
                detection,
                snapshot
                    .as_ref()
                    .map(|s| s.state)
                    .unwrap_or(ConfigParseState::Missing),
                active,
            ));
        }
        out
    })
    .await
    .map_err(|err| {
        crate::commands::CommandError::from(
            BridgeError::internal("Could not check for installed coding agents.")
                .with_details(err.to_string()),
        )
    })?;

    Ok(summaries)
}

/// Re-detect a single agent.
#[tauri::command]
pub async fn refresh_agent(
    state: tauri::State<'_, AppState>,
    agent: String,
) -> CommandResult<AgentSummary> {
    let state = state.inner().clone();
    let agent = parse_agent(&agent)?;
    tauri::async_runtime::spawn_blocking(move || -> BridgeResult<AgentSummary> {
        let adapter = state.adapter(agent);
        let detection = adapter.detect()?;
        let snapshot = adapter.read_config().ok();
        let active = if detection.installed {
            adapter.get_active_profile().ok()
        } else {
            None
        };
        Ok(summarise(
            detection,
            snapshot
                .as_ref()
                .map(|s| s.state)
                .unwrap_or(ConfigParseState::Missing),
            active,
        ))
    })
    .await
    .map_err(|err| {
        crate::commands::CommandError::from(
            BridgeError::internal("Could not refresh this agent.").with_details(err.to_string()),
        )
    })?
    .map_err(Into::into)
}

/// Detailed per-agent diagnostics for the Settings screen.
#[tauri::command]
pub async fn get_diagnostics(
    state: tauri::State<'_, AppState>,
) -> CommandResult<Vec<AgentDiagnostics>> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        for adapter in state.adapters() {
            match adapter.diagnostics() {
                Ok(diagnostics) => out.push(diagnostics),
                Err(err) => {
                    tracing::warn!(
                        target: "eclink",
                        agent = adapter.id().as_str(),
                        error = %err,
                        "diagnostics failed"
                    );
                }
            }
        }
        out
    })
    .await
    .map_err(|err| {
        crate::commands::CommandError::from(
            BridgeError::internal("Could not collect diagnostics.").with_details(err.to_string()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::ActiveConfiguration;

    #[test]
    fn a_managed_configuration_is_reported_as_managed() {
        let mut detection = AgentDetection::not_installed(AgentId::Codex, "");
        detection.installed = true;
        let active = ActiveConfiguration {
            kind: crate::adapters::ActiveConfigurationKind::Managed,
            profile_id: Some("p".into()),
            profile_name: Some("DeepSeek".into()),
            label: "DeepSeek".into(),
            provider_id: Some("eclyzer_bridge_deepseek_1".into()),
            model: Some("deepseek-chat".into()),
        };
        let summary = summarise(detection, ConfigParseState::Valid, Some(active));
        assert_eq!(summary.active_kind, "managed");
        assert_eq!(summary.active_label, "DeepSeek");
        assert_eq!(summary.active_profile_name.as_deref(), Some("DeepSeek"));
    }

    #[test]
    fn an_unrecognized_configuration_is_reported_as_external() {
        // Never claim "Default" when the user has their own custom setup.
        let mut detection = AgentDetection::not_installed(AgentId::Codex, "");
        detection.installed = true;
        let active = ActiveConfiguration::external(Some("my_gw".into()), Some("m".into()));
        let summary = summarise(detection, ConfigParseState::Valid, Some(active));
        assert_eq!(summary.active_kind, "external");
        assert_eq!(summary.active_label, "External Configuration");
        assert!(summary.active_profile_name.is_none());
    }

    #[test]
    fn an_invalid_config_is_not_writable() {
        let mut detection = AgentDetection::not_installed(AgentId::Codex, "");
        detection.installed = true;
        let summary = summarise(detection, ConfigParseState::Invalid, None);
        assert!(!summary.config_writable);
        assert_eq!(summary.active_label, "Unknown");
    }

    #[test]
    fn a_missing_config_is_writable() {
        let mut detection = AgentDetection::not_installed(AgentId::Codex, "");
        detection.installed = true;
        let summary = summarise(detection, ConfigParseState::Missing, None);
        assert!(summary.config_writable);
    }
}
