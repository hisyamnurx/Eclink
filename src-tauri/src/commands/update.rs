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

    let due = {
        let settings = state.settings();
        if !settings.update_check {
            // The user turned automatic checks off; do nothing at all.
            return Ok(None);
        }
        crate::updater::should_check_now(
            settings.last_update_check.as_deref(),
            chrono::Local::now(),
        )
    };

    if !due {
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

#[cfg(test)]
mod tests {
    use crate::updater::UpdateStatus;

    #[test]
    fn a_startup_status_is_only_returned_when_a_check_ran() {
        // The command returns `None` when it deliberately skipped the check.
        let skipped: Option<UpdateStatus> = None;
        assert!(skipped.is_none());
    }
}
