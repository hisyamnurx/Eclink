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
            BridgeError::config_invalid("Your saved provider profiles could not be read.")
                .with_hint("The file may be damaged. Restarting Eclink will recreate it.")
                .with_details(err.to_string())
        })?;

        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;

        let mut file: ProfilesFile = match serde_json::from_value(value.clone()) {
            Ok(file) => file,
            Err(err) => {
                // A whole-file failure would show the user no providers at all, even
                // though most of their entries are fine. Fall back to reading the
                // array entry by entry so one damaged profile costs one profile.
                match salvage_profiles(&value) {
                    Some((file, dropped)) => {
                        tracing::warn!(
                            target: "eclink",
                            dropped,
                            "profiles.json did not parse as a whole; kept the readable profiles"
                        );
                        file
                    }
                    None => {
                        return Err(BridgeError::config_invalid(
                            "Your saved provider profiles could not be read.",
                        )
                        .with_details(err.to_string()))
                    }
                }
            }
        };

        if version > PROFILES_SCHEMA_VERSION {
            // Downgrade: keep what we can read rather than refusing to start.
            tracing::warn!(
                target: "eclink",
                "profiles.json schema v{version} is newer than supported v{PROFILES_SCHEMA_VERSION}; reading best-effort"
            );
        } else if version < PROFILES_SCHEMA_VERSION {
            file = migrate(file, version)?;
        }

        file.version = PROFILES_SCHEMA_VERSION;

        // Normalise fields that default to empty but are shown to the user, so an
        // older file does not render a blank provider label.
        for profile in &mut file.profiles {
            if profile.provider_name.trim().is_empty() {
                profile.provider_name = profile.name.clone();
            }
        }

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

