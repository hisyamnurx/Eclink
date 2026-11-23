//! End-to-end tests for per-agent install locations.
//!
//! These run against real temporary directories and real Windows shims rather
//! than mocks. The feature's promise is about what happens to a user's machine -
//! an agent installed outside PATH is found, a location that disappears is
//! reported instead of silently falling back - so the tests create the files and
//! remove them, and assert on what detection and the settings file then say.
//!
//! No test contacts a network, starts a real coding agent, or touches the user's
//! own Codex/Claude/OMP configuration: every adapter here is pointed at a
//! temporary launcher, and none of them apply a profile.

use std::path::{Path, PathBuf};

use eclink_lib::adapters::locate::{
    self, AgentLocation, AgentLocations, LocationKind, LocationStore,
};
use eclink_lib::adapters::{
    AgentAdapter, AgentId, ClaudeAdapter, CodexAdapter, LocationMode, LocationProblem, OmpAdapter,
    OmpSelection,
};
use eclink_lib::app::AppState;
use eclink_lib::backup::BackupManager;
use eclink_lib::paths::AppPaths;

/// Create a `.cmd` shim that prints `line` and nothing else on stdout.
///
/// `@echo off` matters: without it cmd echoes the command itself, which would
/// make a version line ambiguous.
fn shim(dir: &Path, name: &str, line: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("@echo off\r\necho {line}\r\n")).unwrap();
    path
}

fn configured(path: &Path, kind: LocationKind) -> AgentLocation {
    AgentLocation::new(path.display().to_string(), kind)
}

fn store(agent: AgentId, location: AgentLocation) -> LocationStore {
    let mut locations = AgentLocations::default();
    locations.set(agent, Some(location));
    LocationStore::new(locations)
}

#[test]
fn an_agent_installed_outside_path_is_found_through_its_folder() {
    let dir = tempfile::tempdir().unwrap();
    let expected = shim(
        &dir.path().join("codex tools"),
        "codex.cmd",
        "codex-cli 9.9.9",
    );

    let adapter = CodexAdapter::with_locations(
        BackupManager::new(dir.path().join("backups")),
        store(
            AgentId::Codex,
            configured(&dir.path().join("codex tools"), LocationKind::Folder),
        ),
    );

    let detection = adapter.detect().unwrap();

    assert!(detection.installed);
    assert_eq!(detection.version.as_deref(), Some("9.9.9"));
    assert_eq!(
        detection.executable.as_deref(),
        Some(expected.display().to_string().as_str())
    );
    assert_eq!(detection.location_mode, Some(LocationMode::Folder));
    assert!(detection.location_problem.is_none());
}

#[test]
fn a_launcher_in_a_path_with_spaces_and_unicode_is_accepted() {
    // The path a naive quoting implementation breaks on. Reaching the version
    // proves the shim was executed, not merely found.
    let dir = tempfile::tempdir().unwrap();
    let launcher = shim(
        &dir.path().join("Program Files O'Brien Ünïcødé"),
        "claude.cmd",
        "2.0.0 (Claude Code)",
    );

    let adapter = ClaudeAdapter::with_locations(
        BackupManager::new(dir.path().join("backups")),
        store(
            AgentId::Claude,
            configured(&launcher, LocationKind::Launcher),
        ),
    );

    let detection = adapter.detect().unwrap();

    assert!(detection.installed);
    assert_eq!(detection.version.as_deref(), Some("2.0.0"));
    assert_eq!(detection.location_mode, Some(LocationMode::Launcher));
}

#[test]
fn a_launcher_that_moved_after_it_was_saved_is_reported_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    let launcher = shim(dir.path(), "codex.cmd", "codex-cli 9.9.9");

    let adapter = CodexAdapter::with_locations(
        BackupManager::new(dir.path().join("backups")),
        store(
            AgentId::Codex,
            configured(&launcher, LocationKind::Launcher),
        ),
    );
    assert!(adapter.detect().unwrap().installed, "it starts out valid");

    std::fs::remove_file(&launcher).unwrap();

    let detection = adapter.detect().unwrap();

    assert!(!detection.installed, "a vanished launcher is not installed");
    assert_eq!(detection.location_problem, Some(LocationProblem::Missing));
    assert_eq!(
        detection.configured_location.as_deref(),
        Some(launcher.display().to_string().as_str())
    );
    assert!(
        detection
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("no longer exists"),
        "the note explains the problem: {:?}",
        detection.note
    );
}

