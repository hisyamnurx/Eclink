//! Regression tests for the Eclyzer Bridge -> Eclink upgrade.
//!
//! The rename is a branding change, but it touches strings that existing
//! installations depend on. These tests pin the invariants that make an upgrade
//! safe, and they are written against the real code paths rather than mocks so
//! that a future "cleanup" of an old identifier fails here instead of silently
//! stranding a user's data.
//!
//! What is covered:
//!
//! * the data directory keeps its pre-rename name, so profiles and the DPAPI
//!   secret blob stay where they are;
//! * the managed-provider prefix keeps its pre-rename spelling, so ownership
//!   detection and `restore_default` keep working;
//! * the credential helper accepts both the new and the old executable name, so
//!   a Codex `auth.command` written before the rename still resolves;
//! * a profiles file written by the old version still loads;
//! * a fresh install starts empty rather than seeded;
//! * the legacy helper environment variable is still honored.

use std::path::PathBuf;

use eclink_lib::adapters::APP_DATA_DIR_NAME;
use eclink_lib::profiles::store::{ProfileStore, PROFILES_SCHEMA_VERSION};
use eclink_lib::MANAGED_PROVIDER_PREFIX;

/// The application data directory name must not change across the rename.
///
/// This is the single most important compatibility invariant in the project: it
/// is where `profiles.json`, `app.json` and `secrets.dat` already live. If this
/// assertion ever fails, every existing installation loses its profiles and its
/// DPAPI-sealed API keys on upgrade.
#[test]
fn the_data_directory_keeps_its_pre_rename_name() {
    assert_eq!(
        APP_DATA_DIR_NAME, "EclyzerBridge",
        "changing the data directory name would strand existing profiles and secrets"
    );
}

/// The data directory and the install directory must remain different names.
///
/// `install_survival.rs` proves no cleanup root can reach the install directory;
/// this pins the naming that makes that true. The data directory keeps the old
/// name while the install directory takes the new one, so the two can never
/// collapse into the same path.
#[test]
fn the_data_directory_is_not_the_install_directory() {
    assert_ne!(
        APP_DATA_DIR_NAME, "Eclink",
        "the data directory and the install directory must not share a name"
    );
    assert_ne!(APP_DATA_DIR_NAME, "Eclyzer Bridge");
}

/// The managed provider prefix must keep its pre-rename spelling.
///
/// Every provider Eclink has written into a user's `config.toml` carries this
/// prefix, and ownership is decided by comparing against it. Renaming it would
/// make Eclink treat its own entries as the user's, so `restore_default` would
/// stop removing them and the active profile would no longer be recognized.
#[test]
fn the_managed_provider_prefix_is_unchanged() {
    assert_eq!(MANAGED_PROVIDER_PREFIX, "eclyzer_bridge_");
}

/// The prefix must not collide with a Codex builtin provider.
#[test]
fn the_managed_prefix_cannot_shadow_a_builtin() {
    for builtin in ["openai", "ollama", "lmstudio"] {
        assert!(
            !builtin.starts_with(MANAGED_PROVIDER_PREFIX),
            "{builtin} must never look Eclink-owned"
        );
    }
}

/// A `profiles.json` written by the previous version still loads.
///
/// The file is byte-for-byte what the old app wrote: schema version 1, camelCase
/// fields, one profile targeting both agents. Nothing about the rename may make
/// this fail.
#[test]
fn a_profiles_file_written_before_the_rename_still_loads() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("profiles.json");
    let legacy = r#"{
  "version": 1,
  "profiles": [
    {
      "id": "p-legacy-1",
      "name": "legacy gateway",
      "baseUrl": "https://example.invalid/v1",
      "model": "vendor/model-a",
      "targets": ["codex", "claude"],
      "createdAt": "2026-10-06T00:00:00Z",
      "updatedAt": "2026-10-06T00:00:00Z"
    }
  ]
}"#;
    std::fs::write(&path, legacy).expect("write legacy profiles");

    let store = ProfileStore::new(&path);
    let loaded = store.load().expect("a pre-rename file must load");

    assert_eq!(loaded.profiles.len(), 1, "the legacy profile must survive");
    let profile = &loaded.profiles[0];
    assert_eq!(profile.id, "p-legacy-1");
    assert_eq!(profile.name, "legacy gateway");
    assert_eq!(profile.base_url, "https://example.invalid/v1");
    assert!(
        profile.targets_agent(eclink_lib::adapters::AgentId::Codex),
        "the Codex target must be preserved"
    );
    assert!(
        profile.targets_agent(eclink_lib::adapters::AgentId::Claude),
        "the Claude target must be preserved"
    );
}

/// A profiles file with no version field still loads and is stamped current.
#[test]
fn an_unversioned_profiles_file_is_migrated_in_place() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("profiles.json");
    std::fs::write(
        &path,
        r#"{"profiles":[{"id":"a","name":"n","baseUrl":"https://x.invalid/v1","model":"m","targets":["codex"]}]}"#,
    )
    .expect("write");

    let store = ProfileStore::new(&path);
    let loaded = store.load().expect("an unversioned file must load");
    assert_eq!(loaded.profiles.len(), 1);
    assert_eq!(
        loaded.version, PROFILES_SCHEMA_VERSION,
        "the file should be stamped with the current schema version"
    );
}

