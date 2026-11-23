//! Detection of installed coding agents (PRD §14).
//!
//! Detection is deliberately conservative and fully asynchronous-friendly: it
//! shells out only to `where` and `<agent> --version`, both under a timeout, so
//! the UI never blocks on a hung process (PRD §10, §68).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::adapters::locate::{Launcher, LocationProblem};

/// Maximum time a detection subprocess may take.
pub const DETECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Creation flag that keeps a console window from flashing when a probe runs a
/// console executable such as `cmd.exe` from the GUI process.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Why running an agent executable did not produce output Eclink can use.
///
/// This is deliberately finer than a `BridgeError`: the interface distinguishes
/// "it never answered" from "it answered with an error" when a saved install
/// location turns out to be unusable, and a timeout has to be recognisable after
/// the fact without matching on a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// The process was still running at the timeout and has been killed.
    TimedOut,
    /// The process could not be started at all.
    Unavailable(String),
    /// The process started and exited unsuccessfully.
    Failed(String),
    /// The process succeeded but printed nothing usable.
    Unreadable(String),
}

impl ProbeError {
    /// The machine code the interface reports for a saved location that failed.
    pub fn location_problem(&self) -> LocationProblem {
        match self {
            ProbeError::TimedOut => LocationProblem::Timeout,
            _ => LocationProblem::Unusable,
        }
    }

    /// A sentence naming the agent, for an error the user sees.
    pub fn message(&self, agent_name: &str) -> String {
        match self {
            ProbeError::TimedOut => {
                format!("{agent_name} did not respond in time.")
            }
            ProbeError::Unavailable(_) => {
                format!("Eclink could not run {agent_name}.")
            }
            ProbeError::Failed(_) => {
                format!("{agent_name} exited with an error.")
            }
            ProbeError::Unreadable(_) => {
                format!("{agent_name} printed something Eclink could not read.")
            }
        }
    }

    /// Technical detail, already truncated, for the error's `details` field.
    pub fn detail(&self) -> Option<String> {
        match self {
            ProbeError::TimedOut => None,
            ProbeError::Unavailable(detail)
            | ProbeError::Failed(detail)
            | ProbeError::Unreadable(detail) => Some(detail.clone()),
        }
    }
}

/// Result of running `where <name>`.
#[derive(Debug, Clone, Default)]
pub struct WhichResult {
    /// Every path reported, in the order Windows returned them.
    pub paths: Vec<PathBuf>,
}

impl WhichResult {
    pub fn first(&self) -> Option<&PathBuf> {
        self.paths.first()
    }
}

/// Resolve an executable through `PATH` without invoking a shell.
pub async fn which(name: &str) -> WhichResult {
    let mut result = WhichResult::default();

    // `where.exe` is the Windows equivalent of `which`. It is a separate
    // process but takes the executable name as a single argv entry, so a name
    // can never be interpreted as a command line.
    let output = tokio::time::timeout(
        DETECTION_TIMEOUT,
        Command::new("where.exe")
            .arg(name)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output(),
    )
    .await;

    if let Ok(Ok(output)) = output {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    result.paths.push(PathBuf::from(trimmed));
                }
            }
        }
    }

    result
}

/// Run `<executable> --version` and return the trimmed, normalised version.
///
/// The launcher path comes from detection or from the user's saved location, and
/// is passed as argv rather than being interpolated into a shell string (PRD §27,
/// §63). A Windows shim is run through `cmd.exe` by [`Launcher::invocation`], so
/// npm- and Bun-installed agents are probed the same way they are launched.
pub async fn version_of(executable: &Path) -> Result<String, ProbeError> {
    version_of_launcher(&Launcher::from_path(executable.to_path_buf())).await
}

/// Run a launcher's `--version` and return the cleaned version line.
pub async fn version_of_launcher(launcher: &Launcher) -> Result<String, ProbeError> {
    let stdout = probe_launcher(launcher, &["--version"]).await?;
    let line = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| ProbeError::Unreadable("It printed no version.".to_string()))?;
    Ok(clean_version(line))
}

