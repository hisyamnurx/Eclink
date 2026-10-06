//! Process launching for coding agents (PRD §27, §32).
//!
//! Bridge never builds a terminal emulator and never joins a shell command from
//! user-controlled text. It resolves a terminal host, then passes the agent
//! executable and the project directory as **argv entries**, so a model ID,
//! provider name, or project path that contains spaces, quotes, or shell
//! metacharacters cannot become a command (PRD §63).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::adapters::{AgentId, LaunchPlan};
use crate::error::{BridgeError, BridgeResult};
/// Windows Terminal, preferred when present (PRD §32.2).
const WINDOWS_TERMINAL: &str = "wt.exe";

/// Where Windows Terminal is installed for the current user.
fn windows_terminal_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(&local)
                .join("Microsoft")
                .join("WindowsApps")
                .join(WINDOWS_TERMINAL),
        );
    }
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        candidates.push(
            PathBuf::from(&program_files)
                .join("WindowsApps")
                .join(WINDOWS_TERMINAL),
        );
    }
    candidates
}

/// Resolve the terminal host: Windows Terminal when available, else PowerShell.
///
/// Returns `(program, is_windows_terminal)`.
pub fn resolve_terminal() -> (PathBuf, bool) {
    // Respect an explicit `WT_SESSION`-less PATH lookup first.
    if let Some(path) = crate::adapters::detect::first_existing(&windows_terminal_candidates()) {
        return (path, true);
    }
    if let Ok(system_root) = std::env::var("SystemRoot") {
        let ps = PathBuf::from(&system_root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        if ps.is_file() {
            return (ps, false);
        }
    }
    // Last resort: rely on PATH resolution for `powershell.exe`.
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        let pwsh = PathBuf::from(program_files)
            .join("PowerShell")
            .join("7")
            .join("pwsh.exe");
        if pwsh.is_file() {
            return (pwsh, false);
        }
    }
    (PathBuf::from("powershell.exe"), false)
}

/// Build the launch plan for `agent` in `project_dir`.
///
/// The returned argv is fully explicit; nothing is shell-interpreted.
pub fn build_launch_plan(
    agent: AgentId,
    agent_executable: &Path,
    project_dir: &Path,
) -> BridgeResult<LaunchPlan> {
    let working_dir = normalize_project_dir(project_dir)?;
    let workdir_str = working_dir.display().to_string();
    let exe_str = agent_executable.display().to_string();

    let (terminal, is_wt) = resolve_terminal();

    let args: Vec<String> = if is_wt {
        // `wt -d <dir> -- <exe>` opens a new tab in the project directory and
        // runs the agent there. Each element is a separate argv entry.
        vec![
            "-d".to_string(),
            workdir_str.clone(),
            "--".to_string(),
            exe_str.clone(),
        ]
    } else {
        // PowerShell fallback. `-NoExit` keeps the window open after the agent
        // exits so the user can read output. The command is a single argv
        // element (PowerShell's own -Command contract), built only from the
        // resolved executable path and the *validated* project directory.
        //
        // The path is passed via the `-LiteralPath`/`Set-Location` cmdlet rather
        // than string concatenation into a command pipeline, and the executable
        // is invoked with the call operator on a single-quoted literal.
        let script = format!(
            "Set-Location -LiteralPath {}; & {}",
            ps_single_quote(&workdir_str),
            ps_single_quote(&exe_str)
        );
        vec![
            "-NoExit".to_string(),
            "-NoLogo".to_string(),
            "-Command".to_string(),
            script,
        ]
    };

    let display = if is_wt {
        format!("wt -d \"{workdir_str}\" -- \"{exe_str}\"")
    } else {
        format!("Set-Location -LiteralPath \"{workdir_str}\"; & \"{exe_str}\"")
    };

    Ok(LaunchPlan {
        agent,
        program: terminal.display().to_string(),
        args,
        working_dir: workdir_str,
        display,
    })
}