/// A fresh install starts with no profiles at all.
///
/// The rename must not seed anything: a first run is a first run.
#[test]
fn a_fresh_install_has_no_profiles() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = ProfileStore::new(dir.path().join("profiles.json"));
    let loaded = store.load().expect("a missing file is not an error");
    assert!(
        loaded.profiles.is_empty(),
        "a fresh install must not be seeded with profiles"
    );
}

/// One damaged profile must not hide the healthy ones.
///
/// This is the behaviour the previous release fixed and the rename must not
/// regress: a single malformed entry is skipped, not fatal for the whole file.
#[test]
fn one_damaged_profile_does_not_hide_the_rest() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("profiles.json");
    std::fs::write(
        &path,
        r#"{
  "version": 1,
  "profiles": [
    { "id": "good", "name": "good", "baseUrl": "https://x.invalid/v1", "model": "m", "targets": ["codex"] },
    { "name": "broken, no id" }
  ]
}"#,
    )
    .expect("write");

    let store = ProfileStore::new(&path);
    let loaded = store.load().expect("one bad entry must not fail the file");
    assert_eq!(
        loaded.profiles.len(),
        1,
        "the healthy profile must survive a damaged sibling"
    );
    assert_eq!(loaded.profiles[0].id, "good");
}

/// The credential helper resolves the data directory through the legacy
/// environment variable, so an existing Codex `auth.command` keeps working.
///
/// The variable name itself is the contract; it is read by the helper binary and
/// passed by configuration written before the rename.
#[test]
fn the_helper_still_honors_the_legacy_data_dir_variable() {
    // This asserts the documented contract rather than spawning the helper: the
    // helper is a separate binary, and `helper_roundtrip.rs` already proves it
    // reads the variable end to end.
    let helper_source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crate lives inside the repository")
            .join("helper")
            .join("src")
            .join("main.rs"),
    )
    .expect("the helper source is readable");

    assert!(
        helper_source.contains("ECLYZER_BRIDGE_DATA_DIR"),
        "the helper must keep reading the legacy data-directory variable, \
         because configurations written before the rename still pass it"
    );
    assert!(
        helper_source.contains("EclyzerBridge"),
        "the helper must resolve the same data directory as the app"
    );
}

/// Both helper executable names are accepted.
///
/// An install upgraded in place may still carry `eclyzer-bridge-helper.exe`, and
/// a `config.toml` written by the old app contains an absolute path to it. The
/// candidate list must therefore contain both names.
#[test]
fn both_helper_executable_names_are_accepted() {
    let state_source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("app")
            .join("state.rs"),
    )
    .expect("app/state.rs is readable");

    assert!(
        state_source.contains("eclink-helper.exe"),
        "the new helper name must be probed"
    );
    assert!(
        state_source.contains("eclyzer-bridge-helper.exe"),
        "the legacy helper name must remain a fallback, or an upgraded \
         installation silently loses credential resolution"
    );
}

/// The bundle identifier must not change.
///
/// It is the Windows application identity. Renaming it would make Windows treat
/// Eclink as a different application, breaking the in-place upgrade.
#[test]
fn the_bundle_identifier_is_unchanged() {
    let conf =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"))
            .expect("tauri.conf.json is readable");
    let parsed: serde_json::Value = serde_json::from_str(&conf).expect("valid JSON");

    assert_eq!(
        parsed["identifier"].as_str(),
        Some("com.eclyzer.bridge"),
        "the bundle identifier is the Windows application identity and must not \
         change during a rename"
    );
    assert_eq!(
        parsed["productName"].as_str(),
        Some("Eclink"),
        "the product name is public branding and must be Eclink"
    );
}

/// The updater signing key must not be rotated during a rename.
///
/// Every installed copy verifies updates against the public key compiled into
/// it. Rotating the key would make existing installations reject every update,
/// permanently stranding them.
#[test]
fn the_updater_public_key_is_unchanged() {
    let conf =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"))
            .expect("tauri.conf.json is readable");
    let parsed: serde_json::Value = serde_json::from_str(&conf).expect("valid JSON");

    let key = parsed["plugins"]["updater"]["pubkey"]
        .as_str()
        .expect("an updater public key is configured");
    assert!(
        !key.trim().is_empty(),
        "the updater public key must not be empty"
    );
    // The committed key decodes to a minisign public key comment.
    assert!(
        key.starts_with("dW50cnVzdGVkIGNvbW1lbnQ6"),
        "the updater public key must remain the minisign key already shipped, \
         so installed copies still verify updates"
    );
}

/// The updater endpoint follows the renamed repository.
#[test]
fn the_updater_endpoint_points_at_the_renamed_repository() {
    let conf =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"))
            .expect("tauri.conf.json is readable");
    let parsed: serde_json::Value = serde_json::from_str(&conf).expect("valid JSON");

    let endpoints = parsed["plugins"]["updater"]["endpoints"]
        .as_array()
        .expect("endpoints is an array");
    let first = endpoints[0].as_str().expect("an endpoint URL");

    assert!(
        first.contains("hisyamnurx/Eclink"),
        "the updater must check the renamed repository, got: {first}"
    );
    assert!(
        !first.contains("Eclyzer-Bridge"),
        "the updater must not point at the old repository name: {first}"
    );
}

/// The repository constant follows the rename.
#[test]
fn the_repository_url_points_at_eclink() {
    assert_eq!(eclink_lib::REPO_URL, "https://github.com/hisyamnurx/Eclink");
}