#[test]
fn a_folder_without_a_supported_launcher_reports_the_choice_not_a_path_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("empty install");
    std::fs::create_dir_all(&folder).unwrap();

    // Automatic detection in this build could well succeed (a developer machine
    // often has Codex), so a fallback would show up as `installed: true` with a
    // different executable. Assert it does not.
    let adapter = CodexAdapter::with_locations(
        BackupManager::new(dir.path().join("backups")),
        store(AgentId::Codex, configured(&folder, LocationKind::Folder)),
    );

    let detection = adapter.detect().unwrap();

    assert!(!detection.installed);
    assert_eq!(
        detection.location_problem,
        Some(LocationProblem::NotALauncher)
    );
    assert!(detection.executable.is_none());
}

#[test]
fn a_saved_location_takes_priority_over_automatic_detection() {
    let dir = tempfile::tempdir().unwrap();
    let manual = shim(dir.path(), "codex.cmd", "codex-cli 9.9.9");
    let automatic = shim(&dir.path().join("other"), "codex.cmd", "codex-cli 1.0.0");

    let resolution = locate::resolve(
        AgentId::Codex,
        Some(&configured(&manual, LocationKind::Launcher)),
        || Some(automatic),
    );

    assert_eq!(
        resolution
            .launcher
            .map(|launcher| launcher.path().to_path_buf()),
        Some(manual),
        "the saved launcher wins even when PATH has another one"
    );
    assert_eq!(resolution.mode, LocationMode::Launcher);
}

#[test]
fn clearing_a_location_restores_automatic_detection() {
    let locations = store(
        AgentId::Codex,
        AgentLocation::new(r"C:\does\not\matter", LocationKind::Folder),
    );
    locations.set(AgentId::Codex, None);

    let auto = PathBuf::from(r"C:\auto\codex.exe");
    let resolution = locations.resolve(AgentId::Codex, || Some(auto.clone()));

    assert_eq!(resolution.mode, LocationMode::Automatic);
    assert_eq!(
        resolution
            .launcher
            .map(|launcher| launcher.path().to_path_buf()),
        Some(auto)
    );
}

#[test]
fn oh_my_pi_uses_the_saved_launcher_and_keeps_its_version_gate() {
    let dir = tempfile::tempdir().unwrap();
    let launcher = shim(&dir.path().join("omp tools"), "omp.cmd", "omp 18.1.0");

    let adapter = OmpAdapter::with_locations(
        BackupManager::new(dir.path().join("backups")),
        dir.path().join("ledger.json"),
        OmpSelection::new("default"),
        store(AgentId::Omp, configured(&launcher, LocationKind::Launcher)),
    );

    let detection = adapter.detect().unwrap();

    assert!(detection.installed);
    assert_eq!(detection.version.as_deref(), Some("18.1.0"));
    assert_eq!(
        detection.executable.as_deref(),
        Some(launcher.display().to_string().as_str())
    );
    // 18.1.0 is not older than the 18.0.0 gate, so no upgrade note appears.
    assert!(
        !detection
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("older than"),
        "unexpected version note: {:?}",
        detection.note
    );
}

#[test]
fn one_agents_broken_location_does_not_affect_another() {
    let dir = tempfile::tempdir().unwrap();
    let good = shim(&dir.path().join("claude tools"), "claude.cmd", "2.0.0");
    let missing = dir.path().join("gone").join("codex.exe");

    let codex = CodexAdapter::with_locations(
        BackupManager::new(dir.path().join("backups")),
        store(AgentId::Codex, configured(&missing, LocationKind::Launcher)),
    );
    let claude = ClaudeAdapter::with_locations(
        BackupManager::new(dir.path().join("backups")),
        store(AgentId::Claude, configured(&good, LocationKind::Launcher)),
    );

    let codex_detection = codex.detect().unwrap();
    let claude_detection = claude.detect().unwrap();

    assert!(!codex_detection.installed);
    assert_eq!(
        codex_detection.location_problem,
        Some(LocationProblem::Missing)
    );
    assert!(claude_detection.installed, "Claude is unaffected");
    assert!(claude_detection.location_problem.is_none());
}

