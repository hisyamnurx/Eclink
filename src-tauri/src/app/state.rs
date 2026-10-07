//! Shared, long-lived application state.

use std::sync::Arc;

use crate::adapters::{AgentAdapter, AgentId, ClaudeAdapter, CodexAdapter};
use crate::backup::BackupManager;
use crate::diagnostics::{LogGuard, LogLevel};
use crate::error::BridgeResult;
use crate::paths::AppPaths;
use crate::profiles::ProfileStore;
use crate::secrets::SecretStore;

/// Everything a Tauri command needs, cloned cheaply per call.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

struct AppStateInner {
    paths: AppPaths,
    profiles: ProfileStore,
    secrets: SecretStore,
    /// One adapter per agent, built once. Adapters are stateless apart from the
    /// backup manager they share.
    codex: CodexAdapter,
    claude: ClaudeAdapter,
    settings: std::sync::RwLock<AppSettings>,
    /// Held for the process lifetime so the log flusher stays alive.
    _log_guard: std::sync::Mutex<Option<LogGuard>>,
}

/// User-configurable settings persisted in `app.json` (PRD §44.2, §54).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub schema_version: u32,
    /// Automatic update check on startup, at most once per 24h (PRD §40.2).
    #[serde(default = "default_true")]
    pub update_check: bool,
    #[serde(default)]
    pub last_update_check: Option<String>,
    #[serde(default)]
    pub last_project_directory: Option<String>,
    /// Remember the last project folder between launches (PRD §54).
    #[serde(default = "default_true")]
    pub remember_project_directory: bool,
    #[serde(default)]
    pub log_level: LogLevel,
    /// Whether the user has dismissed the first-run screen (PRD §81).
    #[serde(default)]
    pub first_run_complete: bool,
    /// Shown once: "Testing may make a small billable API request." (PRD §22.3)
    #[serde(default)]
    pub cost_notice_seen: bool,
    /// Whether the user has been told about the insecure-HTTP risk generally.
    #[serde(default)]
    pub insecure_endpoint_notice_seen: bool,
    /// Interface language, as a BCP-47-ish tag: `en` or `id` (PRD §55).
    ///
    /// Defaulted rather than required, so an `app.json` written by an earlier
    /// version keeps loading and simply reads as English.
    #[serde(default = "default_language")]
    pub language: String,
}

/// The language an installation uses before the user chooses one.
fn default_language() -> String {
    "en".to_string()
}

fn default_true() -> bool {
    true
}

/// Where the credential helper may live, in priority order.
///
/// The installer places it beside the application executable; the debug and
/// release target directories cover a development build.
fn helper_candidates() -> Vec<std::path::PathBuf> {
    const FILE_NAME: &str = "eclyzer-bridge-helper.exe";
    let mut candidates = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(FILE_NAME));
            // Tauri places bundled resources in a `resources` subdirectory
            // beside the executable on some layouts.
            candidates.push(dir.join("resources").join(FILE_NAME));
        }
    }
    candidates.push(std::path::PathBuf::from(FILE_NAME));
    candidates
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: crate::state::APP_SETTINGS_SCHEMA_VERSION,
            update_check: true,
            last_update_check: None,
            last_project_directory: None,
            remember_project_directory: true,
            log_level: LogLevel::default(),
            first_run_complete: false,
            cost_notice_seen: false,
            insecure_endpoint_notice_seen: false,
            language: default_language(),
        }
    }
}

impl AppState {
    /// Build state for the default application data directory.
    pub fn bootstrap() -> BridgeResult<Self> {
        let paths = AppPaths::resolve()?;
        paths.ensure_dirs()?;
        Self::with_paths(paths)
    }

    /// Build state for an explicit directory (tests, portable diagnostics).
    pub fn with_paths(paths: AppPaths) -> BridgeResult<Self> {
        paths.ensure_dirs()?;

        let settings = crate::state::load_settings(&paths)?;
        let level = settings.log_level;
        let log_guard = crate::diagnostics::init_logging(&paths.logs_dir(), level).ok();

        tracing::info!(
            target: "eclyzer_bridge",
            version = crate::APP_VERSION,
            data_dir = %paths.root().display(),
            "Eclyzer Bridge starting"
        );

        // Both adapters share one backup manager, so the Original/Snapshot/
        // History ledger is rooted in the same place for every agent.
        let backups = BackupManager::new(paths.backups_dir());

        Ok(Self {
            inner: Arc::new(AppStateInner {
                profiles: ProfileStore::new(paths.profiles_json()),
                secrets: SecretStore::new(paths.secrets_dat()),
                codex: CodexAdapter::new(backups.clone()),
                claude: ClaudeAdapter::new(backups),
                paths,
                settings: std::sync::RwLock::new(settings),
                _log_guard: std::sync::Mutex::new(log_guard),
            }),
        })
    }

    pub fn paths(&self) -> &AppPaths {
        &self.inner.paths
    }

    pub fn profiles(&self) -> &ProfileStore {
        &self.inner.profiles
    }

    pub fn secrets(&self) -> &SecretStore {
        &self.inner.secrets
    }

    pub fn codex(&self) -> &CodexAdapter {
        &self.inner.codex
    }

    pub fn claude(&self) -> &ClaudeAdapter {
        &self.inner.claude
    }

    /// Resolve an adapter by agent.
    pub fn adapter(&self, agent: AgentId) -> &dyn AgentAdapter {
        match agent {
            AgentId::Codex => &self.inner.codex,
            AgentId::Claude => &self.inner.claude,
        }
    }

    /// Every adapter, for the agent list.
    pub fn adapters(&self) -> Vec<&dyn AgentAdapter> {
        vec![&self.inner.codex, &self.inner.claude]
    }

    /// The backup ledger rooted at this install's data directory.
    pub fn backups(&self) -> BackupManager {
        BackupManager::new(self.inner.paths.backups_dir())
    }

    /// Path to the bundled credential helper, when it shipped with the app.
    ///
    /// A missing helper is not an error: Codex falls back to an environment
    /// credential and clearly says so.
    pub fn helper_path(&self) -> Option<std::path::PathBuf> {
        helper_candidates().into_iter().find(|path| path.exists())
    }

    /// Snapshot the current settings.
    pub fn settings(&self) -> AppSettings {
        self.inner
            .settings
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Replace settings and persist them.
    pub fn update_settings<F>(&self, mutate: F) -> BridgeResult<AppSettings>
    where
        F: FnOnce(&mut AppSettings),
    {
        let updated = {
            let mut guard = self.inner.settings.write().map_err(|_| {
                crate::error::BridgeError::internal("Application settings are unavailable.")
            })?;
            mutate(&mut guard);
            guard.clone()
        };
        crate::state::save_settings(&self.inner.paths, &updated)?;
        Ok(updated)
    }
}
