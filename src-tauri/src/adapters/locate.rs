//! Where each coding agent is installed.
//!
//! Detection used to have exactly one answer: whatever `where.exe` returned, or
//! a known location under the user profile. That is wrong for anyone who
//! installed an agent somewhere else, and the GUI process often inherits a PATH
//! that does not contain the same entries as the user's shell.
//!
//! This module adds an explicit, per-agent location choice. Two rules make it
//! safe to reason about:
//!
//! * **A manual choice wins outright.** When the user has set a location, only
//!   that location is examined. Eclink never falls back to PATH, because doing so
//!   would silently run a different installation than the one the user pointed
//!   at. A broken manual location is reported, not papered over.
//! * **Automatic detection is unchanged.** With no manual choice, the existing
//!   lookup order is used exactly as before, so an installation that worked
//!   before this feature still resolves to the same executable.
//!
//! Resolution is deliberately split from probing: this module answers "which
//! file would be run" without spawning anything, so the rules can be tested
//! against temporary directories and the command that actually runs is the same
//! one detection, version probing, OMP registry validation, and Launch use.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::adapters::detect::ProbeError;
use crate::adapters::{AgentDetection, AgentId};
use crate::error::{BridgeError, BridgeResult};

/// Launcher file names accepted inside a chosen installation folder.
///
/// These are the names detection already looked for before this feature, in the
/// same order. A folder is only ever searched for the agent's own launchers, so
/// choosing a broad folder such as `C:\Tools` cannot pick up an unrelated
/// program's executable.
pub const CODEX_LAUNCHER_NAMES: &[&str] = &["codex.exe", "codex.cmd", "codex"];
pub const CLAUDE_LAUNCHER_NAMES: &[&str] = &["claude.exe", "claude.cmd", "claude"];
pub const OMP_LAUNCHER_NAMES: &[&str] = &["omp.exe"];

/// The longest path Windows accepts through the NT namespace.
const MAX_PATH_CHARS: usize = 32_767;

/// Suffixes a directly chosen launcher may have.
///
/// `.cmd` and `.bat` are Windows shims (npm and Bun install both use them). They
/// are run through `cmd.exe`, never through `CreateProcess` directly, which
/// cannot execute a batch file.
const LAUNCHER_EXTENSIONS: &[&str] = &["exe", "cmd", "bat", "com"];

pub fn launcher_names(agent: AgentId) -> &'static [&'static str] {
    match agent {
        AgentId::Codex => CODEX_LAUNCHER_NAMES,
        AgentId::Claude => CLAUDE_LAUNCHER_NAMES,
        AgentId::Omp => OMP_LAUNCHER_NAMES,
    }
}

/// How a saved location should be interpreted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationKind {
    /// A folder that contains the launcher, or a `bin` subfolder that does.
    #[default]
    Folder,
    /// The launcher file itself, for layouts where it sits deeper than `bin`.
    Launcher,
}

/// One agent's saved location, as stored in `app.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLocation {
    /// Absolute Windows path, as the user chose it.
    pub path: String,
    #[serde(default)]
    pub kind: LocationKind,
}

impl AgentLocation {
    pub fn new(path: impl Into<String>, kind: LocationKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }

    pub fn mode(&self) -> LocationMode {
        match self.kind {
            LocationKind::Folder => LocationMode::Folder,
            LocationKind::Launcher => LocationMode::Launcher,
        }
    }
}

/// Every agent's saved location. An absent entry means automatic detection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLocations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<AgentLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<AgentLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omp: Option<AgentLocation>,
}

impl AgentLocations {
    pub fn get(&self, agent: AgentId) -> Option<&AgentLocation> {
        match agent {
            AgentId::Codex => self.codex.as_ref(),
            AgentId::Claude => self.claude.as_ref(),
            AgentId::Omp => self.omp.as_ref(),
        }
    }

