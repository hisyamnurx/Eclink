//! Application assembly: state, plugins, and the command surface.
//!
//! This is the only place that knows the full set of Tauri commands. Keeping the
//! list here makes the IPC surface auditable at a glance — if it is not
//! registered below, the frontend cannot call it.

pub mod state;

pub use state::{AppSettings, AppState};

use crate::error::{BridgeError, BridgeResult};

/// Build the configured Tauri application.
///
/// State is constructed before the builder so a bootstrap failure can be
/// reported honestly rather than surfacing later as a mysterious "state not
/// managed" panic inside the first command call.
pub fn builder() -> BridgeResult<tauri::Builder<tauri::Wry>> {
    let state = AppState::bootstrap()?;

    let builder = tauri::Builder::default()
        // Single-instance behaviour is intentionally omitted: two windows writing
        // the same agent config would be unsafe, but the per-agent file lock in
        // each adapter already serialises those writes, and refusing to start a
        // second window would surprise users more than it helps.
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            // ---- agents -------------------------------------------------
            crate::commands::list_agents,
            crate::commands::refresh_agent,
            crate::commands::get_diagnostics,
            // ---- profiles -----------------------------------------------
            crate::commands::list_profiles,
            crate::commands::get_profile,
            crate::commands::save_profile,
            crate::commands::delete_profile,
            crate::commands::profile_delete_impact,
            crate::commands::list_provider_templates,
            crate::commands::test_profile_connection,
            crate::commands::clear_compatibility,
            crate::commands::get_cost_notice,
            // ---- activation and launch ----------------------------------
            crate::commands::activate_profile,
            crate::commands::preview_launch,
            crate::commands::launch_agent,
            // ---- recovery -----------------------------------------------
            crate::commands::restore_default,
            crate::commands::restore_original,
            crate::commands::restore_snapshot,
            crate::commands::undo_last_change,
            crate::commands::list_snapshots,
            crate::commands::list_history,
            crate::commands::recovery_status,
            // ---- settings and diagnostics -------------------------------
            crate::commands::get_settings,
            crate::commands::update_settings,
            crate::commands::build_diagnostics_report,
            crate::commands::read_logs,
            crate::commands::clear_logs,
            crate::commands::reveal_data_directory,
            crate::commands::reveal_log_directory,
            // ---- updates -------------------------------------------------
            crate::commands::check_for_updates,
            crate::commands::check_for_updates_on_startup,
            crate::commands::install_update,
        ]);

    Ok(builder)
}

/// Log a startup failure and return a short, actionable message.
pub fn bootstrap_error_message(err: &BridgeError) -> String {
    let mut message = format!(
        "Eclyzer Bridge could not start.\n\n{}\n",
        err.message
    );
    if let Some(hint) = &err.hint {
        message.push_str(&format!("\n{hint}\n"));
    }
    if let Some(details) = &err.details {
        message.push_str(&format!("\nDetails: {details}\n"));
    }
    message
}
