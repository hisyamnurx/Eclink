//! Diagnostics and log commands (PRD §38, §39).

use serde::Serialize;

use crate::app::AppState;
use crate::commands::{blocking, CommandResult};
use crate::error::{BridgeError, BridgeResult};

/// A diagnostics report ready to copy or save.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsBundle {
    /// The full text report. Contains no API keys and no prompts.
    pub text: String,
    pub app_version: String,
    pub data_directory: String,
    pub log_directory: String,
}

/// Build the copyable diagnostics report.
#[tauri::command]
pub async fn build_diagnostics_report(
    state: tauri::State<'_, AppState>,
) -> CommandResult<DiagnosticsBundle> {
    let state = state.inner().clone();
    blocking(move || {
        let report = crate::diagnostics::build_report(&state)?;
        Ok(DiagnosticsBundle {
            text: report.to_text(),
            app_version: crate::APP_VERSION.to_string(),
            data_directory: state.paths().root().display().to_string(),
            log_directory: state.paths().logs_dir().display().to_string(),
        })
    })
    .await
}

/// Read the tail of the current log file, already redacted (PRD §39).
#[tauri::command]
pub async fn read_logs(
    state: tauri::State<'_, AppState>,
    max_lines: Option<usize>,
) -> CommandResult<Vec<String>> {
    let state = state.inner().clone();
    let max_lines = max_lines.unwrap_or(500).min(5_000);
    blocking(move || {
        crate::diagnostics::read_recent_log_lines(&state.paths().logs_dir(), max_lines)
    })
    .await
}

/// Clear every log file (PRD §39.4).
#[tauri::command]
pub async fn clear_logs(state: tauri::State<'_, AppState>) -> CommandResult<()> {
    let state = state.inner().clone();
    blocking(move || {
        crate::diagnostics::clear_logs(&state.paths().logs_dir())?;
        Ok(())
    })
    .await
}

/// Open the data directory so the user can inspect it themselves.
#[tauri::command]
pub async fn reveal_data_directory(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> CommandResult<()> {
    open_in_file_manager(&app, state.paths().root()).map_err(Into::into)
}

/// Open the log directory.
#[tauri::command]
pub async fn reveal_log_directory(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> CommandResult<()> {
    open_in_file_manager(&app, &state.paths().logs_dir()).map_err(Into::into)
}

/// Open a directory in Explorer.
///
/// Uses the opener plugin rather than spawning a shell, so no command line is
/// ever constructed from a path (PRD §63). The path is validated to exist first,
/// so the user gets an explanation instead of Explorer opening somewhere
/// unexpected.
fn open_in_file_manager(app: &tauri::AppHandle, path: &std::path::Path) -> BridgeResult<()> {
    use tauri_plugin_opener::OpenerExt;

    if !path.exists() {
        return Err(
            BridgeError::io(format!("The folder {} does not exist yet.", path.display()))
                .with_hint("It is created the first time Eclink saves something."),
        );
    }
    if !path.is_dir() {
        return Err(BridgeError::io(format!(
            "{} is not a folder.",
            path.display()
        )));
    }

    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|err| {
            BridgeError::io("Could not open the folder in File Explorer.")
                .with_details(err.to_string())
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagnostics_bundle_fields_are_populated_from_state() {
        // The bundle is built inside a command that needs a Tauri state, so the
        // shape is asserted through the report type instead.
        let report = crate::diagnostics::DiagnosticsReport {
            app_version: crate::APP_VERSION.to_string(),
            platform: "windows".to_string(),
            data_dir: "C:\\data".to_string(),
            log_dir: "C:\\data\\logs".to_string(),
            secret_backend: "Windows DPAPI".to_string(),
            secret_count: 0,
            profile_count: 0,
            agents: Vec::new(),
            warnings: Vec::new(),
        };
        let text = report.to_text();
        assert!(text.contains(crate::APP_VERSION), "{text}");
        assert!(text.contains("C:\\data"), "{text}");
    }
}
