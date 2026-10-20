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

use eclink_lib::paths::AppPaths;

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
/// them: `%LOCALAPPDATA%\Eclink` plus the Start Menu folder.
///
/// The install directory is named after the product, so it changed with the
/// rename. The *data* directory deliberately did not; that separation is what
/// keeps a cleanup routine from reaching an installed binary.
fn install_locations() -> Vec<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\example\AppData\Local"));
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\example\AppData\Roaming"));
    vec![
        local.join("Eclink"),
        local.join("Eclink").join("eclink.exe"),
        local.join("Eclink").join("eclink-helper.exe"),
        local.join("Eclink").join("uninstall.exe"),
        appdata
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Eclink.lnk"),
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
    // After the rename the two names differ for a reason worth pinning: the data
    // directory keeps the pre-rename spelling `EclyzerBridge` so existing
    // profiles and secrets stay put, while the install directory takes the new
    // product name `Eclink`. Collapsing them to one name would make the previous
    // tests pass while the paths actually became the same directory.
    let paths = AppPaths::resolve().expect("a data directory must resolve");
    let data_name = paths
        .root()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();

    assert_eq!(
        data_name, "EclyzerBridge",
        "the data directory must keep its pre-rename name, or an upgrade strands \
         every existing profile and API key"
    );
    let install_name = install_locations()[0]
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    assert_eq!(install_name, "Eclink");
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

// ---------------------------------------------------------------------------
// What ends up inside the installer.
//
// This is here because a real mistake happened: a one-shot maintenance binary was
// added under `src/bin/`, and Tauri bundles every binary cargo produces. It
// shipped inside the installer without appearing anywhere in
// `bundle.resources`, so reading the configuration did not reveal it — only
// listing the installed directory did.
// ---------------------------------------------------------------------------

#[test]
fn only_the_expected_binaries_live_in_the_crate_binary_directory() {
    // Tauri bundles whatever `src/bin/*.rs` produces. Anything placed there is
    // shipped to users, so the directory must stay empty unless a binary is
    // genuinely part of the application.
    let bin_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("bin");
    if !bin_dir.exists() {
        return;
    }
    let stray: Vec<String> = std::fs::read_dir(&bin_dir)
        .expect("read src/bin")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        stray.is_empty(),
        "src/bin/ contains {stray:?}; Tauri bundles these into the installer. \
         Put maintenance tools in examples/ instead."
    );
}

#[test]
fn maintenance_tools_live_in_examples_not_in_the_bundled_binaries() {
    // The tool must still exist and still be buildable, just not shipped.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(
        root.join("examples")
            .join("fix-claude-settings.rs")
            .exists(),
        "the maintenance tool was deleted rather than relocated"
    );
    assert!(
        !root
            .join("src")
            .join("bin")
            .join("fix-claude-settings.rs")
            .exists(),
        "the maintenance tool is back in the bundled binary directory"
    );
}

// ---------------------------------------------------------------------------
// The icon embedded in the executable.
//
// These exist because of a real failure that produced no error anywhere. The logo
// was replaced and the icon set regenerated, and every subsequent build still
// embedded the *earlier* artwork: `tauri-build` compiles the icons into a Windows
// resource, cargo cached the build script's result, and nothing declared an icon
// as an input. The app built, installed, and ran with the wrong icon in the title
// bar, taskbar, Desktop shortcut, Start Menu, and installer.
//
// It was found by extracting the embedded PNGs from the built executable and
// comparing their pixel bounding boxes against `src-tauri/icons` - the executable
// held a full-square dark image where the repository had a wide blue logo.
//
// A unit test cannot inspect a built binary, but it CAN assert the mechanism that
// prevents the recurrence, plus the completeness of the icon set. That is what
// follows.
// ---------------------------------------------------------------------------

#[test]
fn the_build_script_declares_the_icons_as_build_inputs() {
    let build_rs = Path::new(env!("CARGO_MANIFEST_DIR")).join("build.rs");
    let source = std::fs::read_to_string(&build_rs).expect("build.rs is readable");

    assert!(
        source.contains("cargo:rerun-if-changed=icons"),
        "build.rs does not declare the icon directory as a build input. Replacing an \
         icon would leave cargo's cached Windows resource in place, so the executable \
         would keep the old artwork with no error reported anywhere."
    );
    assert!(
        source.contains("cargo:rerun-if-changed=tauri.conf.json"),
        "build.rs does not declare tauri.conf.json as a build input, so changing which \
         icons the bundle references would not trigger a rebuild"
    );
    // The directory entry alone only covers the directory's own mtime, which does
    // not change when a file inside it is edited, so per-file entries are needed.
    assert!(
        source.contains("read_dir"),
        "build.rs declares the icon directory but not the files inside it; editing an \
         existing icon would not change the directory's timestamp and the rebuild \
         would be skipped"
    );
}

