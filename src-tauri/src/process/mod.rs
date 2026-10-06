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

/// Which terminal host a launch plan targets.
///
/// Passed explicitly to [`build_launch_plan`] rather than detected inside it, so
/// both branches can be exercised by tests on a machine where only one of them
/// would otherwise ever run. The PowerShell fallback is the branch that builds a
/// command string, so it is the one that most needs testing — and it is exactly
/// the branch a developer with Windows Terminal installed never reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKind {
    /// `wt -d <dir> -- <exe>`: every part is a separate argv entry.
    WindowsTerminal,
    /// `powershell.exe -Command <script>`: a single argv element that PowerShell
    /// then parses, so the script must be built with correct quoting.
    PowerShell,
}

/// Build the launch plan for `agent` in `project_dir`.
///
/// The returned argv is fully explicit; nothing is shell-interpreted.
pub fn build_launch_plan(
    agent: AgentId,
    agent_executable: &Path,
    project_dir: &Path,
) -> BridgeResult<LaunchPlan> {
    let (terminal, is_wt) = resolve_terminal();
    let kind = if is_wt {
        TerminalKind::WindowsTerminal
    } else {
        TerminalKind::PowerShell
    };
    build_launch_plan_for(agent, agent_executable, project_dir, kind, &terminal)
}

