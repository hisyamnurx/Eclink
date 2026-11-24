//! Launch regressions: the selected launcher must actually run.
//!
//! These tests start real processes. They do not open a paid provider, touch the
//! user's agent configuration, or depend on an installed coding agent: every
//! "agent" is a temporary `.cmd` fixture that records its working directory and
//! arguments, then exits.
//!
//! The point is to test the seam the unit tests cannot: the command Eclink hands
//! to a terminal (or to `cmd.exe` directly) is executable, arrives with its
//! arguments intact, and runs in the project folder rather than the folder the
//! launcher lives in.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use eclink_lib::adapters::detect::{probe_launcher_with_timeout, ProbeError};
use eclink_lib::adapters::locate::Launcher;
use eclink_lib::adapters::AgentId;
use eclink_lib::process::{build_launch_plan_for_command, spawn_launch, TerminalKind};

/// A shim that records `%CD%` and `%*` next to itself, then exits.
///
/// `%CD%` is quoted in the echo because a project path may contain `&`, which
/// cmd would otherwise treat as a command separator after expansion.
fn write_shim(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("agent O'Brien.cmd");
    std::fs::write(
        &path,
        "@echo off\r\n\
         echo cwd=\"%CD%\">\"%~dp0ran.txt\"\r\n\
         echo args=%*>>\"%~dp0ran.txt\"\r\n",
    )
    .unwrap();
    path
}

fn wait_for_fixture(path: &Path) -> String {
    wait_until(path, "the fixture never ran", |text| text.contains("args="))
}

/// Wait for any non-empty file, used by the native-launcher fixtures.
fn wait_for_text(path: &Path) -> String {
    wait_until(path, "the native fixture never ran", |text| {
        !text.trim().is_empty()
    })
}

fn wait_until(path: &Path, what: &str, ready: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(path) {
            if ready(&text) {
                return text;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{what}; {} was not written", path.display());
}

fn comspec() -> PathBuf {
    std::env::var_os("ComSpec")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cmd.exe"))
}

fn powershell() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe")
}

fn windows_terminal() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let candidate = PathBuf::from(local)
        .join("Microsoft")
        .join("WindowsApps")
        .join("wt.exe");
    candidate.is_file().then_some(candidate)
}

fn kill_tree(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn assert_fixture_recorded(text: &str, project: &Path) {
    let expected = format!(
        "cwd=\"{}\"",
        eclink_lib::process::strip_verbatim_prefix(project.canonicalize().unwrap()).display()
    );
    assert!(
        text.contains(&expected),
        "cwd was not the project; expected {expected}; got {text}"
    );
    for token in ["--profile", "work", "--model", "p/m"] {
        assert!(text.contains(token), "argument {token} was mangled: {text}");
    }
}

#[test]
fn the_shim_command_runs_with_literal_arguments_in_the_project_directory() {
    // The command text probe, registry validation, and Launch all build. This
    // executes it directly, which is the only part PowerShell transports.
    let dir = tempfile::tempdir().unwrap();
    let shim = write_shim(dir.path());
    let project = dir.path().join("My Project O'Brien & Sons");
    std::fs::create_dir_all(&project).unwrap();

    let command = Launcher::from_path(shim)
        .command(&["--profile", "work", "--model", "p/m"])
        .unwrap();
    let raw = command
        .raw_command_line
        .as_deref()
        .expect("a shim has a raw command line");

    let mut process = std::process::Command::new(&command.program);
    process
        .raw_arg(raw)
        .current_dir(&project)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = process.spawn().unwrap().wait().unwrap();

    assert!(status.success(), "the shim exited with {status}");
    let text = wait_for_fixture(&dir.path().join("ran.txt"));
    assert_fixture_recorded(&text, &project);
}

#[test]
fn powershell_fallback_launch_runs_the_selected_shim() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_shim(dir.path());
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let command = Launcher::from_path(shim)
        .command(&["--profile", "work", "--model", "p/m"])
        .unwrap();
    let plan = build_launch_plan_for_command(
        AgentId::Codex,
        &command,
        &project,
        TerminalKind::PowerShell,
        &powershell(),
    )
    .unwrap();

    // Diagnostic: run the same argv without the detached flags first.
    let pid = spawn_launch(&plan, &[]).unwrap();
    let text = wait_for_fixture(&dir.path().join("ran.txt"));
    kill_tree(pid);
    assert_fixture_recorded(&text, &project);
}

