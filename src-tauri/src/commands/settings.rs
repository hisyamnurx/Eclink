//! Settings commands (PRD §44.2, §54).

use serde::{Deserialize, Serialize};

use crate::app::{AppSettings, AppState};
use crate::commands::{blocking, CommandResult};

/// Everything the Settings screen needs in one round trip.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    #[serde(flatten)]
    pub settings: AppSettings,
    pub app_version: String,
    pub data_directory: String,
    pub log_directory: String,
    pub repository_url: String,
    /// Which secret backend is in use, e.g. "Windows DPAPI (Current User)".
    pub secret_backend: String,
    pub secret_count: usize,
    pub profile_count: usize,
    /// Whether the bundled credential helper was found.
    pub credential_helper_available: bool,
}

/// A partial settings update from the frontend.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    #[serde(default)]
    pub update_check: Option<bool>,
    #[serde(default)]
    pub remember_project_directory: Option<bool>,
    #[serde(default)]
    pub log_level: Option<crate::diagnostics::LogLevel>,
    #[serde(default)]
    pub first_run_complete: Option<bool>,
    #[serde(default)]
    pub cost_notice_seen: Option<bool>,
    #[serde(default)]
    pub insecure_endpoint_notice_seen: Option<bool>,
}

/// Read the current settings and environment facts.
#[tauri::command]
pub async fn get_settings(state: tauri::State<'_, AppState>) -> CommandResult<SettingsView> {
    let state = state.inner().clone();
    blocking(move || {
        let profile_count = state.profiles().load()?.profiles.len();
        Ok(SettingsView {
            settings: state.settings(),
            app_version: crate::APP_VERSION.to_string(),
            data_directory: state.paths().root().display().to_string(),
            log_directory: state.paths().logs_dir().display().to_string(),
            repository_url: crate::REPO_URL.to_string(),
            secret_backend: crate::secrets::backend_name().to_string(),
            secret_count: state.secrets().count()?,
            profile_count,
            credential_helper_available: state.helper_path().is_some(),
        })
    })
    .await
}

/// Apply a partial settings update.
#[tauri::command]
pub async fn update_settings(
    state: tauri::State<'_, AppState>,
    patch: SettingsPatch,
) -> CommandResult<SettingsView> {
    let state = state.inner().clone();
    blocking(move || {
        state.update_settings(|settings| {
            // Only explicitly-provided fields change, so a partial patch from an
            // older UI never resets an unrelated preference.
            if let Some(value) = patch.update_check {
                settings.update_check = value;
            }
            if let Some(value) = patch.remember_project_directory {
                settings.remember_project_directory = value;
                if !value {
                    // Stop remembering, and forget what was remembered.
                    settings.last_project_directory = None;
                }
            }
            if let Some(value) = patch.log_level {
                settings.log_level = value;
            }
            if let Some(value) = patch.first_run_complete {
                settings.first_run_complete = value;
            }
            if let Some(value) = patch.cost_notice_seen {
                settings.cost_notice_seen = value;
            }
            if let Some(value) = patch.insecure_endpoint_notice_seen {
                settings.insecure_endpoint_notice_seen = value;
            }
        })?;

        let profile_count = state.profiles().load()?.profiles.len();
        Ok(SettingsView {
            settings: state.settings(),
            app_version: crate::APP_VERSION.to_string(),
            data_directory: state.paths().root().display().to_string(),
            log_directory: state.paths().logs_dir().display().to_string(),
            repository_url: crate::REPO_URL.to_string(),
            secret_backend: crate::secrets::backend_name().to_string(),
            secret_count: state.secrets().count()?,
            profile_count,
            credential_helper_available: state.helper_path().is_some(),
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_patch_changes_nothing() {
        let mut settings = AppSettings::default();
        settings.update_check = false;
        settings.cost_notice_seen = true;

        let patch = SettingsPatch::default();
        // Simulate the same guarded assignment the command performs.
        if let Some(value) = patch.update_check {
            settings.update_check = value;
        }
        if let Some(value) = patch.cost_notice_seen {
            settings.cost_notice_seen = value;
        }

        assert!(!settings.update_check, "an absent field must not be reset");
        assert!(settings.cost_notice_seen, "an absent field must not be reset");
    }

    #[test]
    fn a_present_field_is_applied() {
        let mut settings = AppSettings::default();
        let patch = SettingsPatch {
            update_check: Some(false),
            ..Default::default()
        };
        if let Some(value) = patch.update_check {
            settings.update_check = value;
        }
        assert!(!settings.update_check);
    }
}