/// Read as many profiles as possible from a file that did not parse as a whole.
///
/// Returns the surviving profiles and how many were dropped, or `None` if the
/// document does not even have a `profiles` array — in which case there is nothing
/// to salvage and the caller should report the original error.
///
/// This exists because the profiles array is the user's own data, and losing all of
/// it to one malformed entry is a far worse outcome than losing that one. The
/// caller logs the count so a silent partial load is still visible in the log.
fn salvage_profiles(value: &serde_json::Value) -> Option<(ProfilesFile, usize)> {
    let entries = value.get("profiles")?.as_array()?;
    let mut kept = Vec::new();
    let mut dropped = 0usize;
    for entry in entries {
        match serde_json::from_value::<Profile>(entry.clone()) {
            Ok(profile) => kept.push(profile),
            Err(err) => {
                dropped += 1;
                tracing::warn!(
                    target: "eclink",
                    error = %err,
                    "skipping one unreadable provider profile"
                );
            }
        }
    }
    // If nothing survived there is no partial success worth reporting, and returning
    // an empty list would look like the user has no providers rather than that their
    // file is broken.
    if kept.is_empty() && dropped > 0 {
        return None;
    }
    Some((
        ProfilesFile {
            version: PROFILES_SCHEMA_VERSION,
            profiles: kept,
        },
        dropped,
    ))
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
                tracing::info!(target: "eclink", "migrating profiles.json from v0 to v1");
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
    use crate::profiles::model::{AgentId, CompatibilityStatus};

    fn temp_store() -> (tempfile::TempDir, ProfileStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path().join("profiles.json"));
        (dir, store)
    }

    #[test]
    fn fresh_install_has_no_profiles() {
        let (_dir, store) = temp_store();
        let file = store.load().unwrap();
        assert!(
            file.profiles.is_empty(),
            "fresh install must not seed profiles"
        );
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
    fn a_profile_written_before_compatibility_existed_still_loads() {
        // The oldest shape: an endpoint, a model, and nothing else. Every field
        // added since has to default, or an existing user's file would fail to load
        // and their providers would appear to have vanished.
        let (_dir, store) = temp_store();
        std::fs::write(
            store.path(),
            br#"{
              "version": 1,
              "profiles": [
                {
                  "id": "old-1",
                  "name": "provider_a",
                  "providerName": "provider_a",
                  "baseUrl": "https://gateway.example.com/v1",
                  "model": "example-model",
                  "secretRef": "old-1",
                  "targets": ["claude"]
                }
              ]
            }"#,
        )
        .unwrap();

        let file = store.load().expect("an older profile must still load");
        assert_eq!(file.profiles.len(), 1);
        let profile = &file.profiles[0];
        assert_eq!(profile.name, "provider_a");
        // The endpoint and model are the two values a user must not lose.
        assert_eq!(profile.base_url, "https://gateway.example.com/v1");
        assert_eq!(profile.model, "example-model");
        assert_eq!(profile.targets, vec![AgentId::Claude]);
        // Compatibility is unknown rather than assumed compatible.
        assert_eq!(
            profile.compatibility_for(AgentId::Claude),
            CompatibilityStatus::Untested,
            "an untested legacy profile must not read as compatible"
        );
    }

    #[test]
    fn a_profile_missing_optional_fields_entirely_still_loads() {
        let (_dir, store) = temp_store();
        std::fs::write(
            store.path(),
            br#"{"version":1,"profiles":[{"id":"m","name":"minimal","providerName":"p",
                 "baseUrl":"https://example.com","model":"m","secretRef":"m"}]}"#,
        )
        .unwrap();

        let file = store.load().expect("a minimal profile must load");
        let profile = &file.profiles[0];
        assert!(
            profile.targets.is_empty(),
            "no targets means no agent selected"
        );
        assert_eq!(profile.base_url, "https://example.com");
    }

    #[test]
    fn an_unknown_field_does_not_prevent_a_profile_from_loading() {
        // Forward compatibility in the other direction: a file written by a NEWER
        // build, or hand-edited by the user, must not have its unknown keys silently
        // deleted just because this build loaded and re-saved the file.
        let (_dir, store) = temp_store();
        std::fs::write(
            store.path(),
            br#"{"version":1,"profiles":[{"id":"k","name":"kept","providerName":"p",
                 "baseUrl":"https://example.com","model":"m","secretRef":"k",
                 "userNote":"do not lose me"}]}"#,
        )
        .unwrap();

        let file = store.load().expect("loads");
        assert_eq!(
            file.profiles.len(),
            1,
            "the profile survives an unknown field"
        );

        // Re-saving must not be lossy for the fields this build understands.
        store.save(&file).expect("saves");
        let reloaded = store.load().expect("reloads");
        assert_eq!(reloaded.profiles[0].name, "kept");
        assert_eq!(reloaded.profiles[0].base_url, "https://example.com");
    }

    #[test]
    fn the_users_real_profile_shape_loads_and_keeps_both_agents() {
        // The exact structure of the two providers that were in use, so a change to
        // the persisted shape is caught here rather than by a user losing data.
        let (_dir, store) = temp_store();
        std::fs::write(
            store.path(),
            br#"{
              "version": 1,
              "profiles": [
                {
                  "id": "ef2018da-90db-4292-954d-34297a3eb475",
                  "name": "provider_a",
                  "providerName": "provider_a",
                  "baseUrl": "https://gateway.example.com/v1",
                  "model": "example-model",
                  "secretRef": "ef2018da-90db-4292-954d-34297a3eb475",
                  "targets": ["codex", "claude"],
                  "advanced": { "authMode": "x_api_key" },
                  "compatibility": {
                    "claude": {
                      "status": "compatible",
                      "testedAt": "2026-10-07T00:19:25+07:00",
                      "latencyMs": 1959,
                      "streaming": true,
                      "toolCalling": true
                    }
                  },
                  "createdAt": "2026-10-06T20:00:00+07:00",
                  "updatedAt": "2026-10-07T00:19:25+07:00"
                },
                {
                  "id": "d2964cd8-07d7-47b0-a1bd-400609b5a11d",
                  "name": "provider_b",
                  "providerName": "provider_b",
                  "baseUrl": "https://router.example.com/v1",
                  "model": "vendor_a/example-model",
                  "secretRef": "d2964cd8-07d7-47b0-a1bd-400609b5a11d",
                  "targets": ["codex", "claude"],
                  "compatibility": {
                    "codex": {
                      "status": "incompatible",
                      "reasonCode": "endpoint_unsupported"
                    }
                  }
                }
              ]
            }"#,
        )
        .unwrap();

        let file = store.load().expect("the real shape must load");
        assert_eq!(file.profiles.len(), 2);

        let provider_a = file
            .profiles
            .iter()
            .find(|p| p.name == "provider_a")
            .expect("provider_a survives");
        assert_eq!(provider_a.base_url, "https://gateway.example.com/v1");
        assert_eq!(provider_a.model, "example-model");
        // The verified verdict and its evidence must come back intact: this is what
        // makes Activate usable on the next launch without a re-test.
        let record = provider_a
            .compatibility
            .get(&AgentId::Claude)
            .expect("the claude verdict persisted");
        assert_eq!(record.status, CompatibilityStatus::Compatible);
        assert_eq!(record.latency_ms, Some(1959));
        assert!(record.streaming);
        assert!(record.tool_calling);
        // And the other agent stays untested rather than inheriting the verdict.
        assert_eq!(
            provider_a.compatibility_for(AgentId::Codex),
            CompatibilityStatus::Untested,
            "a per-agent verdict must not leak to the other agent"
        );

        let provider_b = file
            .profiles
            .iter()
            .find(|p| p.name == "provider_b")
            .expect("provider_b survives");
        assert_eq!(provider_b.base_url, "https://router.example.com/v1");
        assert_eq!(
            provider_b.compatibility_for(AgentId::Codex),
            CompatibilityStatus::Incompatible
        );
    }

    #[test]
    fn one_damaged_profile_does_not_cost_the_user_the_others() {
        // The bad outcome this guards against: a single unreadable entry makes the
        // whole array fail to parse, and the user opens the app to an empty list
        // with all their providers apparently gone. Losing one is acceptable;
        // losing all of them is not.
        let (_dir, store) = temp_store();
        std::fs::write(
            store.path(),
            br#"{
              "version": 1,
              "profiles": [
                {
                  "id": "good-1", "name": "provider_a", "providerName": "provider_a",
                  "baseUrl": "https://gateway.example.com/v1",
                  "model": "example-model", "secretRef": "good-1",
                  "targets": ["claude"]
                },
                { "name": "broken", "targets": "not-an-array" },
                {
                  "id": "good-2", "name": "provider_b", "providerName": "provider_b",
                  "baseUrl": "https://router.example.com/v1",
                  "model": "vendor_a/example-model", "secretRef": "good-2",
                  "targets": ["codex"]
                }
              ]
            }"#,
        )
        .unwrap();

        let file = store.load().expect("the readable profiles must survive");
        assert_eq!(
            file.profiles.len(),
            2,
            "the two readable profiles must be kept, found {:?}",
            file.profiles.iter().map(|p| &p.name).collect::<Vec<_>>()
        );
        assert_eq!(file.profiles[0].name, "provider_a");
        assert_eq!(file.profiles[1].name, "provider_b");
        // The survivors keep their data intact.
        assert_eq!(file.profiles[0].base_url, "https://gateway.example.com/v1");
        assert_eq!(file.profiles[1].model, "vendor_a/example-model");
    }

    #[test]
    fn a_file_whose_profiles_are_all_damaged_is_reported_rather_than_shown_empty() {
        // Returning an empty list here would tell the user they have no providers,
        // when the truth is that their file cannot be read. The distinction matters
        // because the second case is recoverable from a backup.
        let (_dir, store) = temp_store();
        std::fs::write(
            store.path(),
            br#"{"version":1,"profiles":[{"name":"broken","targets":"nope"}]}"#,
        )
        .unwrap();

        let err = store
            .load()
            .expect_err("an unreadable file must be reported");
        assert_eq!(err.kind, crate::error::ErrorKind::ConfigInvalid);
    }

    #[test]
    fn a_profile_without_a_provider_name_borrows_its_label_from_the_name() {
        // A blank provider label in the list would look like corruption, so an older
        // file gets the name as its label instead.
        let (_dir, store) = temp_store();
        std::fs::write(
            store.path(),
            br#"{"version":1,"profiles":[{"id":"a","name":"provider_a",
                 "baseUrl":"https://example.com","model":"m"}]}"#,
        )
        .unwrap();

        let file = store.load().expect("loads");
        assert_eq!(file.profiles[0].provider_name, "provider_a");
    }

    #[test]
    fn a_profile_without_an_id_gets_one_so_it_stays_addressable() {
        // Without an identifier the profile could not be edited, deleted, or looked
        // up in the secret store, so a generated one is better than a broken entry.
        let (_dir, store) = temp_store();
        std::fs::write(
            store.path(),
            br#"{"version":1,"profiles":[{"name":"n","providerName":"p",
                 "baseUrl":"https://example.com","model":"m"}]}"#,
        )
        .unwrap();

        let file = store.load().expect("loads");
        assert!(
            !file.profiles[0].id.trim().is_empty(),
            "a generated id is required for the profile to be usable"
        );
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