#[test]
fn the_icon_set_covers_every_size_windows_asks_for() {
    // Windows requests specific sizes: 16 px in the title bar, 32 px in the taskbar
    // and Alt-Tab, 48 px in Explorer's medium view, 256 px for large thumbnails and
    // the installer. A set missing any of them is scaled up from a smaller image
    // and looks soft exactly where a user notices.
    let icons = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons");
    let ico_path = icons.join("icon.ico");
    assert!(ico_path.exists(), "icon.ico is missing");

    let bytes = std::fs::read(&ico_path).expect("icon.ico is readable");
    assert!(bytes.len() > 1024, "icon.ico is implausibly small");
    // Every Windows .ico opens with a 6-byte header: reserved(2), type(2), count(2).
    assert_eq!(
        &bytes[0..2],
        &[0, 0],
        "icon.ico is not an icon (bad reserved field)"
    );
    assert_eq!(&bytes[2..4], &[1, 0], "icon.ico is a cursor, not an icon");
    let image_count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    assert!(
        image_count >= 5,
        "icon.ico holds only {image_count} sizes; Windows needs a multi-resolution icon"
    );

    // The PNG sizes the bundle names in tauri.conf.json must exist too.
    for name in ["32x32.png", "128x128.png", "128x128@2x.png"] {
        let path = icons.join(name);
        assert!(path.exists(), "{name} is missing from the icon set");
        let head = std::fs::read(&path).expect("icon is readable");
        assert_eq!(
            &head[0..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            "{name} is not a PNG"
        );
    }
}

#[test]
fn the_bundle_references_an_installer_icon_and_the_window_icon_set() {
    // A configured installer icon that does not exist, or a window icon list that
    // omits the .ico, means Windows falls back to a generic icon on the shortcut
    // and in Add/Remove Programs.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config =
        std::fs::read_to_string(root.join("tauri.conf.json")).expect("tauri.conf.json is readable");
    let parsed: serde_json::Value =
        serde_json::from_str(&config).expect("tauri.conf.json is valid JSON");

    let bundle_icons = parsed["bundle"]["icon"]
        .as_array()
        .expect("bundle.icon is an array");
    assert!(
        bundle_icons
            .iter()
            .any(|v| v.as_str().is_some_and(|s| s.ends_with(".ico"))),
        "bundle.icon does not include an .ico, so Windows has no multi-size icon"
    );
    for icon in bundle_icons {
        let relative = icon.as_str().expect("icon entries are strings");
        assert!(
            root.join(relative).exists(),
            "bundle.icon names {relative}, which does not exist"
        );
    }

    let installer_icon = parsed["bundle"]["windows"]["nsis"]["installerIcon"]
        .as_str()
        .expect("nsis.installerIcon is set");
    assert!(
        root.join(installer_icon).exists(),
        "nsis.installerIcon names {installer_icon}, which does not exist"
    );
}

#[test]
fn the_bundle_helper_is_built_in_release_before_the_bundle() {
    // `tauri.conf.json` bundles the credential helper from `target/release/`, and
    // Tauri fails the whole build if that file is absent:
    //
    //     resource path `..\target\release\eclyzer-bridge-helper.exe` doesn't exist
    //
    // That ordering is easy to get wrong. Building the app from a clean checkout
    // fails with an error that names a *build artefact* rather than the missing
    // step, and it passes again as soon as a previous release build has left the
    // helper behind - so the mistake is invisible on a machine that has built
    // before and only bites on a fresh clone or in CI.
    //
    // The release workflow already builds it first. This asserts that the local
    // npm entry point does too, because that is the one a contributor runs.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives inside the repository")
        .to_path_buf();

    let package =
        std::fs::read_to_string(root.join("package.json")).expect("package.json readable");
    let parsed: serde_json::Value =
        serde_json::from_str(&package).expect("package.json is valid JSON");
    let scripts = &parsed["scripts"];

    let build = scripts["tauri:build"]
        .as_str()
        .expect("the tauri:build script exists");
    assert!(
        build.contains("helper:build"),
        "`npm run tauri:build` does not build the helper first, so a clean checkout \
         would fail with \"resource path ... doesn't exist\". The script is: {build}"
    );
    // The helper must be a release build: the bundle reads from `target/release`.
    let helper = scripts["helper:build"]
        .as_str()
        .expect("the helper:build script exists");
    assert!(
        helper.contains("--release"),
        "the helper is not built in release mode, but the bundle reads it from \
         target/release. The script is: {helper}"
    );
    assert!(
        helper.contains("eclink-helper"),
        "the helper:build script does not name the helper crate: {helper}"
    );

    // And the configuration must actually point at the path the script produces.
    let config = std::fs::read_to_string(root.join("src-tauri").join("tauri.conf.json"))
        .expect("tauri.conf.json readable");
    let conf: serde_json::Value =
        serde_json::from_str(&config).expect("tauri.conf.json is valid JSON");
    let resources = conf["bundle"]["resources"]
        .as_object()
        .expect("bundle.resources is an object");
    assert!(
        resources
            .keys()
            .any(|k| k.contains("eclink-helper.exe") && k.contains("release")),
        "bundle.resources does not read the helper from a release path: {resources:?}"
    );
}
