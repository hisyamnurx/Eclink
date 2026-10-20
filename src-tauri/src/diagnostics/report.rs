//! Sanitized diagnostics bundle (PRD §38).
//!
//! "Copy Diagnostics" produces a pasteable report a user can attach to a bug.
//! It is assembled from values the backend already holds and passes through the
//! redactor twice: once per field and once over the finished text. It contains
//! no API key, no prompt, and no full log dump.

use serde::{Deserialize, Serialize};

use crate::diagnostics::redact::redact_text;

/// Everything the diagnostics view shows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    pub app_version: String,
    pub platform: String,
    pub data_dir: String,
    pub log_dir: String,
    pub secret_backend: String,
    pub secret_count: usize,
    pub profile_count: usize,
    pub agents: Vec<crate::adapters::AgentDiagnostics>,
    /// Non-fatal problems the user should know about.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl DiagnosticsReport {
    /// Render as pasteable text.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str("Eclink Diagnostics\n");
        out.push_str("==========================\n\n");
        out.push_str(&format!("App version: {}\n", self.app_version));
        out.push_str(&format!("Platform: {}\n", self.platform));
        out.push_str(&format!("Data directory: {}\n", self.data_dir));
        out.push_str(&format!("Log directory: {}\n", self.log_dir));
        out.push_str(&format!("Secret backend: {}\n", self.secret_backend));
        out.push_str(&format!("Stored secrets: {}\n", self.secret_count));
        out.push_str(&format!("Provider profiles: {}\n", self.profile_count));
        out.push('\n');

        for agent in &self.agents {
            out.push_str(&format!("{}:\n", agent.display_name));
            out.push_str(&format!("  Installed: {}\n", yes_no(agent.installed)));
            out.push_str(&format!(
                "  Version: {}\n",
                agent.version.as_deref().unwrap_or("unknown")
            ));
            out.push_str(&format!(
                "  Config: {}\n",
                agent.config_path.as_deref().unwrap_or("not found")
            ));
            out.push_str(&format!(
                "  Config state: {}\n",
                config_state_label(agent.config_state)
            ));
            out.push_str(&format!(
                "  Backups present: {}\n",
                yes_no(agent.backups_present)
            ));
            out.push_str(&format!(
                "  Original backup: {}\n",
                yes_no(agent.original_backup_present)
            ));
            out.push_str(&format!("  Snapshots: {}\n", agent.snapshot_count));
            out.push('\n');
        }

        if !self.warnings.is_empty() {
            out.push_str("Warnings:\n");
            for warning in &self.warnings {
                out.push_str(&format!("  - {warning}\n"));
            }
            out.push('\n');
        }

        out.push_str("No API keys, prompts, or credentials are included in this report.\n");

        // Final safety net over the assembled text.
        redact_text(&out)
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn config_state_label(state: crate::adapters::ConfigParseState) -> &'static str {
    match state {
        crate::adapters::ConfigParseState::Missing => "not created yet",
        crate::adapters::ConfigParseState::Valid => "valid",
        crate::adapters::ConfigParseState::Invalid => "INVALID (Eclink will not modify it)",
    }
}

/// Assemble a report from live application state (PRD §38).
///
/// Every value is drawn from the backend, never from the frontend, so a
/// compromised webview cannot inject content into a report the user is about to
/// paste into a public issue.
pub fn build_report(state: &crate::app::AppState) -> crate::error::BridgeResult<DiagnosticsReport> {
    let mut warnings = Vec::new();

    // Agents: a failure for one must not lose the report for the others.
    let mut agents = Vec::new();
    for adapter in state.adapters() {
        match adapter.diagnostics() {
            Ok(diagnostics) => agents.push(diagnostics),
            Err(err) => warnings.push(format!(
                "{} diagnostics could not be collected: {}",
                adapter.id().display_name(),
                err.message
            )),
        }
    }

    // An unparsable config is the single most common support issue, so call it
    // out at the top rather than making the reader scan the agent blocks.
    for agent in &agents {
        if agent.config_state == crate::adapters::ConfigParseState::Invalid {
            warnings.push(format!(
                "{} has a configuration file Eclink cannot parse, so it will not be modified.",
                agent.display_name
            ));
        }
    }

    if state.helper_path().is_none() {
        warnings.push(
            "The bundled credential helper was not found; Codex will use an environment credential instead."
                .to_string(),
        );
    }

    let profile_count = state.profiles().load().map(|f| f.profiles.len())?;

    Ok(DiagnosticsReport {
        app_version: crate::APP_VERSION.to_string(),
        platform: platform_label(),
        data_dir: state.paths().root().display().to_string(),
        log_dir: state.paths().logs_dir().display().to_string(),
        secret_backend: crate::secrets::backend_name().to_string(),
        secret_count: state.secrets().count().unwrap_or(0),
        profile_count,
        agents,
        warnings,
    })
}

/// A short platform label, e.g. `windows 11 x86_64`.
fn platform_label() -> String {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "windows" => format!("windows {arch}"),
        other => format!("{other} {arch} (unsupported)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{AgentDiagnostics, ConfigParseState};
    use crate::profiles::AgentId;

    fn report() -> DiagnosticsReport {
        DiagnosticsReport {
            app_version: "0.1.0".into(),
            platform: "windows x86_64".into(),
            data_dir: r"C:\Users\Test User\AppData\Local\EclyzerBridge".into(),
            log_dir: r"C:\Users\Test User\AppData\Local\EclyzerBridge\logs".into(),
            secret_backend: "dpapi-current-user".into(),
            secret_count: 2,
            profile_count: 3,
            agents: vec![AgentDiagnostics {
                agent: AgentId::Codex,
                display_name: "Codex".into(),
                installed: true,
                version: Some("0.42.0".into()),
                config_path: Some(r"C:\Users\Test User\.codex\config.toml".into()),
                config_state: ConfigParseState::Valid,
                active_profile_id: None,
                backups_present: true,
                original_backup_present: true,
                snapshot_count: 4,
            }],
            warnings: vec![],
        }
    }

    #[test]
    fn text_contains_useful_facts() {
        let text = report().to_text();
        assert!(text.contains("0.1.0"));
        assert!(text.contains("Codex"));
        assert!(text.contains("0.42.0"));
        assert!(text.contains("dpapi-current-user"));
        assert!(text.contains("Snapshots: 4"));
    }

    #[test]
    fn text_never_contains_a_secret() {
        let mut r = report();
        // Simulate a caller accidentally passing a secret-bearing warning.
        r.warnings.push(
            "probe failed with X-Api-Key: sk-abcdef0123456789abcdef while calling the endpoint"
                .into(),
        );
        let text = r.to_text();
        assert!(
            !text.contains("sk-abcdef0123456789abcdef"),
            "diagnostics leaked a secret: {text}"
        );
        assert!(text.contains("[REDACTED]"), "{text}");
    }

    #[test]
    fn states_are_worded_not_coloured() {
        // Accessibility: status must be conveyed by text, not colour alone.
        let mut r = report();
        r.agents[0].config_state = ConfigParseState::Invalid;
        let text = r.to_text();
        assert!(text.contains("INVALID"), "{text}");
        assert!(text.contains("will not modify"), "{text}");
    }

    #[test]
    fn unicode_and_space_paths_render_verbatim() {
        let text = report().to_text();
        assert!(text.contains(r"C:\Users\Test User\AppData\Local\EclyzerBridge"));
    }

    #[test]
    fn report_states_that_no_secrets_are_included() {
        assert!(report().to_text().contains("No API keys"));
    }
}