    /// Replace one agent's location, leaving every other agent alone.
    pub fn set(&mut self, agent: AgentId, location: Option<AgentLocation>) {
        match agent {
            AgentId::Codex => self.codex = location,
            AgentId::Claude => self.claude = location,
            AgentId::Omp => self.omp = location,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.codex.is_none() && self.claude.is_none() && self.omp.is_none()
    }
}

/// Where the effective launcher came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationMode {
    /// Nothing was saved for this agent; the pre-existing lookup order decides.
    Automatic,
    /// The saved path is a folder.
    Folder,
    /// The saved path is the launcher itself.
    Launcher,
}

/// Why a saved location produced no usable launcher.
///
/// These are stable machine codes. The interface translates each one, so the
/// explanation follows the chosen language instead of being an English sentence
/// stored in an error path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationProblem {
    /// The saved path does not exist any more.
    Missing,
    /// The path exists, but holds no supported launcher for this agent.
    NotALauncher,
    /// The launcher exists but could not be executed.
    Unusable,
    /// The launcher did not answer within the probe timeout.
    Timeout,
    /// The saved path is not a path Eclink can safely execute.
    UnsupportedPath,
}

impl LocationProblem {
    pub fn describe(self, agent: AgentId, detail: &str) -> String {
        let name = agent.display_name();
        match self {
            LocationProblem::Missing => format!(
                "The saved {name} install location no longer exists: {detail}. \
                 Choose it again in Settings, or switch that agent back to automatic detection."
            ),
            LocationProblem::NotALauncher => format!(
                "Eclink found no supported {name} launcher at {detail}. \
                 Choose the launcher file directly in Settings, or switch back to automatic detection."
            ),
            LocationProblem::Unusable => format!(
                "The {name} launcher at {detail} could not be run. \
                 Check the file in Settings, or switch back to automatic detection."
            ),
            LocationProblem::Timeout => format!(
                "The {name} launcher at {detail} did not respond in time. \
                 Check the file in Settings, or switch back to automatic detection."
            ),
            LocationProblem::UnsupportedPath => format!(
                "The saved {name} install location is not usable: {detail}. \
                 Choose another location in Settings, or switch back to automatic detection."
            ),
        }
    }
}

/// A launcher Eclink has decided to run, plus how to run it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    path: PathBuf,
    kind: LauncherKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherKind {
    /// A Windows executable that `CreateProcess` can start directly.
    Native,
    /// A `.cmd` or `.bat` shim, which must be run through `cmd.exe`.
    CommandShim,
}

impl Launcher {
    /// Classify a resolved path. Never fails; an unusable shim fails at
    /// [`Self::check_executable`] or at the probe.
    pub fn from_path(path: PathBuf) -> Self {
        let kind = match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("cmd") | Some("bat") => LauncherKind::CommandShim,
            _ => LauncherKind::Native,
        };
        Self { path, kind }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> LauncherKind {
        self.kind
    }

    /// Whether Eclink can execute this launcher safely.
    ///
    /// A `.cmd`/`.bat` runs through `cmd.exe`, which expands `%NAME%` before it
    /// runs the file. A path containing `%` would therefore be rewritten into
    /// something else, so it is refused rather than guessed at. A double quote is
    /// refused for the same reason; Windows does not allow it in a real path, but
    /// a hand-edited `app.json` is not a real path.
    pub fn check_executable(&self) -> Result<(), String> {
        if self.kind != LauncherKind::CommandShim {
            return Ok(());
        }
        let text = self.path.to_string_lossy();
        if text.contains('%') {
            return Err(
                "A .cmd or .bat launcher cannot be used from a path containing %, because the \
                 Windows command interpreter would expand it. Choose the installation folder or \
                 the .exe file instead."
                    .to_string(),
            );
        }
        if text.contains('"') {
            return Err("A .cmd or .bat launcher path cannot contain a double quote.".to_string());
        }
        Ok(())
    }

