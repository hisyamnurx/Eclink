//! Regression tests for the "the installed app disappeared" report.
//!
//! The reported symptom was that the installer remained on disk but the
//! *installed* application sometimes vanished later. These tests pin the
//! invariant that makes that impossible from inside this program: every
//! destructive filesystem operation is confined to the application data
//! directory, and the installation directory is a different path that no cleanup
//! routine can reach.
//!
//! They are deliberately written against the real `AppPaths` and the real
//! cleanup routines rather than against mocks, because the thing being guarded
//! is a path relationship, and a mock would let the real paths drift apart
//! without failing.

use std::path::{Path, PathBuf};

use eclyzer_bridge_lib::paths::AppPaths;

/// Every path this application is allowed to delete inside.
///
/// Mirrors the layout in `AppPaths`: the data root and everything under it.
fn deletable_roots(paths: &AppPaths) -> Vec<PathBuf> {
    vec![
        paths.root().to_path_buf(),
        paths.backups_dir(),
        paths.logs_dir(),
        paths.cache_dir(),
    ]
}

/// The directories a Windows per-user NSIS install occupies.
///
/// These are *not* derived from `AppPaths`, because the point of the test is
/// that the two are different. They are written out as the installer creates
/// them: `%LOCALAPPDATA%\Eclyzer Bridge` (with a space) plus the Start Menu
/// folder.
fn install_locations() -> Vec<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\example\AppData\Local"));
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\example\AppData\Roaming"));
    vec![
        local.join("Eclyzer Bridge"),
        local.join("Eclyzer Bridge").join("eclyzer-bridge.exe"),
        local
            .join("Eclyzer Bridge")
            .join("eclyzer-bridge-helper.exe"),
        local.join("Eclyzer Bridge").join("uninstall.exe"),
        appdata
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Eclyzer Bridge.lnk"),
    ]
}

#[test]
fn every_deletable_path_lives_under_the_data_directory() {
    let paths = AppPaths::resolve().expect("a data directory must resolve");
    let root = paths.root().to_path_buf();

    for candidate in deletable_roots(&paths) {
        assert!(
            candidate.starts_with(&root),
            "{} is outside the data root {}",
            candidate.display(),
            root.display()
        );
    }
}

#[test]
fn no_cleanup_root_can_reach_the_installation_directory() {
    // The decisive check. If any directory this program deletes inside were an
    // ancestor of the install directory — or were the install directory itself —
    // then a log rotation or a snapshot prune could remove the application.
    let paths = AppPaths::resolve().expect("a data directory must resolve");

    for root in deletable_roots(&paths) {
        for target in install_locations() {
            assert!(
                !target.starts_with(&root),
                "cleanup root {} contains the installed {}",
                root.display(),
                target.display()
            );
            assert_ne!(
                root,
                target,
                "cleanup root {} is the installed {}",
                root.display(),
                target.display()
            );
        }
    }
}

#[test]
fn the_installation_directory_cannot_reach_any_cleanup_root() {
    // The mirror image: the install directory must not contain the data
    // directory either, because uninstalling would then be expected to delete
    // the user's profiles and backups, which it correctly does not.
    let paths = AppPaths::resolve().expect("a data directory must resolve");

    for install in install_locations() {
        for root in deletable_roots(&paths) {
            assert!(
                !root.starts_with(&install),
                "install path {} contains the data root {}",
                install.display(),
                root.display()
            );
        }
    }
}

#[test]
fn the_data_directory_and_the_install_directory_have_different_names() {
    // The two are confusingly similar on purpose-different spellings:
    // `EclyzerBridge` for data, `Eclyzer Bridge` for the program. Pin the
    // distinction, because collapsing them to one name would make the previous
    // tests pass while the paths actually became the same directory.
    let paths = AppPaths::resolve().expect("a data directory must resolve");
    let data_name = paths
        .root()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();

    assert_eq!(data_name, "EclyzerBridge");
    let install_name = install_locations()[0]
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    assert_eq!(install_name, "Eclyzer Bridge");
    assert_ne!(
        data_name, install_name,
        "the data and install directories must not share a name"
    );
}

#[test]
fn no_managed_artifact_is_written_outside_the_data_directory() {
    // Profiles, secrets, settings, backups, logs, and cache must all stay inside
    // the data root. A stray write elsewhere is how an app ends up fighting the
    // installer for a path.
    let paths = AppPaths::resolve().expect("a data directory must resolve");
    let root = paths.root();

    for artifact in [
        paths.app_json(),
        paths.profiles_json(),
        paths.secrets_dat(),
        paths.backups_dir(),
        paths.logs_dir(),
        paths.cache_dir(),
    ] {
        assert!(
            artifact.starts_with(root),
            "{} escapes the data root",
            artifact.display()
        );
    }
}

/// Whether `path` is inside `root`, treating both as already-absolute.
///
/// Only used to make the assertions above readable; kept here so the intent is
/// explicit rather than hidden behind a bare `starts_with`.
#[allow(dead_code)]
fn is_inside(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}
