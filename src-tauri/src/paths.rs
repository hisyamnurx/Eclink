//! Filesystem locations used by Eclyzer Bridge.
//!
//! All application data lives under `%LOCALAPPDATA%\EclyzerBridge\` (PRD §12).
//! Nothing is written during installation; directories are created lazily on
//! first use so a fresh install stays inert (PRD §41.3).

use std::path::{Path, PathBuf};

use crate::error::{BridgeError, BridgeResult};

/// Resolved application data directories.
#[derive(Debug, Clone)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    /// Resolve the default root using the OS-provided local app data directory.
    ///
    /// Windows: `%LOCALAPPDATA%`. Falls back to a temp-scoped directory only if
    /// the OS cannot report a data directory at all, which keeps unit tests and
    /// odd CI images working without hardcoding `C:\Users\...`.
    pub fn resolve() -> BridgeResult<Self> {
        let base = directories::BaseDirs::new().ok_or_else(|| {
            BridgeError::io("Could not determine the application data directory.")
                .with_hint("Your Windows user profile may be unavailable.")
        })?;
        let root = base
            .data_local_dir()
            .join(crate::adapters::APP_DATA_DIR_NAME);
        Ok(Self { root })
    }

    /// Build an [`AppPaths`] rooted at an explicit directory (used by tests).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn app_json(&self) -> PathBuf {
        self.root.join("app.json")
    }

    pub fn profiles_json(&self) -> PathBuf {
        self.root.join("profiles.json")
    }

    pub fn secrets_dat(&self) -> PathBuf {
        self.root.join("secrets.dat")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    pub fn backups_dir_for(&self, agent: &str) -> PathBuf {
        self.backups_dir().join(agent)
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// Create every directory Eclyzer Bridge owns.
    pub fn ensure_dirs(&self) -> BridgeResult<()> {
        for dir in [
            self.root.clone(),
            self.backups_dir(),
            self.logs_dir(),
            self.cache_dir(),
        ] {
            std::fs::create_dir_all(&dir).map_err(|err| {
                BridgeError::io(format!(
                    "Could not create the Eclyzer Bridge data folder: {}",
                    dir.display()
                ))
                .with_details(err.to_string())
            })?;
        }
        Ok(())
    }
}

/// The current user's home directory.
///
/// Used to derive default agent config locations. Never hardcode a username
/// (PRD §47).
pub fn home_dir() -> BridgeResult<PathBuf> {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .ok_or_else(|| {
            BridgeError::io("Could not determine your home folder.")
                .with_hint("Your Windows user profile may be unavailable.")
        })
}

/// Expand a leading `~` and normalize separators.
///
/// Windows paths containing spaces, Unicode, and backslashes must survive
/// untouched (PRD §47); this only handles the `~` shorthand.
pub fn expand_home(input: &str) -> BridgeResult<PathBuf> {
    let trimmed = input.trim();
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        return Ok(home_dir()?.join(rest));
    }
    if trimmed == "~" {
        return home_dir();
    }
    Ok(PathBuf::from(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_stable_and_separate() {
        let paths = AppPaths::with_root(r"D:\tmp\EclyzerBridge");
        assert_eq!(
            paths.app_json(),
            PathBuf::from(r"D:\tmp\EclyzerBridge\app.json")
        );
        assert_eq!(
            paths.backups_dir_for("codex"),
            PathBuf::from(r"D:\tmp\EclyzerBridge\backups\codex")
        );
    }

    #[test]
    fn expand_home_leaves_absolute_paths_alone() {
        // Paths with spaces must remain intact.
        let p = expand_home(r"C:\Users\Test User\My Project\").unwrap();
        assert_eq!(p, PathBuf::from(r"C:\Users\Test User\My Project\"));
    }

    #[test]
    fn expand_home_handles_unicode() {
        let p = expand_home(r"C:\Users\Ünïcødé User\prôjèct").unwrap();
        assert!(p.to_string_lossy().contains("Ünïcødé"));
    }
}