/// Validate and canonicalise a project directory.
///
/// Rejects anything that is not an existing directory so a typo cannot launch
/// the agent in an unexpected location.
pub fn normalize_project_dir(project_dir: &Path) -> BridgeResult<PathBuf> {
    let trimmed = project_dir.as_os_str().to_string_lossy().trim().to_string();
    if trimmed.is_empty() {
        return Err(BridgeError::validation("Choose a project folder first."));
    }
    let path = PathBuf::from(&trimmed);
    if !path.exists() {
        return Err(
            BridgeError::validation("That project folder does not exist.")
                .with_hint("Pick an existing folder."),
        );
    }
    if !path.is_dir() {
        return Err(
            BridgeError::validation("The project path is a file, not a folder.")
                .with_hint("Pick a folder."),
        );
    }
    // Canonicalising resolves `..` and short (8.3) names so the launched
    // process' working directory is unambiguous.
    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    Ok(strip_verbatim_prefix(canonical))
}

/// Remove the `\\?\` verbatim prefix Windows adds during canonicalisation.
///
/// The prefix is correct for the Win32 API but ugly in a terminal title, in the
/// path handed to `wt -d`, and in the UI. Removing it keeps the displayed and
/// launched path the one the user actually chose. UNC paths (`\\?\UNC\...`)
/// are converted back to their `\\server\share` form.
pub fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path
}

/// Quote a value for a PowerShell single-quoted literal.
///
/// Inside single quotes PowerShell treats everything literally; the only escape
/// needed is doubling a single quote. This is defense in depth: even though the
/// plan passes argv directly, the fallback path embeds these in a `-Command`
/// string.
fn ps_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Everything needed to spawn a launched agent.
#[derive(Debug, Clone)]
pub struct SpawnPlan {
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: String,
    /// Environment overlay applied to this process tree only.
    pub env: Vec<(String, String)>,
}

