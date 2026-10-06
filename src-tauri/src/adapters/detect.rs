//! Detection of installed coding agents (PRD §14).
//!
//! Detection is deliberately conservative and fully asynchronous-friendly: it
//! shells out only to `where` and `<agent> --version`, both under a timeout, so
//! the UI never blocks on a hung process (PRD §10, §68).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::error::BridgeResult;

/// Maximum time a detection subprocess may take.
pub const DETECTION_TIMEOUT: Duration = Duration::from_secs(5);

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

/// Run `<executable> --version` and return the trimmed first line.
///
/// The executable path comes from detection, never from user input, and is
/// passed as argv rather than being interpolated into a shell string
/// (PRD §27, §63).
pub async fn version_of(executable: &Path) -> Option<String> {
    let output = tokio::time::timeout(
        DETECTION_TIMEOUT,
        Command::new(executable)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output(),
    )
    .await;

    let Ok(Ok(output)) = output else {
        return None;
    };
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().find(|l| !l.trim().is_empty())?;
    Some(clean_version(line))
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
        if candidate
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
        {
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
pub fn resolve_config_dir(
    env_var: &str,
    default_dir: PathBuf,
) -> (PathBuf, bool) {
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

/// Run an arbitrary detected-agent command with a timeout, returning stdout.
///
/// Used for `--version` style probes. The command is always a resolved
/// executable plus a fixed argv, never a shell string.
pub async fn run_capture(program: &Path, args: &[&str]) -> BridgeResult<String> {
    let output = tokio::time::timeout(
        DETECTION_TIMEOUT,
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| {
        crate::error::BridgeError::new(
            crate::error::ErrorKind::AgentUnavailable,
            "The agent did not respond in time.",
        )
    })?
    .map_err(|err| {
        crate::error::BridgeError::new(
            crate::error::ErrorKind::AgentUnavailable,
            "Could not run the agent executable.",
        )
        .with_details(err.to_string())
    })?;

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
            assert!(!text.contains("hisya"), "no developer machine paths: {text}");
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
}