    /// Program and argv used to execute the launcher with `args`.
    ///
    /// The path is always one argv entry; nothing is joined into a free-form
    /// command line. `call` is required for a batch file so `cmd.exe` runs it
    /// with the remaining arguments instead of parsing the quoted path as part of
    /// its own `/c` command string.
    pub fn invocation(&self, args: &[&str]) -> (PathBuf, Vec<String>) {
        match self.kind {
            LauncherKind::Native => (
                self.path.clone(),
                args.iter().map(|arg| arg.to_string()).collect(),
            ),
            LauncherKind::CommandShim => {
                let interpreter = std::env::var_os("ComSpec")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("cmd.exe"));
                let mut argv = vec![
                    "/d".to_string(),
                    "/c".to_string(),
                    "call".to_string(),
                    self.path.to_string_lossy().into_owned(),
                ];
                argv.extend(args.iter().map(|arg| arg.to_string()));
                (interpreter, argv)
            }
        }
    }
}

/// What resolution concluded for one agent.
#[derive(Debug, Clone)]
pub struct Resolution {
    pub mode: LocationMode,
    /// The path the user saved, when there is one.
    pub configured_path: Option<String>,
    /// The launcher that would be run, when one was found.
    pub launcher: Option<Launcher>,
    /// Why no launcher was found, when the saved location is at fault.
    pub problem: Option<LocationProblem>,
    /// A path, agent-specific list, or rejection reason for `problem`.
    pub detail: Option<String>,
}

impl Resolution {
    fn automatic(launcher: Option<Launcher>) -> Self {
        Self {
            mode: LocationMode::Automatic,
            configured_path: None,
            launcher,
            problem: None,
            detail: None,
        }
    }

    fn manual(configured: &AgentLocation, launcher: Option<Launcher>) -> Self {
        Self {
            mode: configured.mode(),
            configured_path: Some(configured.path.clone()),
            launcher,
            problem: None,
            detail: None,
        }
    }

    fn failed(configured: &AgentLocation, problem: LocationProblem, detail: String) -> Self {
        Self {
            mode: configured.mode(),
            configured_path: Some(configured.path.clone()),
            launcher: None,
            problem: Some(problem),
            detail: Some(detail),
        }
    }
}

/// Resolve the launcher for `agent`.
///
/// `automatic` is only called when no location is saved. That is what guarantees
/// a manual choice can never silently degrade into a PATH lookup: the fallback
/// is not merely deprioritised, it is not consulted.
pub fn resolve<F>(agent: AgentId, configured: Option<&AgentLocation>, automatic: F) -> Resolution
where
    F: FnOnce() -> Option<PathBuf>,
{
    let Some(configured) = configured else {
        return Resolution::automatic(automatic().map(Launcher::from_path));
    };

    match resolve_manual(agent, configured) {
        Ok(launcher) => Resolution::manual(configured, Some(launcher)),
        Err((problem, detail)) => Resolution::failed(configured, problem, detail),
    }
}

/// Resolve a saved location without consulting PATH.
fn resolve_manual(
    agent: AgentId,
    configured: &AgentLocation,
) -> Result<Launcher, (LocationProblem, String)> {
    let path = validate_absolute_path(&configured.path)
        .map_err(|reason| (LocationProblem::UnsupportedPath, reason))?;

    match configured.kind {
        LocationKind::Folder => {
            if !path.exists() {
                return Err((LocationProblem::Missing, path.display().to_string()));
            }
            if !path.is_dir() {
                return Err((
                    LocationProblem::NotALauncher,
                    format!("{} (a file, not a folder)", path.display()),
                ));
            }
            folder_candidates(&path, launcher_names(agent))
                .into_iter()
                .find(|candidate| candidate.is_file())
                .map(Launcher::from_path)
                .ok_or_else(|| (LocationProblem::NotALauncher, path.display().to_string()))
        }
        LocationKind::Launcher => {
            if !path.exists() {
                return Err((LocationProblem::Missing, path.display().to_string()));
            }
            if !path.is_file() {
                return Err((
                    LocationProblem::NotALauncher,
                    format!("{} is a folder, not a launcher file", path.display()),
                ));
            }
            let launcher = Launcher::from_path(path);
            launcher
                .check_executable()
                .map_err(|reason| (LocationProblem::UnsupportedPath, reason))?;
            Ok(launcher)
        }
    }
}

