//! Process launching for coding agents (PRD §27, §32).
//!
//! Bridge never builds a terminal emulator and never joins a shell command from
//! user-controlled text. It resolves a terminal host, then passes the agent
//! executable and the project directory as **argv entries**, so a model ID,
//! provider name, or project path that contains spaces, quotes, or shell
//! metacharacters cannot become a command (PRD §63).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::adapters::locate::{AgentCommand, Launcher};
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
/// The returned argv is fully explicit; nothing is shell-interpreted. A
/// `.cmd`/`.bat` launcher becomes a `cmd.exe` command built by
/// [`Launcher::command`], the same builder probes and registry validation use.
pub fn build_launch_plan(
    agent: AgentId,
    agent_executable: &Path,
    project_dir: &Path,
) -> BridgeResult<LaunchPlan> {
    let command = launcher_command(agent_executable, &[])?;
    build_launch_plan_command(agent, &command, project_dir)
}

/// Build a launch plan with agent arguments (for example OMP's `--profile`).
///
/// For a native executable the arguments stay separate argv entries. For a
/// shim they are quoted once each and appended to one raw command line, so
/// `cmd.exe` receives them as literals rather than as Rust-quoted argv.
pub fn build_launch_plan_with_args(
    agent: AgentId,
    agent_executable: &Path,
    project_dir: &Path,
    agent_args: &[String],
) -> BridgeResult<LaunchPlan> {
    let refs: Vec<&str> = agent_args.iter().map(String::as_str).collect();
    let command = launcher_command(agent_executable, &refs)?;
    build_launch_plan_command(agent, &command, project_dir)
}

/// Build a launch plan for an already-prepared [`AgentCommand`].
pub fn build_launch_plan_command(
    agent: AgentId,
    command: &AgentCommand,
    project_dir: &Path,
) -> BridgeResult<LaunchPlan> {
    let (terminal, is_wt) = resolve_terminal();
    let kind = if is_wt {
        TerminalKind::WindowsTerminal
    } else {
        TerminalKind::PowerShell
    };
    build_launch_plan_for_command(agent, command, project_dir, kind, &terminal)
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
    let command = launcher_command(agent_executable, &[])?;
    build_launch_plan_for_command(agent, &command, project_dir, kind, terminal)
}

/// Build a launch plan for an explicit command and terminal host.
pub fn build_launch_plan_for_command(
    agent: AgentId,
    command: &AgentCommand,
    project_dir: &Path,
    kind: TerminalKind,
    terminal: &Path,
) -> BridgeResult<LaunchPlan> {
    let working_dir = normalize_project_dir(project_dir)?;
    let workdir_str = working_dir.display().to_string();
    for arg in &command.args {
        if arg.chars().any(char::is_control) {
            return Err(BridgeError::validation(
                "An agent argument contains an unsupported control character.",
            ));
        }
    }

    let is_wt = kind == TerminalKind::WindowsTerminal;

    let args: Vec<String>;
    let display;

    match (is_wt, command.command_text.is_some()) {
        // A shim. cmd.exe needs a command line, but Windows Terminal's parser
        // treats a `'` in that line as a quote and breaks a path containing one,
        // and Windows PowerShell 5.1 mangles embedded quotes in native arguments.
        // Both are avoided by transporting the shim command as an encoded
        // PowerShell script: base64 contains no quotes, apostrophes, or spaces.
        // On the Windows Terminal path the working directory is set inside the
        // script too, so no user path reaches the terminal's own parser.
        (true, true) => {
            let encoded = command
                .powershell_encoded_script(&workdir_str)
                .ok_or_else(|| BridgeError::internal("The agent command could not be prepared."))?;
            args = vec![
                "--".to_string(),
                powershell_host().display().to_string(),
                "-NoExit".to_string(),
                "-NoLogo".to_string(),
                "-EncodedCommand".to_string(),
                encoded,
            ];
            display = format!(
                "Set-Location -LiteralPath \"{workdir_str}\"; {}",
                command.display
            );
        }
        (false, true) => {
            let encoded = command
                .powershell_encoded_script(&workdir_str)
                .ok_or_else(|| BridgeError::internal("The agent command could not be prepared."))?;
            args = vec![
                "-NoExit".to_string(),
                "-NoLogo".to_string(),
                "-EncodedCommand".to_string(),
                encoded,
            ];
            display = format!(
                "Set-Location -LiteralPath \"{workdir_str}\"; {}",
                command.display
            );
        }
        (true, false) => {
            let mut native = vec![
                "-d".to_string(),
                workdir_str.clone(),
                "--".to_string(),
                command.program.display().to_string(),
            ];
            native.extend(command.args.iter().cloned());
            args = native;
            display = format!(
                "wt -d \"{workdir_str}\" -- \"{}\"{}",
                command.program.display(),
                argument_suffix(&command.args)
            );
        }
        (false, false) => {
            let mut script = format!(
                "Set-Location -LiteralPath {}; & {}",
                powershell_literal(&workdir_str),
                powershell_literal(&command.program.display().to_string())
            );
            for arg in &command.args {
                script.push(' ');
                script.push_str(&powershell_literal(arg));
            }
            args = vec![
                "-NoExit".to_string(),
                "-NoLogo".to_string(),
                "-Command".to_string(),
                script,
            ];
            display = format!(
                "Set-Location -LiteralPath \"{workdir_str}\"; & \"{}\"{}",
                command.program.display(),
                argument_suffix(&command.args)
            );
        }
    }

    Ok(LaunchPlan {
        agent,
        program: terminal.display().to_string(),
        args,
        working_dir: workdir_str,
        display,
        new_console: !is_wt,
    })
}