/// Spawn the process described by `plan`.
///
/// The child is detached from Bridge: closing Eclyzer Bridge must not kill the
/// coding agent the user just opened (PRD §84).
pub fn spawn(plan: &SpawnPlan) -> BridgeResult<u32> {
    let mut command = std::process::Command::new(&plan.program);
    command
        .args(&plan.args)
        .current_dir(&plan.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    for (key, value) in &plan.env {
        command.env(key, value);
    }

    // A new process group keeps the agent alive if Bridge is closed.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }

    let child = command.spawn().map_err(|err| {
        BridgeError::new(
            crate::error::ErrorKind::AgentUnavailable,
            "Could not open a terminal for the agent.",
        )
        .with_hint("Check that Windows Terminal or PowerShell is available.")
        .with_details(err.to_string())
    })?;

    Ok(child.id())
}

/// Spawn an agent from a [`LaunchPlan`] plus its environment overlay.
///
/// Keeping this as the single entry point means the launch command cannot
/// accidentally build a `SpawnPlan` without the overlay, which would silently
/// drop the credential an agent needs.
pub fn spawn_launch(
    plan: &crate::adapters::LaunchPlan,
    env: &[(String, String)],
) -> BridgeResult<u32> {
    let spawn_plan = SpawnPlan {
        program: plan.program.clone(),
        args: plan.args.clone(),
        working_dir: plan.working_dir.clone(),
        env: env.to_vec(),
    };
    spawn(&spawn_plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn rejects_missing_project_directory() {
        let err = normalize_project_dir(Path::new(r"C:\definitely\not\here\xyz")).unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Validation);
        assert!(err.hint.is_some());
    }

    #[test]
    fn rejects_a_file_as_project_directory() {
        let dir = project();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let err = normalize_project_dir(&file).unwrap_err();
        assert!(err.message.contains("not a folder"), "{}", err.message);
    }

    #[test]
    fn rejects_empty_project_directory() {
        assert!(normalize_project_dir(Path::new("   ")).is_err());
    }

    #[test]
    fn accepts_paths_with_spaces_and_unicode() {
        let dir = project();
        let nested = dir.path().join("My Project Ünïcødé");
        std::fs::create_dir_all(&nested).unwrap();
        let normalized = normalize_project_dir(&nested).unwrap();
        assert!(normalized.is_dir());
        assert!(normalized.to_string_lossy().contains("My Project Ünïcødé"));
    }

    #[test]
    fn launch_plan_passes_paths_as_separate_argv_entries() {
        // The core command-injection guard: a project path containing shell
        // metacharacters must remain a *single* argv entry, so no shell can
        // reinterpret the `&` or `;`.
        let dir = project();
        let evil = dir.path().join("proj & calc.exe ; rm -rf");
        std::fs::create_dir_all(&evil).unwrap();

        let plan =
            build_launch_plan(AgentId::Codex, Path::new(r"C:\tools\codex.exe"), &evil).unwrap();

        // Exactly one argv entry contains the full path, verbatim.
        let matching: Vec<&String> = plan
            .args
            .iter()
            .filter(|arg| arg.contains("proj & calc.exe ; rm -rf"))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "the path must appear in exactly one argv entry: {:?}",
            plan.args
        );
        assert!(
            matching[0].ends_with("proj & calc.exe ; rm -rf"),
            "the path must not be split or truncated: {}",
            matching[0]
        );

        // The injection payload must never appear as its own executable argv.
        assert!(!plan.args.iter().any(|arg| arg == "calc.exe"));
        assert!(!plan.args.iter().any(|arg| arg == "rm"));
        assert!(!plan.args.iter().any(|arg| arg == "-rf"));
    }

    #[test]
    fn launch_plan_uses_resolved_executable_not_user_text() {
        let dir = project();
        let plan = build_launch_plan(
            AgentId::Claude,
            Path::new(r"C:\Users\Test User\AppData\Local\Programs\claude.exe"),
            dir.path(),
        )
        .unwrap();
        let joined = plan.args.join(" ");
        assert!(joined.contains("claude.exe"), "{joined}");
        assert_eq!(plan.agent, AgentId::Claude);
    }

    #[test]
    fn powershell_fallback_quotes_single_quotes() {
        assert_eq!(ps_single_quote("plain"), "'plain'");
        assert_eq!(ps_single_quote("it's"), "'it''s'");
    }

    #[test]
    fn powershell_fallback_script_uses_literal_path() {
        let dir = project();
        let tricky = dir.path().join("proj'quote");
        std::fs::create_dir_all(&tricky).unwrap();

        let (_, is_wt) = resolve_terminal();
        if is_wt {
            // Windows Terminal path: no shell string at all.
            let plan = build_launch_plan(AgentId::Codex, Path::new("codex.exe"), &tricky).unwrap();
            assert!(plan.args.contains(&"--".to_string()));
            return;
        }

        let plan = build_launch_plan(AgentId::Codex, Path::new("codex.exe"), &tricky).unwrap();
        let script = plan.args.last().unwrap();
        assert!(script.contains("Set-Location -LiteralPath"), "{script}");
        assert!(
            script.contains("''"),
            "single quote must be escaped: {script}"
        );
    }

    #[test]
    fn launch_plan_rejects_bad_directory() {
        let err = build_launch_plan(
            AgentId::Codex,
            Path::new("codex.exe"),
            Path::new(r"C:\nope\nope"),
        )
        .unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Validation);
    }

    #[test]
    fn terminal_resolution_always_yields_something() {
        let (program, _) = resolve_terminal();
        assert!(!program.as_os_str().is_empty());
    }

    #[test]
    fn windows_terminal_candidates_avoid_hardcoded_usernames() {
        // Candidates must be derived from environment variables, never from a
        // literal developer-maching path. Check the constructed strings contain
        // no `\Users\<name>` segment that did not come from the environment.
        let env_users = std::env::var("LOCALAPPDATA").unwrap_or_default();
        for candidate in windows_terminal_candidates() {
            let text = candidate.to_string_lossy();
            assert!(text.ends_with("wt.exe"), "unexpected candidate: {text}");
            if text.contains(r"\Users\") {
                // If a user segment appears, it must have come from LOCALAPPDATA.
                assert!(
                    !env_users.is_empty() && text.starts_with(&env_users),
                    "candidate does not derive from the environment: {text}"
                );
            }
        }
    }

    #[test]
    fn display_string_is_readable() {
        let dir = project();
        let plan = build_launch_plan(AgentId::Codex, Path::new(r"C:\tools\codex.exe"), dir.path())
            .unwrap();
        assert!(!plan.display.is_empty());
        assert!(plan.display.contains("codex.exe"));
    }
}
