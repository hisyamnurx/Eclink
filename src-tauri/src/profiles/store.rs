//! Profile persistence with schema versioning and forward-compatible migration
//! (PRD §44.1, §46).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::fs::{read_to_string_opt, write_atomic};
use crate::error::{BridgeError, BridgeResult};
use crate::profiles::model::Profile;

/// Current on-disk schema version for `profiles.json`.
pub const PROFILES_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesFile {
    /// Defaults to the current version so a hand-edited or pre-versioning file
    /// still loads instead of failing (PRD §46).
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

fn default_version() -> u32 {
    PROFILES_SCHEMA_VERSION
}

impl Default for ProfilesFile {
    fn default() -> Self {
        Self {
            version: PROFILES_SCHEMA_VERSION,
            profiles: Vec::new(),
        }
    }
}

/// Reads and writes the profile list.
pub struct ProfileStore {
    path: PathBuf,
}

impl ProfileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load profiles, migrating older schemas when needed.
    ///
    /// A fresh install yields an empty list — never seeded profiles (PRD §18.1).
    pub fn load(&self) -> BridgeResult<ProfilesFile> {
        let Some(raw) = read_to_string_opt(&self.path)? else {
            return Ok(ProfilesFile::default());
        };
        if raw.trim().is_empty() {
            return Ok(ProfilesFile::default());
        }

        // Parse loosely first so an unknown future version still yields the
        // fields we understand instead of crashing (PRD §46).
        let value: serde_json::Value = serde_json::from_str(&raw).map_err(|err| {
            BridgeError::config_invalid(
                "Your saved provider profiles could not be read.",
            )
            .with_hint("The file may be damaged. Restarting Eclyzer Bridge will recreate it.")
            .with_details(err.to_string())
        })?;

        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;

        let mut file: ProfilesFile = serde_json::from_value(value).map_err(|err| {
            BridgeError::config_invalid("Your saved provider profiles could not be read.")
                .with_details(err.to_string())
        })?;

        if version > PROFILES_SCHEMA_VERSION {
            // Downgrade: keep what we can read rather than refusing to start.
            tracing::warn!(
                target: "eclyzer_bridge",
                "profiles.json schema v{version} is newer than supported v{PROFILES_SCHEMA_VERSION}; reading best-effort"
            );
        } else if version < PROFILES_SCHEMA_VERSION {
            file = migrate(file, version)?;
        }

        file.version = PROFILES_SCHEMA_VERSION;
        Ok(file)
    }

    /// Persist profiles atomically.
    pub fn save(&self, file: &ProfilesFile) -> BridgeResult<()> {
        let mut to_write = file.clone();
        to_write.version = PROFILES_SCHEMA_VERSION;
        let json = serde_json::to_string_pretty(&to_write).map_err(|err| {
            BridgeError::internal("Could not serialise provider profiles.")
                .with_details(err.to_string())
        })?;
        write_atomic(&self.path, json.as_bytes())
    }

    /// Load, mutate, and persist in one call.
    pub fn update<F, T>(&self, mutate: F) -> BridgeResult<T>
    where
        F: FnOnce(&mut ProfilesFile) -> BridgeResult<T>,
    {
        let mut file = self.load()?;
        let result = mutate(&mut file)?;
        self.save(&file)?;
        Ok(result)
    }
}

/// Bring an older `profiles.json` forward.
///
/// v1 is the first released schema, so this currently only normalises the
/// version stamp; the seam exists so future changes are additive.
fn migrate(file: ProfilesFile, from_version: u32) -> BridgeResult<ProfilesFile> {
    let mut current = file;
    let mut version = from_version;

    while version < PROFILES_SCHEMA_VERSION {
        match version {
            0 => {
                // v0 == "pre-versioning": same shape, no version field.
                tracing::info!(target: "eclyzer_bridge", "migrating profiles.json from v0 to v1");
                version = 1;
            }
            other => {
                return Err(BridgeError::internal(format!(
                    "No migration path from profiles schema v{other}."
                )));
            }
        }
    }

    current.version = version;
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::model::AgentId;

    fn temp_store() -> (tempfile::TempDir, ProfileStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path().join("profiles.json"));
        (dir, store)
    }

    #[test]
    fn fresh_install_has_no_profiles() {
        let (_dir, store) = temp_store();
        let file = store.load().unwrap();
        assert!(file.profiles.is_empty(), "fresh install must not seed profiles");
        assert_eq!(file.version, PROFILES_SCHEMA_VERSION);
    }

    #[test]
    fn save_and_reload_round_trips() {
        let (_dir, store) = temp_store();
        let mut file = ProfilesFile::default();
        file.profiles.push(Profile::new(
            "DeepSeek".into(),
            "DeepSeek".into(),
            "https://api.deepseek.com/v1".into(),
            "deepseek-chat".into(),
            vec![AgentId::Codex],
        ));
        store.save(&file).unwrap();

        let reloaded = store.load().unwrap();
        assert_eq!(reloaded.profiles.len(), 1);
        assert_eq!(reloaded.profiles[0].name, "DeepSeek");
    }

    #[test]
    fn profile_file_never_contains_an_api_key() {
        let (_dir, store) = temp_store();
        let mut profile = Profile::new(
            "P".into(),
            "Prov".into(),
            "https://api.example.com/v1".into(),
            "m".into(),
            vec![AgentId::Codex],
        );
        profile.secret_ref = Some("secret-id-1".into());

        let mut file = ProfilesFile::default();
        file.profiles.push(profile);
        store.save(&file).unwrap();

        let raw = std::fs::read_to_string(store.path()).unwrap();
        assert!(raw.contains("secret-id-1"), "the reference must persist");
        for forbidden in ["apiKey", "api_key", "sk-", "Authorization"] {
            assert!(
                !raw.contains(forbidden),
                "profiles.json must not contain {forbidden}: {raw}"
            );
        }
    }

    #[test]
    fn corrupt_file_gives_actionable_error_not_panic() {
        let (_dir, store) = temp_store();
        std::fs::write(store.path(), b"{ this is not json").unwrap();
        let err = store.load().unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::ConfigInvalid);
        assert!(err.hint.is_some(), "error must suggest a next step");
    }

    #[test]
    fn unknown_future_version_still_loads_known_fields() {
        let (_dir, store) = temp_store();
        std::fs::write(
            store.path(),
            br#"{"version": 99, "profiles": [], "futureField": {"x": 1}}"#,
        )
        .unwrap();
        let file = store.load().unwrap();
        assert_eq!(file.version, PROFILES_SCHEMA_VERSION);
    }

    #[test]
    fn versionless_file_migrates() {
        let (_dir, store) = temp_store();
        std::fs::write(store.path(), br#"{"profiles": []}"#).unwrap();
        let file = store.load().unwrap();
        assert_eq!(file.version, PROFILES_SCHEMA_VERSION);
    }

    #[test]
    fn update_persists_mutation() {
        let (_dir, store) = temp_store();
        store
            .update(|file| {
                file.profiles.push(Profile::new(
                    "N".into(),
                    "P".into(),
                    "https://e.com".into(),
                    "m".into(),
                    vec![AgentId::Claude],
                ));
                Ok(())
            })
            .unwrap();
        assert_eq!(store.load().unwrap().profiles.len(), 1);
    }
}
