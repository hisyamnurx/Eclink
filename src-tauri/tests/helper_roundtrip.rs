//! Verifies the GUI and the helper agree on the on-disk secret format, and
//! that a real DPAPI round-trip works between the two binaries.

use std::process::Command;

use eclyzer_bridge_lib::secrets::SecretStore;

#[test]
fn helper_resolves_a_secret_written_by_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    // Write with the same DPAPI backend the application uses.
    let store = SecretStore::new(root.join("secrets.dat"));
    store
        .put("profile-abc", "sk-roundtrip-secret-0123456789")
        .expect("DPAPI seal must succeed for the current user");

    // Locate the helper built by `cargo build --release -p eclyzer-bridge-helper`.
    let helper = helper_path().expect("helper binary must exist; build it first");
    let output = Command::new(&helper)
        .args(["secret", "resolve", "profile-abc"])
        .env("ECLYZER_BRIDGE_DATA_DIR", &root)
        .output()
        .expect("helper must run");

    assert!(
        output.status.success(),
        "helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "sk-roundtrip-secret-0123456789"
    );
    // stderr must never carry the secret.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("sk-roundtrip"), "stderr leaked the secret: {stderr}");
}

#[test]
fn helper_fails_closed_for_an_unknown_profile() {
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::new(dir.path().join("secrets.dat"));
    store.put("known", "value-0123456789").unwrap();

    let helper = helper_path().expect("helper binary must exist");
    let output = Command::new(&helper)
        .args(["secret", "resolve", "unknown-profile"])
        .env("ECLYZER_BRIDGE_DATA_DIR", dir.path())
        .output()
        .expect("helper must run");

    assert!(!output.status.success(), "unknown profile must fail");
    assert!(output.stdout.is_empty(), "no credential may be emitted");
}

#[test]
fn helper_emits_no_output_without_arguments() {
    let helper = helper_path().expect("helper binary must exist");
    let output = Command::new(&helper).output().expect("helper must run");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

/// Resolve the helper from the workspace `target` directory.
fn helper_path() -> Option<std::path::PathBuf> {
    // CARGO_MANIFEST_DIR is `<repo>/src-tauri`; the workspace target dir is one
    // level up.
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.parent()?;
    for profile in ["release", "debug"] {
        let candidate = root
            .join("target")
            .join(profile)
            .join("eclyzer-bridge-helper.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