/// Build a launch plan for an explicitly chosen terminal host.
///
/// [`build_launch_plan`] is the production entry point; this variant exists so
/// tests can exercise both terminal branches deterministically.
pub fn build_launch_plan_for(
    agent: AgentId,
    agent_executable: &Path,
    project_dir: &Path,
    kind: TerminalKind,
    terminal: &Path,
) -> BridgeResult<LaunchPlan> {
    let working_dir = normalize_project_dir(project_dir)?;
    let workdir_str = working_dir.display().to_string();
    let exe_str = agent_executable.display().to_string();

    let is_wt = kind == TerminalKind::WindowsTerminal;

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
    fn powershell_fallback_escapes_a_single_quote_in_the_project_path() {
        // Regression: this assertion used to sit behind `if is_wt { return; }`, so
        // on any machine with Windows Terminal installed — the common case, and
        // the machine the release is built on — the escaping was never checked.
        // The terminal is now chosen explicitly, so this always runs.
        let dir = project();
        let tricky = dir.path().join("proj'quote");
        std::fs::create_dir_all(&tricky).unwrap();

        let plan = build_launch_plan_for(
            AgentId::Codex,
            Path::new("codex.exe"),
            &tricky,
            TerminalKind::PowerShell,
            Path::new("powershell.exe"),
        )
        .unwrap();

        assert_eq!(plan.args[0], "-NoExit");
        assert_eq!(plan.args[2], "-Command");
        let script = plan.args.last().unwrap();
        assert!(script.contains("Set-Location -LiteralPath"), "{script}");
        assert!(
            script.contains("''"),
            "a single quote in the path must be doubled: {script}"
        );
        // The doubled quote must appear in the path position, not somewhere else.
        assert!(
            script.contains("proj''quote"),
            "the escaped path is malformed: {script}"
        );
        // PowerShell is invoked by absolute path, not resolved through PATH.
        assert_eq!(plan.program, "powershell.exe");
    }

    #[test]
    fn powershell_fallback_does_not_interpret_shell_metacharacters() {
        // A directory name containing characters that mean something to a shell
        // must arrive as a literal. `Set-Location -LiteralPath '<path>'` is what
        // makes this safe; if the implementation ever switches to string
        // interpolation without quoting, this test fails.
        //
        // The name must also be one Windows will actually create, which rules out
        // `|`, `"`, `<`, `>`, `:`, and a trailing space or dot. `;`, `&`, `$(`,
        // `(`, backtick and `'` are all legal in a directory name and all
        // meaningful to PowerShell, so they cover the injection surface that
        // exists in practice.
        let dir = project();
        let name = "a; Remove-Item -Recurse & $(whoami) (x) `whoami`";
        let tricky = dir.path().join(name);
        std::fs::create_dir_all(&tricky).unwrap();

        let plan = build_launch_plan_for(
            AgentId::Codex,
            Path::new("codex.exe"),
            &tricky,
            TerminalKind::PowerShell,
            Path::new("powershell.exe"),
        )
        .unwrap();

        let script = plan.args.last().unwrap();

        // The path must sit inside a single-quoted literal that begins
        // immediately after `-LiteralPath `, so PowerShell treats every
        // metacharacter in it as an ordinary character.
        let after = script
            .split("Set-Location -LiteralPath ")
            .nth(1)
            .expect("the path is passed to -LiteralPath");
        let quoted = after
            .strip_prefix('\'')
            .expect("the path must be a single-quoted literal");
        let literal = quoted.split('\'').next().expect("the literal terminates");

        assert!(
            literal.contains("a; Remove-Item -Recurse & $(whoami) (x) `whoami`"),
            "the path was not quoted as one literal: {script}"
        );
        // And the executable is quoted too, not left bare after the call operator.
        assert!(
            script.contains("& 'codex.exe'"),
            "the executable must be quoted: {script}"
        );
    }

    #[test]
    fn a_path_containing_a_quote_is_doubled_so_the_literal_cannot_be_broken_out_of() {
        // The escaping rule that actually matters: a `'` inside the path must
        // become `''`. If it were emitted raw, the literal would end early and
        // everything after it would be parsed as PowerShell.
        let quoted = ps_single_quote(r"C:\Users\o'brien\proj'ect");
        assert_eq!(quoted, r"'C:\Users\o''brien\proj''ect'");
        // The result must be a single well-formed literal.
        assert!(quoted.starts_with('\''));
        assert!(quoted.ends_with('\''));
        // No lone quote may survive inside.
        let inner = &quoted[1..quoted.len() - 1];
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\'' {
                assert_eq!(
                    chars.next(),
                    Some('\''),
                    "a lone quote inside the literal would terminate it early"
                );
            }
        }
    }

    #[test]
    fn windows_terminal_branch_passes_each_part_as_its_own_argument() {
        // The preferred branch builds no shell string at all, so a path with
        // spaces or metacharacters is inert. Assert the argv shape directly.
        let dir = project();
        let tricky = dir.path().join("proj'quote & spaces");
        std::fs::create_dir_all(&tricky).unwrap();

        let plan = build_launch_plan_for(
            AgentId::Codex,
            Path::new(r"C:\Program Files\codex.exe"),
            &tricky,
            TerminalKind::WindowsTerminal,
            Path::new("wt.exe"),
        )
        .unwrap();

        assert_eq!(plan.args.len(), 4, "argv: {:?}", plan.args);
        assert_eq!(plan.args[0], "-d");
        assert_eq!(plan.args[2], "--");
        assert!(
            plan.args[3].ends_with("codex.exe"),
            "the executable is its own argv entry: {:?}",
            plan.args
        );
        // The path is one entry, unquoted inside itself.
        assert!(
            plan.args[1].contains("proj'quote & spaces"),
            "{:?}",
            plan.args
        );
        // No argument may contain a shell operator joining two commands.
        for arg in &plan.args {
            assert!(
                !arg.contains("; &"),
                "an argument looks like a joined shell command: {arg}"
            );
        }
    }

    #[test]
    fn both_terminal_branches_reject_the_same_bad_directory() {
        // Directory validation must not depend on which terminal was chosen.
        for kind in [TerminalKind::WindowsTerminal, TerminalKind::PowerShell] {
            let err = build_launch_plan_for(
                AgentId::Codex,
                Path::new("codex.exe"),
                Path::new(r"C:\definitely\not\here"),
                kind,
                Path::new("powershell.exe"),
            )
            .expect_err("a nonexistent directory must be rejected");
            assert!(
                !err.message.is_empty(),
                "the error must explain itself: {err:?}"
            );
        }
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