/// Files a chosen folder could hold, in priority order: the folder itself, then
/// its `bin` subfolder.
///
/// Only the selected folder and one documented subfolder are examined. Recursing
/// further would turn a mistyped path such as `C:\` into an expensive scan, and
/// a launcher found three levels down is not obviously the one the user meant.
pub fn folder_candidates(folder: &Path, names: &[&str]) -> Vec<PathBuf> {
    let mut candidates = Vec::with_capacity(names.len() * 2);
    for name in names {
        candidates.push(folder.join(name));
    }
    let bin = folder.join("bin");
    for name in names {
        candidates.push(bin.join(name));
    }
    candidates
}

/// Validate and normalise a location coming from the interface.
///
/// A missing path is accepted: the user may be configuring a machine before the
/// agent is installed, and a path can disappear later anyway. What is rejected
/// here is anything Eclink could not address unambiguously - a relative path, a
/// device path, a folder chosen as a launcher, or a launcher chosen as a folder.
pub fn validate_location(location: AgentLocation) -> BridgeResult<AgentLocation> {
    let path = validate_absolute_path(&location.path).map_err(|reason| {
        BridgeError::validation(reason).with_hint(
            "Pick the folder with the button, or paste a full path such as C:\\Tools\\codex.",
        )
    })?;

    match location.kind {
        LocationKind::Launcher => {
            if !has_launcher_extension(&path) {
                return Err(BridgeError::validation(
                    "Choose a launcher file with a .exe, .cmd, .bat, or .com extension.",
                )
                .with_hint("Pick the executable itself, or choose its folder instead."));
            }
            if path.is_dir() {
                return Err(BridgeError::validation(format!(
                    "{} is a folder, not a launcher file.",
                    path.display()
                ))
                .with_hint("Choose it as an installation folder instead."));
            }
            Launcher::from_path(path.clone())
                .check_executable()
                .map_err(|reason| {
                    BridgeError::validation(reason).with_hint(
                        "Choose the installation folder or the .exe file, which does not go through cmd.exe.",
                    )
                })?;
        }
        LocationKind::Folder => {
            if path.is_file() {
                return Err(BridgeError::validation(format!(
                    "{} is a file, not an installation folder.",
                    path.display()
                ))
                .with_hint("Choose it as a launcher file instead."));
            }
        }
    }

    Ok(AgentLocation {
        path: path.display().to_string(),
        kind: location.kind,
    })
}

/// Validate an absolute Windows path without touching the filesystem.
pub fn validate_absolute_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Enter a full installation path.".to_string());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("That path contains a control character.".to_string());
    }
    if trimmed.chars().count() > MAX_PATH_CHARS {
        return Err("That path is longer than Windows can address.".to_string());
    }
    if trimmed.contains('"') {
        return Err("A Windows path cannot contain a double quote.".to_string());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("Enter an absolute path, for example C:\\Tools\\codex.".to_string());
    }
    if trimmed.starts_with(r"\\.\") {
        return Err("Device paths that start with \\.\\ are not supported.".to_string());
    }
    Ok(crate::process::strip_verbatim_prefix(path))
}

fn has_launcher_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            LAUNCHER_EXTENSIONS
                .iter()
                .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        })
        .unwrap_or(false)
}