#[test]
fn launch_uses_the_saved_launcher() {
    // Detection, probing, and Launch must name the same file. The plan is built
    // without starting a terminal, so the assertion is on the argv it would run.
    let dir = tempfile::tempdir().unwrap();
    let launcher = shim(
        &dir.path().join("codex bin"),
        "codex.cmd",
        "codex-cli 9.9.9",
    );
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let adapter = CodexAdapter::with_locations(
        BackupManager::new(dir.path().join("backups")),
        store(
            AgentId::Codex,
            configured(&launcher, LocationKind::Launcher),
        ),
    );

    let plan = adapter.launch_argv(&project).unwrap();

    assert!(
        plan.args
            .iter()
            .any(|arg| arg.contains("codex bin") && arg.contains("codex.cmd")),
        "the saved launcher must appear in argv: {:?}",
        plan.args
    );
    assert!(
        plan.display.contains(&launcher.display().to_string()),
        "the preview must show the saved launcher: {}",
        plan.display
    );
}

#[test]
fn oh_my_pi_launch_keeps_profile_arguments_with_a_saved_launcher() {
    let dir = tempfile::tempdir().unwrap();
    let launcher = shim(&dir.path().join("omp bin"), "omp.cmd", "omp 18.1.0");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let adapter = OmpAdapter::with_locations(
        BackupManager::new(dir.path().join("backups")),
        dir.path().join("ledger.json"),
        OmpSelection::new("work"),
        store(AgentId::Omp, configured(&launcher, LocationKind::Launcher)),
    );

    let plan = adapter.launch_argv(&project).unwrap();

    // Windows Terminal receives `--profile` and `work` as two argv entries;
    // the PowerShell fallback embeds them, quoted, in its script. Both must name
    // the same profile and the saved launcher.
    let joined = plan.args.join(" ");
    assert!(
        joined.contains("--profile") && joined.contains("work"),
        "the profile arguments were dropped: {:?}",
        plan.args
    );
    assert!(
        plan.display.contains("--profile work"),
        "the preview must show the profile arguments: {}",
        plan.display
    );
    assert!(
        joined.contains("omp.cmd"),
        "the saved launcher must appear in argv: {:?}",
        plan.args
    );
}

#[test]
fn locations_persist_per_agent_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::with_root(dir.path());
    let codex_location = AgentLocation::new(r"D:\Tools\codex", LocationKind::Folder);
    let omp_location = AgentLocation::new(r"D:\Tools\omp\omp.exe", LocationKind::Launcher);

    {
        let state = AppState::with_paths(paths.clone()).unwrap();
        state
            .update_settings(|settings| {
                settings.language = "id".to_string();
                settings.first_run_complete = true;
            })
            .unwrap();
        state
            .set_agent_location(AgentId::Codex, Some(codex_location.clone()))
            .unwrap();
        state
            .set_agent_location(AgentId::Omp, Some(omp_location.clone()))
            .unwrap();
    }

    // A new AppState reads app.json from disk: this is the restart.
    let restarted = AppState::with_paths(paths.clone()).unwrap();
    let settings = restarted.settings();

    assert_eq!(settings.agent_locations.codex, Some(codex_location));
    assert_eq!(settings.agent_locations.omp, Some(omp_location));
    assert!(settings.agent_locations.claude.is_none());
    assert_eq!(settings.language, "id", "other settings survive too");
    assert!(settings.first_run_complete);

    // Clearing one agent leaves the other and every unrelated setting alone.
    restarted.set_agent_location(AgentId::Codex, None).unwrap();
    let after = AppState::with_paths(paths).unwrap().settings();

    assert!(after.agent_locations.codex.is_none());
    assert!(after.agent_locations.omp.is_some(), "OMP must be kept");
    assert_eq!(after.language, "id");
}

#[test]
fn an_invalid_saved_location_is_rejected_before_it_is_stored() {
    for bad in [
        AgentLocation::new("Tools\\codex", LocationKind::Folder),
        AgentLocation::new("", LocationKind::Launcher),
        AgentLocation::new(r"C:\Tools\codex.txt", LocationKind::Launcher),
    ] {
        let error = locate::validate_location(bad.clone()).unwrap_err();
        assert_eq!(
            error.kind,
            eclink_lib::error::ErrorKind::Validation,
            "{bad:?} should be refused"
        );
        assert!(
            error.hint.is_some(),
            "a refusal must say what to do instead"
        );
    }
}
