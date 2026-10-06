//! `app.json` persistence and schema migration (PRD §44.2, §46).

use serde_json::Value;

use crate::app::state::AppSettings;
use crate::config::fs::{read_to_string_opt, write_atomic};
use crate::error::{BridgeError, BridgeResult};
use crate::paths::AppPaths;

/// Current `app.json` schema version.
pub const APP_SETTINGS_SCHEMA_VERSION: u32 = 1;

/// Load `app.json`, migrating older or partial files.
///
/// A missing, empty, or unreadable file yields defaults rather than blocking
/// startup: preferences are not worth failing to launch over.
pub fn load_settings(paths: &AppPaths) -> BridgeResult<AppSettings> {
    let path = paths.app_json();
    let Some(raw) = read_to_string_opt(&path)? else {
        return Ok(AppSettings::default());
    };
    if raw.trim().is_empty() {
        return Ok(AppSettings::default());
    }

    let value: Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(
                target: "eclyzer_bridge",
                error = %err,
                "app.json is unreadable; continuing with defaults"
            );
            return Ok(AppSettings::default());
        }
    };

    let version = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;

    // Merge onto defaults field-by-field so one unrecognised or malformed value
    // cannot discard the settings the user did set. `#[serde(default)]` only
    // covers *absent* keys; this also covers a wrong-typed key.
    let settings = match serde_json::from_value::<AppSettings>(value) {
        Ok(settings) => settings,
        Err(err) => {
            tracing::warn!(
                target: "eclyzer_bridge",
                error = %err,
                "app.json has an unexpected value; falling back to defaults for the affected field"
            );
            // Re-read leniently: drop the offending keys and retry once.
            settings_from_lenient(raw.as_str(), &err.to_string())
        }
    };

    let mut settings = settings;
    if version < APP_SETTINGS_SCHEMA_VERSION {
        // v0 == no version field; the shape is identical, so bumping the stamp
        // is the whole migration. Future versions add explicit steps here.
        tracing::info!(
            target: "eclyzer_bridge",
            from = version,
            to = APP_SETTINGS_SCHEMA_VERSION,
            "migrating app.json"
        );
    }
    settings.schema_version = APP_SETTINGS_SCHEMA_VERSION;
    Ok(settings)
}

/// Best-effort parse that keeps whatever fields are individually valid.
fn settings_from_lenient(raw: &str, error: &str) -> AppSettings {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return AppSettings::default();
    };
    let Some(map) = value.as_object() else {
        return AppSettings::default();
    };

    let mut settings = AppSettings::default();
    // The field named in the serde error is the one to drop; everything else is
    // still trustworthy. Rather than parse the error text, validate each known
    // field by type, which is deterministic.
    for (key, value) in map {
        match key.as_str() {
            "updateCheck" => {
                if let Some(flag) = value.as_bool() {
                    settings.update_check = flag;
                }
            }
            "rememberProjectDirectory" => {
                if let Some(flag) = value.as_bool() {
                    settings.remember_project_directory = flag;
                }
            }
            "firstRunComplete" => {
                if let Some(flag) = value.as_bool() {
                    settings.first_run_complete = flag;
                }
            }
            "costNoticeSeen" => {
                if let Some(flag) = value.as_bool() {
                    settings.cost_notice_seen = flag;
                }
            }
            "insecureEndpointNoticeSeen" => {
                if let Some(flag) = value.as_bool() {
                    settings.insecure_endpoint_notice_seen = flag;
                }
            }
            "lastUpdateCheck" => {
                if let Some(text) = value.as_str() {
                    settings.last_update_check = Some(text.to_string());
                }
            }
            "lastProjectDirectory" => {
                if let Some(text) = value.as_str() {
                    settings.last_project_directory = Some(text.to_string());
                }
            }
            "logLevel" => {
                if let Ok(level) =
                    serde_json::from_value::<crate::diagnostics::LogLevel>(value.clone())
                {
                    settings.log_level = level;
                }
            }
            _ => {}
        }
    }

    tracing::warn!(
        target: "eclyzer_bridge",
        detail = %error,
        "recovered application settings leniently"
    );

    settings
}

/// Persist `app.json` atomically.
pub fn save_settings(paths: &AppPaths, settings: &AppSettings) -> BridgeResult<()> {
    let mut to_write = settings.clone();
    to_write.schema_version = APP_SETTINGS_SCHEMA_VERSION;
    let json = serde_json::to_string_pretty(&to_write).map_err(|err| {
        BridgeError::internal("Could not save application settings.").with_details(err.to_string())
    })?;
    write_atomic(&paths.app_json(), json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(dir.path());
        let settings = load_settings(&paths).unwrap();
        assert!(settings.update_check);
        assert!(!settings.first_run_complete);
    }

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(dir.path());
        let settings = AppSettings {
            last_project_directory: Some(r"C:\Users\Test User\My Project\".into()),
            first_run_complete: true,
            ..AppSettings::default()
        };
        save_settings(&paths, &settings).unwrap();

        let reloaded = load_settings(&paths).unwrap();
        assert_eq!(
            reloaded.last_project_directory.as_deref(),
            Some(r"C:\Users\Test User\My Project\")
        );
        assert!(reloaded.first_run_complete);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults_without_crashing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(dir.path());
        std::fs::write(paths.app_json(), b"{{{ not json").unwrap();
        let settings = load_settings(&paths).unwrap();
        assert_eq!(settings.schema_version, APP_SETTINGS_SCHEMA_VERSION);
    }

    #[test]
    fn partial_file_uses_defaults_for_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(dir.path());
        std::fs::write(paths.app_json(), br#"{"updateCheck": false}"#).unwrap();
        let settings = load_settings(&paths).unwrap();
        assert!(!settings.update_check, "explicit value must win");
        assert!(
            settings.remember_project_directory,
            "missing field must default"
        );
    }

    #[test]
    fn app_json_never_contains_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(dir.path());
        save_settings(&paths, &AppSettings::default()).unwrap();
        let raw = std::fs::read_to_string(paths.app_json()).unwrap();
        for forbidden in ["apiKey", "api_key", "sk-", "Authorization", "Bearer"] {
            assert!(
                !raw.contains(forbidden),
                "app.json leaked {forbidden}: {raw}"
            );
        }
    }
}