/// Strip a leading product name from a version line.
///
/// `codex-cli 0.42.0` and `1.2.3 (Claude Code)` both normalise to a version-ish
/// string, which is what the UI shows.
pub fn clean_version(line: &str) -> String {
    let trimmed = line.trim();
    // Prefer the first token that starts with a digit.
    for token in trimmed.split_whitespace() {
        let candidate = token.trim_start_matches('v');
        if candidate.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return token.trim_start_matches('v').to_string();
        }
    }
    trimmed.to_string()
}

/// Parse a `MAJOR.MINOR.PATCH` prefix into a comparable tuple.
///
/// Returns `None` when the string does not start with a version, so callers
/// treat an unparsable version as "unknown" rather than "too old".
pub fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim().trim_start_matches('v');
    let mut parts = core.split(['.', '-', '+']);
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
    let patch = parts.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
    Some((major, minor, patch))
}

/// Whether `version` is at least `minimum`. Unparsable versions pass.
pub fn version_at_least(version: &str, minimum: (u64, u64, u64)) -> bool {
    match parse_version(version) {
        Some(parsed) => parsed >= minimum,
        None => true,
    }
}

/// Candidate locations checked when `where` finds nothing (PRD §14.1).
///
/// These are derived from the user profile and standard package manager
/// prefixes — never a hardcoded `C:\Users\<name>` (PRD §47).
pub fn known_locations(home: &Path, names: &[&str], subdirs: &[&str]) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    let bases: Vec<PathBuf> = vec![
        home.join("AppData").join("Local").join("Programs"),
        home.join(".local").join("bin"),
        home.join("AppData").join("Roaming").join("npm"),
        home.join(".npm-global").join("bin"),
        home.join(".bun").join("bin"),
        home.join("scoop").join("shims"),
        home.join("AppData").join("Local").join("pnpm"),
    ];

    for base in &bases {
        for name in names {
            candidates.push(base.join(name));
        }
    }

    for subdir in subdirs {
        let base = home.join(subdir);
        for name in names {
            candidates.push(base.join(name));
        }
    }

    candidates
}

/// The first candidate that exists and is a file.
pub fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|path| path.is_file()).cloned()
}

/// Resolve the effective config directory for an agent.
///
/// Honours the agent's documented environment override before falling back to
/// the default under the user profile (PRD §14.1, §14.2, §16.1, §17.1).
pub fn resolve_config_dir(env_var: &str, default_dir: PathBuf) -> (PathBuf, bool) {
    match std::env::var(env_var) {
        Ok(value) if !value.trim().is_empty() => {
            // The value may embed `~`, which the agent itself expands.
            let expanded = crate::paths::expand_home(value.trim())
                .unwrap_or_else(|_| PathBuf::from(value.trim()));
            (expanded, true)
        }
        _ => (default_dir, false),
    }
}

/// A `std::process::Command`-free helper: check whether a path is an existing
/// directory without following into it.
pub fn dir_exists(path: &Path) -> bool {
    path.is_dir()
}

/// Run a launcher command under a timeout, returning stdout.
///
/// Used for `--version` style probes and for OMP's registry check. The command is
/// always a resolved launcher plus a fixed argv, never a shell string. On timeout
/// the child is killed (`kill_on_drop`), so a hung agent cannot keep running after
/// Eclink has already reported a failure.
pub async fn probe_launcher(launcher: &Launcher, args: &[&str]) -> Result<String, ProbeError> {
    probe_launcher_with_env(launcher, args, &[]).await
}

/// [`probe_launcher`] with environment variables applied to the child only.
///
/// OMP's registry check needs `PI_CODING_AGENT_DIR`; the overlay is scoped to
/// this process and never written to the user's global environment.
pub async fn probe_launcher_with_env(
    launcher: &Launcher,
    args: &[&str],
    env: &[(String, String)],
) -> Result<String, ProbeError> {
    probe_launcher_with_timeout(launcher, args, env, DETECTION_TIMEOUT).await
}