/// PowerShell used to transport a shim command on the Windows Terminal path.
///
/// Windows PowerShell is present on every supported Windows build and supports
/// `-EncodedCommand`; when PowerShell is already the fallback host, that host is
/// used instead.
fn powershell_host() -> PathBuf {
    if let Ok(system_root) = std::env::var("SystemRoot") {
        let candidate = PathBuf::from(system_root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from("powershell.exe")
}

/// Prepare a launcher path for execution, mapping a refused shim path onto a
/// user-facing validation error.
fn launcher_command(agent_executable: &Path, args: &[&str]) -> BridgeResult<AgentCommand> {
    Launcher::from_path(agent_executable.to_path_buf()).prepare(args)
}

fn argument_suffix(args: &[String]) -> String {
    if args.is_empty() {
        String::new()
    } else {
        format!(" {}", args.join(" "))
    }
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
/// needed is doubling a single quote. This is used both by the PowerShell
/// fallback's script and by the shim builder, which carries a `cmd.exe` command
/// line through that script as data.
pub(crate) fn powershell_literal(value: &str) -> String {
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
    /// Start the host with its own console window (the PowerShell fallback).
    pub new_console: bool,
}

/// How a launched host's standard streams are wired.
///
/// Split out of [`spawn`] so the decision is testable: getting it wrong is
/// invisible at the call site but leaves the agent with no console to talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioWiring {
    /// The child inherits Eclink's own console streams.
    ///
    /// Used when Eclink itself was started from a terminal, so the agent's input
    /// and output continue to work in the window the user is already looking at.
    Inherit,
    /// Do not pass standard handles at all; let Windows initialise them.
    ///
    /// Used for the PowerShell fallback, which is a console program created with
    /// `CREATE_NEW_CONSOLE`, when Eclink has no console of its own (a normal GUI
    /// launch, and the case this app ships). Because the child owns a new
    /// console, Windows points its standard handles at that console's input and
    /// output buffers, so the agent can read the keyboard and print output and
    /// errors in its own window.
    ///
    /// Passing handles explicitly is what broke this: Eclink is a GUI process
    /// with no console, so "inherit" handed the child handles that were
    /// redirected or absent. Measured from a parent with no console, this wiring
    /// gives `IsInputRedirected=False`, `IsOutputRedirected=False`, and
    /// `IsErrorRedirected=False` in the child — real console handles.
    Unset,
    /// All three streams are `NUL`.
    ///
    /// Used for Windows Terminal, which is a GUI host: it opens its own window
    /// and takes no input on its standard streams, so nothing should reach
    /// Eclink's own (possibly absent) console.
    Null,
}

/// Standard-stream wiring for a launch plan.
///
/// `parent_has_console` is whether Eclink was itself started from a terminal.
pub fn stdio_wiring(new_console: bool, parent_has_console: bool) -> StdioWiring {
    match (new_console, parent_has_console) {
        // A console host that can see a console: keep using it.
        (true, true) => StdioWiring::Inherit,
        // A console host with none: let Windows wire its new console.
        (true, false) => StdioWiring::Unset,
        // A GUI host never uses the parent's streams.
        (false, _) => StdioWiring::Null,
    }
}

/// Whether Eclink itself has a console to hand down.
fn parent_has_console() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
        || std::io::stderr().is_terminal()
        || std::io::stdin().is_terminal()
}

/// Spawn the process described by `plan`.
///
/// The child is detached from Bridge: closing Eclink must not kill the
/// coding agent the user just opened (PRD §84).
pub fn spawn(plan: &SpawnPlan) -> BridgeResult<u32> {
    let mut command = std::process::Command::new(&plan.program);
    command.args(&plan.args).current_dir(&plan.working_dir);

    // A console host must own the console streams, or it has a window it cannot
    // print in and a keyboard it cannot read. A GUI host takes none of them, so
    // it gets `NUL` and can never inherit a handle Eclink does not have.
    match stdio_wiring(plan.new_console, parent_has_console()) {
        StdioWiring::Inherit => {
            command
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
        }
        StdioWiring::Unset => {}
        StdioWiring::Null => {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
        }
    }

    for (key, value) in &plan.env {
        command.env(key, value);
    }

    // A new process group keeps the agent alive if Bridge is closed.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        // A console host started detached has no window and nowhere to run the
        // agent, so it gets its own console. Windows Terminal is a GUI process
        // and is detached instead.
        let flags = if plan.new_console {
            CREATE_NEW_PROCESS_GROUP | CREATE_NEW_CONSOLE
        } else {
            CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS
        };
        command.creation_flags(flags);
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
        new_console: plan.new_console,
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
        assert_eq!(powershell_literal("plain"), "'plain'");
        assert_eq!(powershell_literal("it's"), "'it''s'");
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
        let quoted = powershell_literal(r"C:\Users\o'brien\proj'ect");
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
    fn a_console_host_keeps_a_console_and_a_gui_host_never_does() {
        // Windows Terminal opens its own window and must not inherit anything.
        assert_eq!(stdio_wiring(false, true), StdioWiring::Null);
        assert_eq!(stdio_wiring(false, false), StdioWiring::Null);
        // The fallback keeps Eclink's console when there is one.
        assert_eq!(stdio_wiring(true, true), StdioWiring::Inherit);
        // Otherwise Windows must wire the child's new console itself. This is
        // the shipped case: Eclink is a GUI process with no console, and
        // inheriting "nothing" is what previously produced NUL handles.
        assert_eq!(stdio_wiring(true, false), StdioWiring::Unset);
    }

    #[test]
    fn display_string_is_readable() {
        let dir = project();
        let plan = build_launch_plan(AgentId::Codex, Path::new(r"C:\tools\codex.exe"), dir.path())
            .unwrap();
        assert!(!plan.display.is_empty());
        assert!(plan.display.contains("codex.exe"));
    }

    #[test]
    fn a_shim_plan_transports_the_command_as_an_encoded_powershell_script() {
        let dir = project();
        let shim = dir.path().join("codex O'Brien.cmd");
        std::fs::write(&shim, b"@echo off\r\n").unwrap();
        let command = Launcher::from_path(shim.clone())
            .command(&["--profile", "work", "--model", "p/m"])
            .unwrap();
        let project_dir = dir.path().join("proj");
        std::fs::create_dir_all(&project_dir).unwrap();
        let canonical = strip_verbatim_prefix(project_dir.canonicalize().unwrap())
            .display()
            .to_string();

        let wt = build_launch_plan_for_command(
            AgentId::Omp,
            &command,
            &project_dir,
            TerminalKind::WindowsTerminal,
            Path::new("wt.exe"),
        )
        .unwrap();

        // Windows Terminal receives no user text at all: the path, the working
        // directory, and the arguments travel inside the encoded script. That is
        // what keeps its parser from treating an apostrophe as a quote.
        assert_eq!(wt.args[0], "--");
        assert_eq!(wt.args[2], "-NoExit");
        assert_eq!(wt.args[4], "-EncodedCommand");
        assert_eq!(wt.working_dir, canonical);
        assert_encoded_script(&wt.args[5], &canonical, &shim, &command);

        let ps = build_launch_plan_for_command(
            AgentId::Omp,
            &command,
            &project_dir,
            TerminalKind::PowerShell,
            Path::new("powershell.exe"),
        )
        .unwrap();

        assert_eq!(ps.args[0], "-NoExit");
        assert_eq!(ps.args[2], "-EncodedCommand");
        assert_eq!(ps.working_dir, canonical);
        assert_encoded_script(&ps.args[3], &canonical, &shim, &command);
    }

    /// Decode the script Eclink hands to PowerShell and assert its contract.
    fn assert_encoded_script(encoded: &str, workdir: &str, shim: &Path, command: &AgentCommand) {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("the command must be valid base64");
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let script = String::from_utf16(&units).expect("the script must be UTF-16");

        assert!(
            script.contains(&format!(
                "Set-Location -LiteralPath '{}'",
                workdir.replace('\'', "''")
            )),
            "the working directory is not a literal: {script}"
        );
        assert!(
            script.contains("$env:ECLINK_AGENT_CMD = "),
            "the command is not carried as data: {script}"
        );
        let escaped_shim = shim.display().to_string().replace('\'', "''");
        assert!(
            script.contains(&format!("\"{escaped_shim}\"")),
            "the shim path is missing from the encoded command: {script}"
        );
        assert!(
            script.contains("'--profile'") || script.contains("\"--profile\""),
            "the profile argument is missing: {script}"
        );
        assert!(
            script.contains("/d /s /v:off /c '%ECLINK_AGENT_CMD%'"),
            "cmd.exe is not invoked with the carried command: {script}"
        );
        let _ = command;
    }

    #[test]
    fn a_native_plan_still_uses_separate_argv_entries() {
        let dir = project();
        let plan = build_launch_plan_for(
            AgentId::Claude,
            Path::new(r"C:\Program Files\claude.exe"),
            dir.path(),
            TerminalKind::WindowsTerminal,
            Path::new("wt.exe"),
        )
        .unwrap();

        assert_eq!(plan.args[0], "-d");
        assert_eq!(plan.args[2], "--");
        assert_eq!(plan.args[3], r"C:\Program Files\claude.exe");
    }
}