#[test]
fn windows_terminal_launch_runs_the_selected_shim() {
    // Skipped where Windows Terminal is not installed (some CI images). The tab
    // closes itself because `cmd.exe /c` finishes with the fixture.
    let Some(wt) = windows_terminal() else {
        eprintln!("skipped: Windows Terminal is not installed on this machine");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let shim = write_shim(dir.path());
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let command = Launcher::from_path(shim)
        .command(&["--profile", "work", "--model", "p/m"])
        .unwrap();
    let plan = build_launch_plan_for_command(
        AgentId::Codex,
        &command,
        &project,
        TerminalKind::WindowsTerminal,
        &wt,
    )
    .unwrap();

    // `spawn_launch` is the production entry point. It may return the pid of a
    // short-lived client; the fixture file, not the pid, is the proof.
    let _pid = spawn_launch(&plan, &[]).unwrap();
    let text = wait_for_fixture(&dir.path().join("ran.txt"));

    assert_fixture_recorded(&text, &project);
}

#[test]
fn powershell_fallback_launch_runs_a_native_launcher() {
    // `cmd.exe` is a stand-in for a native agent executable: the transport is
    // what is under test, and `cd` proves the working directory the agent sees.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let marker = dir.path().join("native.txt");

    let command = Launcher::from_path(comspec())
        .command(&["/c", "cd", ">", &marker.display().to_string()])
        .unwrap();
    let plan = build_launch_plan_for_command(
        AgentId::Claude,
        &command,
        &project,
        TerminalKind::PowerShell,
        &powershell(),
    )
    .unwrap();

    let pid = spawn_launch(&plan, &[]).unwrap();
    let text = wait_for_text(&marker);
    kill_tree(pid);

    assert!(
        text.contains("project"),
        "the native launcher did not run in the project: {text}"
    );
}

#[test]
fn windows_terminal_launch_runs_a_native_launcher() {
    let Some(wt) = windows_terminal() else {
        eprintln!("skipped: Windows Terminal is not installed on this machine");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let marker = dir.path().join("native.txt");

    let command = Launcher::from_path(comspec())
        .command(&["/c", "cd", ">", &marker.display().to_string()])
        .unwrap();
    let plan = build_launch_plan_for_command(
        AgentId::Claude,
        &command,
        &project,
        TerminalKind::WindowsTerminal,
        &wt,
    )
    .unwrap();

    let _pid = spawn_launch(&plan, &[]).unwrap();
    let text = wait_for_text(&marker);

    assert!(
        text.contains("project"),
        "the native launcher did not run in the project: {text}"
    );
}

#[tokio::test]
async fn a_timed_out_shim_is_killed_but_its_descendants_may_survive() {
    // The documented boundary: `kill_on_drop` kills the `cmd.exe` Eclink started.
    // A process that batch file started with `start /b` is not in a job object,
    // so it can outlive the timeout. This test records that boundary instead of
    // claiming a guarantee the implementation does not provide.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let shim = dir.path().join("slow.cmd");
    std::fs::write(
        &shim,
        format!(
            "@echo off\r\n\
             cd /d \"{}\"\r\n\
             start \"\" /b cmd /c \"ping -n 5 127.0.0.1 >nul & echo alive>child.txt\"\r\n\
             ping -n 30 127.0.0.1 >nul\r\n\
             echo parent>parent.txt\r\n",
            project.display()
        ),
    )
    .unwrap();

    let launcher = Launcher::from_path(shim);
    let error =
        probe_launcher_with_timeout(&launcher, &["--version"], &[], Duration::from_millis(300))
            .await
            .unwrap_err();

    assert_eq!(error, ProbeError::TimedOut);
    std::thread::sleep(Duration::from_secs(6));

    assert!(
        !project.join("parent.txt").exists(),
        "the timed-out shim itself must have been killed before it finished"
    );
    assert!(
        project.join("child.txt").exists(),
        "if this fails, descendants are now being killed too: update the documented boundary"
    );
}
