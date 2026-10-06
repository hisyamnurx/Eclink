//! Update checking against GitHub Releases (PRD §40).
//!
//! Bridge does **not** phone home to Eclyzer. The only outbound update traffic
//! is a request for this project's own public release manifest on GitHub, made
//! at most once every 24 hours on startup, plus whenever the user asks.
//!
//! ## Guarantees
//!
//! * **No silent installation.** An available update is reported; installing it
//!   is an explicit user action.
//! * **Signed artifacts only.** Handled by `tauri-plugin-updater`, which refuses
//!   a payload whose signature does not match the embedded public key. This
//!   module never bypasses that check.
//! * **No private key here.** The signing key is a maintainer secret supplied at
//!   release time (see `RELEASING.md`); it is never in the repository.

use serde::Serialize;

use crate::error::{BridgeError, BridgeResult};

/// How long to wait between automatic startup checks (PRD §40.2).
pub const CHECK_INTERVAL_HOURS: i64 = 24;

/// What the UI needs to know about update state.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// Whether the check itself succeeded.
    pub checked: bool,
    /// Whether a newer version is available.
    pub available: bool,
    /// Version offered by the release manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Release notes, when the manifest carries them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// The version currently running.
    pub current_version: String,
    /// Why the check could not complete, when it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl UpdateStatus {
    /// The state before any check has run.
    pub fn unchecked() -> Self {
        Self {
            checked: false,
            available: false,
            version: None,
            notes: None,
            current_version: crate::APP_VERSION.to_string(),
            error: None,
        }
    }

    /// The state when no newer version exists.
    pub fn up_to_date() -> Self {
        Self {
            checked: true,
            ..Self::unchecked()
        }
    }

    /// The state when a check failed. A failed check is never an error dialog;
    /// it is informational, because being offline is normal.
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            checked: false,
            error: Some(crate::diagnostics::redact_text(&message.into())),
            ..Self::unchecked()
        }
    }
}

/// Whether an automatic check is due, given the last check timestamp.
///
/// A missing or unparsable timestamp counts as due, so a user who has never
/// checked gets one.
pub fn should_check_now(last_check: Option<&str>, now: chrono::DateTime<chrono::Local>) -> bool {
    let Some(raw) = last_check else {
        return true;
    };
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(raw) else {
        return true;
    };
    let elapsed = now.signed_duration_since(parsed.with_timezone(&chrono::Local));
    elapsed.num_hours() >= CHECK_INTERVAL_HOURS
}

/// Ask GitHub Releases whether a newer signed version exists.
///
/// `confirm` performs the installation and is only ever invoked from an explicit
/// user action.
pub async fn check_for_update(
    app: &tauri::AppHandle,
    allow_download: bool,
) -> BridgeResult<UpdateStatus> {
    use tauri_plugin_updater::UpdaterExt;

    let updater = app.updater().map_err(|err| {
        BridgeError::update("Update checking is not configured in this build.")
            .with_hint("This build has no updater signing key, so updates are unavailable.")
            .with_details(err.to_string())
    })?;

    let update = updater.check().await.map_err(|err| {
        // A failed check is expected when offline; keep the message short and
        // never surface a raw reqwest chain to the user.
        BridgeError::network("Could not check for updates.")
            .with_hint("This is usually a network problem. Eclyzer Bridge will try again later.")
            .with_details(err.to_string())
    })?;

    let Some(update) = update else {
        return Ok(UpdateStatus::up_to_date());
    };

    if !allow_download {
        // Report only; installation requires an explicit user action.
        return Ok(UpdateStatus {
            checked: true,
            available: true,
            version: Some(update.version.clone()),
            notes: update.body.clone(),
            current_version: update.current_version.clone(),
            error: None,
        });
    }

    // The plugin verifies the bundle signature against the embedded public key
    // before this returns; a payload with a bad or missing signature fails here.
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|err| {
            BridgeError::update("The update could not be installed.")
                .with_hint(
                    "The download may have been interrupted, or its signature did not match. Nothing on your machine was changed.",
                )
                .with_details(err.to_string())
        })?;

    Ok(UpdateStatus {
        checked: true,
        available: true,
        version: Some(update.version.clone()),
        notes: update.body.clone(),
        current_version: update.current_version.clone(),
        error: None,
    })
}

/// Record that a check happened, so the next automatic one waits 24 hours.
pub fn mark_checked(state: &crate::app::AppState) {
    let _ = state.update_settings(|settings| {
        settings.last_update_check = Some(chrono::Local::now().to_rfc3339());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hours_ago: i64) -> chrono::DateTime<chrono::Local> {
        let now = chrono::Local.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        now - chrono::Duration::hours(hours_ago)
    }

    #[test]
    fn a_missing_timestamp_is_always_due() {
        assert!(should_check_now(None, at(0)));
    }

    #[test]
    fn an_unparsable_timestamp_is_treated_as_due() {
        assert!(should_check_now(Some("not a timestamp"), at(0)));
    }

    #[test]
    fn a_recent_check_is_not_due() {
        let now = chrono::Local.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let one_hour_ago = (now - chrono::Duration::hours(1)).to_rfc3339();
        assert!(!should_check_now(Some(&one_hour_ago), now));
    }

    #[test]
    fn a_check_older_than_the_interval_is_due() {
        let now = chrono::Local.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let twenty_five_hours_ago = (now - chrono::Duration::hours(25)).to_rfc3339();
        assert!(should_check_now(Some(&twenty_five_hours_ago), now));
    }

    #[test]
    fn exactly_the_interval_is_due() {
        let now = chrono::Local.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let exactly = (now - chrono::Duration::hours(CHECK_INTERVAL_HOURS)).to_rfc3339();
        assert!(should_check_now(Some(&exactly), now));
    }

    #[test]
    fn a_future_timestamp_does_not_panic() {
        // A clock change must not make the app check on every launch forever.
        let now = chrono::Local.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let future = (now + chrono::Duration::hours(48)).to_rfc3339();
        assert!(!should_check_now(Some(&future), now));
    }

    #[test]
    fn status_helpers_report_sane_defaults() {
        let unchecked = UpdateStatus::unchecked();
        assert!(!unchecked.checked);
        assert!(!unchecked.available);
        assert_eq!(unchecked.current_version, crate::APP_VERSION);

        let current = UpdateStatus::up_to_date();
        assert!(current.checked);
        assert!(!current.available);
    }

    #[test]
    fn a_failed_status_is_informational_not_fatal() {
        let failed = UpdateStatus::failed("connection refused");
        assert!(!failed.checked);
        assert!(failed.error.is_some());
        // It must still report the running version so the UI has something true.
        assert_eq!(failed.current_version, crate::APP_VERSION);
    }

    #[test]
    fn a_failed_status_redacts_its_message() {
        let failed = UpdateStatus::failed("failed with token sk-abcdefghijklmnop123456");
        let error = failed.error.unwrap();
        assert!(!error.contains("sk-abcdefghijklmnop123456"), "{error}");
    }
}