/// [`probe_launcher_with_env`] with an explicit timeout, for tests.
pub async fn probe_launcher_with_timeout(
    launcher: &Launcher,
    args: &[&str],
    env: &[(String, String)],
    timeout: Duration,
) -> Result<String, ProbeError> {
    let (program, argv) = launcher.invocation(args);
    let mut command = Command::new(&program);
    command
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in env {
        command.env(key, value);
    }
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| ProbeError::TimedOut)?
        .map_err(|err| ProbeError::Unavailable(truncate(&err.to_string(), 400)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let fallback = format!("exit status {}", output.status);
        return Err(ProbeError::Failed(truncate(
            if detail.is_empty() {
                fallback.as_str()
            } else {
                detail
            },
            400,
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Keep an error detail short enough to read and to store in a log line.
fn truncate(value: &str, limit: usize) -> String {
    let mut out: String = value.chars().take(limit).collect();
    if value.chars().count() > limit {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_version_extracts_numeric_token() {
        assert_eq!(clean_version("codex-cli 0.42.0"), "0.42.0");
        assert_eq!(clean_version("1.2.3 (Claude Code)"), "1.2.3");
        assert_eq!(clean_version("v2.0.0"), "2.0.0");
        assert_eq!(clean_version("no version here"), "no version here");
    }

    #[test]
    fn parse_version_handles_prefixes_and_suffixes() {
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3-beta.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("2.0"), Some((2, 0, 0)));
        assert_eq!(parse_version("not-a-version"), None);
    }

    #[test]
    fn version_gate_treats_unknown_as_passing() {
        // A version we cannot parse must never produce a false "too old".
        assert!(version_at_least("weird", (9, 9, 9)));
        assert!(version_at_least("1.0.0", (1, 0, 0)));
        assert!(!version_at_least("0.9.9", (1, 0, 0)));
    }

    #[test]
    fn known_locations_never_hardcode_a_username() {
        let home = Path::new(r"C:\Users\Test User");
        let candidates = known_locations(home, &["codex.exe"], &[".codex"]);
        assert!(!candidates.is_empty());
        for candidate in &candidates {
            let text = candidate.to_string_lossy();
            assert!(
                text.starts_with(r"C:\Users\Test User"),
                "location must be derived from home: {text}"
            );
            assert!(
                !text.contains("hisya"),
                "no developer machine paths: {text}"
            );
        }
    }

    #[test]
    fn known_locations_handles_unicode_home() {
        let home = Path::new(r"C:\Users\Ünïcødé User");
        let candidates = known_locations(home, &["claude.exe"], &[".claude"]);
        assert!(candidates
            .iter()
            .any(|c| c.to_string_lossy().contains("Ünïcødé")));
    }

    #[test]
    fn first_existing_picks_the_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.exe");
        let present = dir.path().join("yes.exe");
        std::fs::write(&present, b"stub").unwrap();

        let found = first_existing(&[missing.clone(), present.clone()]);
        assert_eq!(found, Some(present));
        assert_eq!(first_existing(&[missing]), None);
    }

    #[test]
    fn resolve_config_dir_prefers_env_override() {
        // Use a variable unlikely to collide with a real one.
        let var = "ECLYZER_TEST_CODEX_HOME_XYZ";
        std::env::set_var(var, r"D:\custom\codex-home");
        let (dir, from_env) = resolve_config_dir(var, PathBuf::from(r"C:\default\.codex"));
        assert!(from_env);
        assert_eq!(dir, PathBuf::from(r"D:\custom\codex-home"));

        std::env::remove_var(var);
        let (dir, from_env) = resolve_config_dir(var, PathBuf::from(r"C:\default\.codex"));
        assert!(!from_env);
        assert_eq!(dir, PathBuf::from(r"C:\default\.codex"));
    }

    #[test]
    fn resolve_config_dir_ignores_blank_env() {
        let var = "ECLYZER_TEST_BLANK_HOME_XYZ";
        std::env::set_var(var, "   ");
        let (dir, from_env) = resolve_config_dir(var, PathBuf::from(r"C:\default"));
        assert!(!from_env, "a blank override must be ignored");
        assert_eq!(dir, PathBuf::from(r"C:\default"));
        std::env::remove_var(var);
    }

    #[tokio::test]
    async fn which_finds_a_real_system_executable() {
        // `cmd.exe` exists on every supported Windows build.
        let found = which("cmd.exe").await;
        assert!(found.first().is_some(), "where.exe should locate cmd.exe");
    }

    #[tokio::test]
    async fn which_returns_empty_for_missing_binary() {
        let found = which("eclyzer-definitely-not-a-real-binary-xyz").await;
        assert!(found.paths.is_empty());
    }

    /// Write a Windows shim that prints `version`.
    fn write_shim(dir: &Path, name: &str, body: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body.as_bytes()).unwrap();
        path
    }

    #[tokio::test]
    async fn a_windows_shim_is_probed_through_cmd_with_spaces_and_apostrophes() {
        // npm and Bun install `codex.cmd`, not `codex.exe`. A path containing a
        // space, an apostrophe, and non-ASCII characters is the case where naive
        // quoting breaks: `cmd.exe` must receive the path as one literal entry.
        let dir = tempfile::tempdir().unwrap();
        let shim = write_shim(
            &dir.path().join("Program Files O'Brien Ünïcødé"),
            "codex.cmd",
            "@echo off\r\necho codex-cli 9.9.9\r\n",
        );

        let launcher = Launcher::from_path(shim.clone());
        let version = version_of_launcher(&launcher).await.unwrap();

        assert_eq!(version, "9.9.9");
        // The invocation must name the shim once and keep it as one argv entry.
        let (program, argv) = launcher.invocation(&["--version"]);
        assert!(
            program
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with("cmd.exe"),
            "a shim must run through cmd.exe, got {}",
            program.display()
        );
        assert_eq!(argv[..3], ["/d", "/c", "call"], "argv: {argv:?}");
        assert_eq!(
            argv[3],
            shim.display().to_string(),
            "the shim path is one entry and is not split: {argv:?}"
        );
    }

    #[tokio::test]
    async fn a_hung_probe_is_killed_and_reported_as_a_timeout() {
        // `ping` takes about nine seconds here; the probe must give up long before
        // that and say so, rather than blocking detection.
        let dir = tempfile::tempdir().unwrap();
        let shim = write_shim(
            dir.path(),
            "slow.cmd",
            "@echo off\r\nping -n 10 127.0.0.1 >nul\r\necho codex-cli 9.9.9\r\n",
        );

        let launcher = Launcher::from_path(shim);
        let error =
            probe_launcher_with_timeout(&launcher, &["--version"], &[], Duration::from_millis(250))
                .await
                .unwrap_err();

        assert_eq!(error, ProbeError::TimedOut);
        assert_eq!(
            error.location_problem(),
            LocationProblem::Timeout,
            "a timeout is reported distinctly so the copy can say so"
        );
    }

    #[tokio::test]
    async fn a_shim_that_exits_with_an_error_is_not_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let shim = write_shim(
            dir.path(),
            "broken.cmd",
            "@echo off\r\necho nope 1>&2\r\nexit /b 3\r\n",
        );

        let error = version_of_launcher(&Launcher::from_path(shim))
            .await
            .unwrap_err();

        match error {
            ProbeError::Failed(detail) => assert!(detail.contains("nope"), "{detail}"),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_native_executable_is_still_run_directly() {
        // `cmd.exe /c exit 0` as a native launcher: no shim wrapper is involved.
        let comspec = std::env::var_os("ComSpec")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cmd.exe"));
        let launcher = Launcher::from_path(comspec);
        let (program, argv) = launcher.invocation(&["/c", "exit", "0"]);

        assert_eq!(program, launcher.path());
        assert_eq!(argv, ["/c", "exit", "0"]);
    }
}