/// Apply a resolution to a detection result.
///
/// `probe` is the outcome of running the launcher's version command, when one was
/// run. In automatic mode a probe failure only means "version unknown", exactly
/// as before. In manual mode it means the saved location cannot be executed, so
/// the agent is reported as not installed with an actionable explanation: the
/// user pointed Eclink at this file and needs to know it does not work.
///
/// Returns whether the agent counts as installed.
pub fn apply_resolution(
    detection: &mut AgentDetection,
    resolution: &Resolution,
    probe: Option<&ProbeError>,
) -> bool {
    detection.location_mode = Some(resolution.mode);
    detection.configured_location = resolution.configured_path.clone();
    detection.location_problem = resolution.problem;
    detection.location_detail = resolution.detail.clone();

    if resolution.mode == LocationMode::Automatic {
        return resolution.launcher.is_some();
    }

    if let Some(problem) = resolution.problem {
        let detail = resolution.detail.clone().unwrap_or_default();
        detection.note = Some(problem.describe(detection.agent, &detail));
        return false;
    }

    if let Some(probe) = probe {
        let problem = probe.location_problem();
        let path = resolution.configured_path.clone().unwrap_or_default();
        detection.location_problem = Some(problem);
        detection.location_detail = Some(path.clone());
        detection.note = Some(problem.describe(detection.agent, &path));
        return false;
    }

    true
}

/// Shared, live view of the saved locations.
///
/// Adapters are built once and live for the process lifetime, so they cannot read
/// `AppState` without a cycle. This handle is the smallest thing that lets them
/// see a change made in Settings without being rebuilt.
#[derive(Debug, Clone, Default)]
pub struct LocationStore {
    inner: Arc<RwLock<AgentLocations>>,
}

impl LocationStore {
    pub fn new(locations: AgentLocations) -> Self {
        Self {
            inner: Arc::new(RwLock::new(locations)),
        }
    }

    pub fn snapshot(&self) -> AgentLocations {
        self.inner
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn get(&self, agent: AgentId) -> Option<AgentLocation> {
        self.snapshot().get(agent).cloned()
    }

    /// Replace one agent's location. Persistence is the caller's job, so this is
    /// only reached after `app.json` was written successfully.
    pub fn set(&self, agent: AgentId, location: Option<AgentLocation>) {
        match self.inner.write() {
            Ok(mut guard) => guard.set(agent, location),
            Err(poisoned) => {
                // A panic while holding the lock must not wedge detection for the
                // rest of the session; recover the value and keep going.
                tracing::warn!(target: "eclink", "agent location lock was poisoned; recovering");
                poisoned.into_inner().set(agent, location);
            }
        }
    }

    /// Resolve an agent against the current settings.
    pub fn resolve<F>(&self, agent: AgentId, automatic: F) -> Resolution
    where
        F: FnOnce() -> Option<PathBuf>,
    {
        let configured = self.get(agent);
        resolve(agent, configured.as_ref(), automatic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn write_launcher(dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, b"@echo off\r\necho codex-cli 9.9.9\r\n").unwrap();
        path
    }

    fn configured(path: &Path, kind: LocationKind) -> AgentLocation {
        AgentLocation::new(path.display().to_string(), kind)
    }

    #[test]
    fn absolute_paths_are_accepted_and_relative_paths_are_not() {
        for accepted in [
            r"C:\Tools\codex",
            r"C:\Program Files\codex",
            r"D:\Ünïcødé\O'Brien codex",
            r"\\server\share\codex",
        ] {
            assert!(
                validate_absolute_path(accepted).is_ok(),
                "{accepted} should be accepted"
            );
        }

        for rejected in [
            "",
            "   ",
            r"Tools\codex",
            r".\codex",
            r"C:codex",
            r"\Tools\codex",
        ] {
            assert!(
                validate_absolute_path(rejected).is_err(),
                "{rejected} should be rejected"
            );
        }
    }

    #[test]
    fn a_path_with_a_control_character_or_double_quote_is_refused() {
        assert!(validate_absolute_path("C:\\Tools\\code\nx").is_err());
        assert!(validate_absolute_path("C:\\Tools\\co\"dex").is_err());
    }

    #[test]
    fn device_paths_are_refused() {
        assert!(validate_absolute_path(r"\\.\C:\Tools").is_err());
    }

    #[test]
    fn a_verbatim_prefix_is_removed() {
        let path = validate_absolute_path(r"\\?\C:\Tools\codex").unwrap();
        assert_eq!(path, PathBuf::from(r"C:\Tools\codex"));
    }

    #[test]
    fn a_folder_is_searched_for_the_agents_own_launchers_only() {
        let dir = tempfile::tempdir().unwrap();
        let expected = write_launcher(dir.path(), "codex.exe");
        write_launcher(dir.path(), "unrelated.exe");

        let resolution = resolve(
            AgentId::Codex,
            Some(&configured(dir.path(), LocationKind::Folder)),
            || panic!("automatic detection must not run when a folder is saved"),
        );

        assert_eq!(resolution.mode, LocationMode::Folder);
        assert_eq!(resolution.launcher.unwrap().path(), expected);
        assert!(resolution.problem.is_none());
    }

    #[test]
    fn a_folder_finds_a_launcher_inside_bin() {
        let dir = tempfile::tempdir().unwrap();
        let expected = write_launcher(&dir.path().join("bin"), "claude.cmd");

        let resolution = resolve(
            AgentId::Claude,
            Some(&configured(dir.path(), LocationKind::Folder)),
            || None,
        );

        assert_eq!(resolution.launcher.unwrap().path(), expected);
    }

    #[test]
    fn a_folder_without_a_supported_launcher_is_reported_as_such() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();

        let resolution = resolve(
            AgentId::Codex,
            Some(&configured(dir.path(), LocationKind::Folder)),
            || Some(PathBuf::from(r"C:\elsewhere\codex.exe")),
        );

        assert!(resolution.launcher.is_none());
        assert_eq!(resolution.problem, Some(LocationProblem::NotALauncher));
        assert!(resolution.configured_path.is_some());
    }

    #[test]
    fn a_missing_folder_is_reported_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone");

        let resolution = resolve(
            AgentId::Codex,
            Some(&configured(&missing, LocationKind::Folder)),
            || Some(PathBuf::from(r"C:\elsewhere\codex.exe")),
        );

        assert_eq!(resolution.problem, Some(LocationProblem::Missing));
    }

