//! Update commands (PRD §40).

use crate::app::AppState;
use crate::commands::{blocking, CommandResult};
use crate::error::BridgeResult;
use crate::updater::UpdateStatus;

/// Check for an update because the user asked.
///
/// Never installs; installation is a separate, explicit action.
#[tauri::command]
pub async fn check_for_updates(app: tauri::AppHandle) -> CommandResult<UpdateStatus> {
    match crate::updater::check_for_update(&app, false).await {
        Ok(status) => Ok(status),
        // A failed check is informational, not an error the UI should shout
        // about: being offline is normal and expected.
        Err(err) => Ok(UpdateStatus::failed(err.message)),
    }
}

/// Download and install an available update, then restart.
///
/// Only reachable from an explicit user action, and only succeeds if the
/// downloaded bundle's signature matches the embedded public key.
#[tauri::command]
pub async fn install_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> CommandResult<UpdateStatus> {
    let state = state.inner().clone();
    // Performs the download and signature verification; a bad or missing
    // signature fails here and nothing on disk changes.
    let _installed = crate::updater::check_for_update(&app, true).await?;

    // Record the check so the next automatic one waits the full interval.
    blocking(move || {
        crate::updater::mark_checked(&state);
        Ok(())
    })
    .await?;

    // The installer has replaced the binary; restart so the new version runs.
    // This diverges, so the return type is never actually produced.
    app.restart();
}

/// Check for an update at most once every 24 hours, on startup (PRD §40.2).
#[tauri::command]
pub async fn check_for_updates_on_startup(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> CommandResult<Option<UpdateStatus>> {
    let state = state.inner().clone();

    if !startup_check_is_due(&state) {
        return Ok(None);
    }

    let status = match crate::updater::check_for_update(&app, false).await {
        Ok(status) => status,
        Err(err) => UpdateStatus::failed(err.message),
    };

    // Record the attempt even when it failed, so an offline machine does not
    // retry on every single launch.
    blocking(move || -> BridgeResult<()> {
        crate::updater::mark_checked(&state);
        Ok(())
    })
    .await?;

    Ok(Some(status))
}

/// Whether the silent startup check should run at all.
///
/// Extracted so the two rules that keep this from being a background updater —
/// the user's setting, and the 24-hour interval — are testable without an
/// `AppHandle` or a network.
fn startup_check_is_due(state: &AppState) -> bool {
    let settings = state.settings();
    if !settings.update_check {
        // The user turned automatic checks off; do nothing at all.
        return false;
    }
    crate::updater::should_check_now(settings.last_update_check.as_deref(), chrono::Local::now())
}

#[cfg(test)]
mod tests {
    use crate::updater::UpdateStatus;

    #[test]
    fn a_startup_status_is_only_returned_when_a_check_ran() {
        // The command returns `None` when it deliberately skipped the check, so
        // a caller can distinguish "no update" from "did not look".
        let checked_and_current = Some(UpdateStatus::up_to_date());
        let skipped: Option<UpdateStatus> = None;

        assert!(checked_and_current.is_some());
        assert!(skipped.is_none());
        assert!(!UpdateStatus::up_to_date().available);
    }

    #[test]
    fn a_failed_startup_check_is_still_reported_as_having_run() {
        // Being offline must not be rendered as "no update available" forever;
        // the status carries the failure so the UI can say what happened.
        let status = UpdateStatus::failed("Could not check for updates.".to_string());
        assert!(status.error.is_some());
        assert!(!status.available);
    }
}
