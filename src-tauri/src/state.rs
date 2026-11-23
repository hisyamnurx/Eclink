//! `app.json` persistence and schema migration (PRD §44.2, §46).

use serde_json::Value;

use crate::app::state::AppSettings;
use crate::config::fs::{read_to_string_opt, write_atomic};
use crate::error::{BridgeError, BridgeResult};
use crate::paths::AppPaths;

/// Current `app.json` schema version.
///
/// 2 added `agentLocations`. The field is optional and defaults to "automatic
/// detection for every agent", so the bump records the change without requiring
/// a transformation: a version-1 file loads unchanged and simply has no saved
/// locations.
pub const APP_SETTINGS_SCHEMA_VERSION: u32 = 2;

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
                target: "eclink",
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
                target: "eclink",
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
            target: "eclink",
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
            "agentLocations" => {
                // Parsed as a unit: a half-readable location map is not worth
                // trusting, and dropping it only means automatic detection.
                if let Ok(locations) =
                    serde_json::from_value::<crate::adapters::AgentLocations>(value.clone())
                {
                    settings.agent_locations = locations;
                }
            }
            _ => {}
        }
    }

    tracing::warn!(
        target: "eclink",
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
    fn a_settings_file_without_locations_reads_as_automatic() {
        // The migration path for every installation that upgrades: the file has
        // no `agentLocations`, so every agent keeps automatic detection.
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(dir.path());
        std::fs::write(
            paths.app_json(),
            br#"{"schemaVersion":1,"updateCheck":true,"language":"id"}"#,
        )
        .unwrap();

        let settings = load_settings(&paths).unwrap();

        assert!(settings.agent_locations.is_empty());
        assert_eq!(settings.language, "id", "other settings still load");
        assert_eq!(settings.schema_version, APP_SETTINGS_SCHEMA_VERSION);
    }

    #[test]
    fn saved_locations_survive_a_save_and_reload_per_agent() {
        use crate::adapters::{AgentId, AgentLocation, LocationKind};

        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(dir.path());
        let mut settings = load_settings(&paths).unwrap();
        settings.agent_locations.set(
            AgentId::Codex,
            Some(AgentLocation::new(r"D:\Tools\codex", LocationKind::Folder)),
        );
        settings.agent_locations.set(
            AgentId::Omp,
            Some(AgentLocation::new(
                r"D:\Tools\omp\omp.exe",
                LocationKind::Launcher,
            )),
        );
        save_settings(&paths, &settings).unwrap();

        let reloaded = load_settings(&paths).unwrap();

        assert_eq!(
            reloaded.agent_locations.codex,
            settings.agent_locations.codex
        );
        assert_eq!(reloaded.agent_locations.omp, settings.agent_locations.omp);
        assert!(
            reloaded.agent_locations.claude.is_none(),
            "an agent that was never configured stays automatic"
        );

        // Clearing one agent must not disturb the others.
        let mut cleared = reloaded.clone();
        cleared.agent_locations.set(AgentId::Codex, None);
        save_settings(&paths, &cleared).unwrap();
        let after = load_settings(&paths).unwrap();
        assert!(after.agent_locations.codex.is_none());
        assert!(after.agent_locations.omp.is_some());
    }

    #[test]
    fn a_malformed_unrelated_field_does_not_discard_saved_locations() {
        // `#[serde(default)]` only covers absent keys. A wrong-typed value sends
        // the loader down the lenient path, which must still recover locations.
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(dir.path());
        std::fs::write(
            paths.app_json(),
            br#"{
              "schemaVersion": 2,
              "updateCheck": "not a boolean",
              "agentLocations": {
                "claude": { "path": "D:\\Tools\\Claude O'Brien", "kind": "folder" }
              }
            }"#,
        )
        .unwrap();

        let settings = load_settings(&paths).unwrap();

        let claude = settings
            .agent_locations
            .claude
            .as_ref()
            .expect("the saved location is still readable");
        assert_eq!(claude.path, r"D:\Tools\Claude O'Brien");
        assert!(
            settings.update_check,
            "the bad field falls back to its default"
        );
    }

    #[test]
    fn app_json_always_carries_the_agent_location_map() {
        // The settings view flattens `AppSettings`, so an omitted field would
        // arrive at the interface as `undefined` rather than an empty map.
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(dir.path());
        save_settings(&paths, &AppSettings::default()).unwrap();

        let raw = std::fs::read_to_string(paths.app_json()).unwrap();
        assert!(raw.contains("\"agentLocations\": {}"), "{raw}");
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