    #[test]
    fn a_launcher_file_is_used_exactly_as_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = write_launcher(&dir.path().join("a b O'Brien Ünïcødé"), "npm shim.cmd");

        let resolution = resolve(
            AgentId::Codex,
            Some(&configured(&launcher, LocationKind::Launcher)),
            || None,
        );

        let resolved = resolution.launcher.unwrap();
        assert_eq!(resolved.path(), launcher);
        assert_eq!(resolved.kind(), LauncherKind::CommandShim);
    }

    #[test]
    fn a_manual_choice_never_falls_back_to_automatic_detection() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("removed");
        let automatic_calls = Cell::new(0);

        let resolution = resolve(
            AgentId::Codex,
            Some(&configured(&broken, LocationKind::Folder)),
            || {
                automatic_calls.set(automatic_calls.get() + 1);
                Some(PathBuf::from(r"C:\other\codex.exe"))
            },
        );

        assert!(resolution.launcher.is_none());
        assert_eq!(
            automatic_calls.get(),
            0,
            "the PATH lookup must not even run for a saved location"
        );
    }

    #[test]
    fn clearing_the_saved_location_restores_automatic_detection() {
        let auto = PathBuf::from(r"C:\Users\Test\AppData\Local\Programs\codex.exe");
        let resolution = resolve(AgentId::Codex, None, || Some(auto.clone()));

        assert_eq!(resolution.mode, LocationMode::Automatic);
        assert_eq!(resolution.launcher.unwrap().path(), auto);
        assert!(resolution.configured_path.is_none());
    }

    #[test]
    fn a_shim_path_with_a_percent_sign_is_refused_rather_than_guessed() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = write_launcher(dir.path(), "100% codex.cmd");

        let resolution = resolve(
            AgentId::Codex,
            Some(&configured(&launcher, LocationKind::Launcher)),
            || None,
        );

        assert_eq!(
            resolution.problem,
            Some(LocationProblem::UnsupportedPath),
            "cmd.exe would expand %NAME% in this path"
        );
    }

    #[test]
    fn a_launcher_needs_an_executable_extension() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("codex.txt");
        std::fs::write(&script, b"x").unwrap();

        let err = validate_location(configured(&script, LocationKind::Launcher)).unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Validation);
        assert!(err.message.contains(".exe"), "{}", err.message);
    }

    #[test]
    fn a_file_chosen_as_a_folder_is_refused_with_the_right_hint() {
        let dir = tempfile::tempdir().unwrap();
        let file = write_launcher(dir.path(), "codex.exe");

        let err = validate_location(configured(&file, LocationKind::Folder)).unwrap_err();
        assert!(
            err.hint
                .as_deref()
                .unwrap_or_default()
                .contains("launcher file"),
            "{err:?}"
        );
    }

    #[test]
    fn a_missing_path_can_still_be_saved() {
        // The user may configure a machine before the agent is installed, and a
        // folder can disappear afterwards anyway. The status carries the problem.
        let dir = tempfile::tempdir().unwrap();
        let future = dir.path().join("not-installed-yet");
        let saved = validate_location(configured(&future, LocationKind::Folder)).unwrap();
        assert_eq!(saved.kind, LocationKind::Folder);
    }

    #[test]
    fn locations_round_trip_and_leave_other_agents_alone() {
        let mut locations = AgentLocations::default();
        locations.set(
            AgentId::Codex,
            Some(AgentLocation::new(r"D:\Tools\codex", LocationKind::Folder)),
        );
        locations.set(
            AgentId::Omp,
            Some(AgentLocation::new(
                r"D:\Tools\omp\omp.exe",
                LocationKind::Launcher,
            )),
        );

        let json = serde_json::to_string(&locations).unwrap();
        let parsed: AgentLocations = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, locations);
        assert!(parsed.claude.is_none());

        let mut cleared = parsed.clone();
        cleared.set(AgentId::Codex, None);
        assert!(cleared.codex.is_none());
        assert!(
            cleared.omp.is_some(),
            "clearing one agent must not clear another"
        );
    }

    #[test]
    fn an_older_settings_file_without_locations_reads_as_automatic() {
        let parsed: AgentLocations = serde_json::from_str("{}").unwrap();
        assert!(parsed.is_empty());
        assert!(parsed.get(AgentId::Codex).is_none());
    }

    #[test]
    fn the_store_shares_one_value_between_clones() {
        let store = LocationStore::default();
        let clone = store.clone();
        clone.set(
            AgentId::Claude,
            Some(AgentLocation::new(r"D:\Claude", LocationKind::Folder)),
        );
        assert!(store.get(AgentId::Claude).is_some());
    }

    #[test]
    fn a_probe_failure_in_manual_mode_is_reported_as_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = write_launcher(dir.path(), "codex.exe");
        let resolution = resolve(
            AgentId::Codex,
            Some(&configured(&launcher, LocationKind::Launcher)),
            || None,
        );

        let mut detection = AgentDetection::not_installed(AgentId::Codex, "");
        let installed = apply_resolution(&mut detection, &resolution, Some(&ProbeError::TimedOut));

        assert!(!installed);
        assert_eq!(detection.location_problem, Some(LocationProblem::Timeout));
        assert!(detection.note.unwrap().contains("did not respond"));
    }

    #[test]
    fn a_probe_failure_in_automatic_mode_keeps_the_old_semantics() {
        let resolution = Resolution::automatic(Some(Launcher::from_path(PathBuf::from(
            r"C:\Tools\codex.exe",
        ))));
        let mut detection = AgentDetection::not_installed(AgentId::Codex, "");
        let installed = apply_resolution(&mut detection, &resolution, Some(&ProbeError::TimedOut));

        assert!(
            installed,
            "automatic detection used to ignore the probe result"
        );
        assert!(detection.location_problem.is_none());
    }
}
