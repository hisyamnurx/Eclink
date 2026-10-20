//! Backup, snapshot, and history management (PRD §25–§29).
//!
//! Three distinct artifacts:
//!
//! * **Original** — captured once, before Bridge's first mutation of an agent.
//!   Immutable from the UI and never auto-deleted.
//! * **Snapshot** — captured before *every* mutation, retained 20 per agent.
//! * **History** — an append-only local ledger of what happened, with a pointer
//!   to the snapshot that can undo it.
//!
//! Backups store the agent config file, with any credential *values* blanked
//! out first.
//!
//! It is not true that keys never live in these config files: Codex uses the
//! credential helper, but Claude Code has no such mechanism, so its gateway key
//! is written into `settings.json` by design. Because the backup system copies
//! config files verbatim, that key would otherwise be duplicated into the
//! snapshot tree and the original backup on every mutation. Both writers
//! therefore run the payload through
//! [`crate::diagnostics::redact::strip_credential_values`], which empties the
//! value of a known credential key while leaving the file valid and restorable.

pub mod history;
pub mod snapshots;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::fs::{read_to_string_opt, sha256_hex, write_atomic};
use crate::error::{BridgeError, BridgeResult};

pub use history::{HistoryEntry, HistoryStore, HISTORY_LIMIT};
pub use snapshots::{Snapshot, SnapshotStore, SNAPSHOT_RETENTION};

/// Metadata recorded alongside each snapshot (PRD §26.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMeta {
    pub id: String,
    pub agent: String,
    pub created_at: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// SHA-256 of the backed-up content. Never a secret.
    pub sha256: String,
    /// File name inside the snapshot directory.
    pub file_name: String,
    /// Size in bytes, for diagnostics.
    pub bytes: u64,
    /// Managed keys as understood at snapshot time, so a restore can rebuild
    /// ownership metadata (PRD §49).
    #[serde(default)]
    pub managed_keys: Vec<String>,
}

/// Manages originals and snapshots for every agent.
#[derive(Clone)]
pub struct BackupManager {
    root: PathBuf,
    snapshots: SnapshotStore,
}

impl BackupManager {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            snapshots: SnapshotStore::new(root.clone()),
            root,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn snapshots(&self) -> &SnapshotStore {
        &self.snapshots
    }

    /// The change ledger, rooted alongside the snapshots it points at.
    pub fn history(&self) -> HistoryStore {
        HistoryStore::new(self.root.clone())
    }

    fn agent_dir(&self, agent: &str) -> PathBuf {
        self.root.join(agent)
    }

    fn original_path(&self, agent: &str) -> PathBuf {
        self.agent_dir(agent).join("original")
    }

    /// Whether an immutable original backup exists for `agent`.
    pub fn has_original(&self, agent: &str) -> bool {
        self.original_path(agent).join("meta.json").exists()
    }

    /// Create the original backup if it does not exist yet.
    ///
    /// Returns `true` when a new original was written. Existing originals are
    /// never overwritten, which is what makes "restore before Bridge ever
    /// touched this agent" meaningful (PRD §26.1).
    pub fn ensure_original(
        &self,
        agent: &str,
        config_path: &Path,
        managed_keys: &[String],
    ) -> BridgeResult<bool> {
        if self.has_original(agent) {
            return Ok(false);
        }

        let raw = read_to_string_opt(config_path)?;
        let (content, sha256, bytes, existed) = match raw {
            Some(text) => {
                // A user who already configures a gateway has the credential in
                // the config file *before* Bridge's first run, so the original
                // backup would otherwise capture it. Blank credential values for
                // the same reason as in the snapshot store: the backup must
                // never hold a second copy of a key.
                let stored = crate::diagnostics::redact::strip_credential_values(&text);
                // The hash and size still describe the file as it really was, so
                // the backup metadata stays an accurate record of the original.
                let bytes = text.len() as u64;
                let hash = sha256_hex(text.as_bytes());
                (stored, hash, bytes, true)
            }
            // The agent config did not exist yet. Record that fact explicitly so
            // restore-original can remove a file Bridge created rather than
            // writing an empty one.
            None => (String::new(), sha256_hex(b""), 0, false),
        };

        let dir = self.original_path(agent);
        std::fs::create_dir_all(&dir)?;

        let meta = OriginalMeta {
            agent: agent.to_string(),
            created_at: chrono::Local::now().to_rfc3339(),
            sha256,
            bytes,
            existed,
            source_path: config_path.display().to_string(),
            managed_keys: managed_keys.to_vec(),
        };

        // Write content first, then meta, so a crash cannot leave a meta file
        // pointing at a missing payload.
        write_atomic(&dir.join("content"), content.as_bytes())?;
        let meta_json = serde_json::to_string_pretty(&meta).map_err(|err| {
            BridgeError::internal("Could not record the original backup.")
                .with_details(err.to_string())
        })?;
        write_atomic(&dir.join("meta.json"), meta_json.as_bytes())?;

        tracing::info!(target: "eclink", agent, "original backup created");
        Ok(true)
    }

    /// Read the original backup payload and metadata.
    pub fn read_original(&self, agent: &str) -> BridgeResult<Option<(OriginalMeta, String)>> {
        let dir = self.original_path(agent);
        let Some(meta_raw) = read_to_string_opt(&dir.join("meta.json"))? else {
            return Ok(None);
        };
        let meta: OriginalMeta = serde_json::from_str(&meta_raw).map_err(|err| {
            BridgeError::internal("The original backup metadata is unreadable.")
                .with_details(err.to_string())
        })?;
        let content = read_to_string_opt(&dir.join("content"))?.unwrap_or_default();
        Ok(Some((meta, content)))
    }

    /// Take a snapshot of `config_path` before a mutation.
    pub fn create_snapshot(
        &self,
        agent: &str,
        config_path: &Path,
        reason: &str,
        profile_id: Option<&str>,
        managed_keys: &[String],
    ) -> BridgeResult<Snapshot> {
        self.snapshots
            .create(agent, config_path, reason, profile_id, managed_keys)
    }

    /// Total number of snapshots held for `agent`.
    pub fn snapshot_count(&self, agent: &str) -> usize {
        self.snapshots
            .list(agent)
            .map(|list| list.len())
            .unwrap_or(0)
    }

    /// Restore the raw original configuration atomically (PRD §29).
    ///
    /// The caller is responsible for having taken a snapshot of the current
    /// state first; this method only writes.
    pub fn write_original(&self, agent: &str, config_path: &Path) -> BridgeResult<()> {
        let Some((meta, content)) = self.read_original(agent)? else {
            return Err(BridgeError::internal(
                "There is no original backup to restore for this agent.",
            )
            .with_hint("Eclink has not modified this agent yet."));
        };

        if meta.existed {
            write_atomic(config_path, content.as_bytes())?;
        } else if config_path.exists() {
            // Bridge created this file; restoring the original state means
            // removing it.
            std::fs::remove_file(config_path).map_err(|err| {
                BridgeError::io("Could not remove the configuration file Eclink created.")
                    .with_details(err.to_string())
            })?;
        }
        Ok(())
    }
}

/// Metadata for the immutable original backup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginalMeta {
    pub agent: String,
    pub created_at: String,
    pub sha256: String,
    pub bytes: u64,
    /// Whether the config file existed when the original was captured.
    pub existed: bool,
    pub source_path: String,
    #[serde(default)]
    pub managed_keys: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, BackupManager, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let manager = BackupManager::new(dir.path().join("backups"));
        let config = dir.path().join("config.toml");
        (dir, manager, config)
    }

    #[test]
    fn original_is_created_once_and_never_overwritten() {
        let (_dir, manager, config) = setup();
        std::fs::write(&config, "model = \"original\"\n").unwrap();

        assert!(manager.ensure_original("codex", &config, &[]).unwrap());
        // Change the file, then try again: the original must be untouched.
        std::fs::write(&config, "model = \"changed\"\n").unwrap();
        assert!(!manager.ensure_original("codex", &config, &[]).unwrap());

        let (_meta, content) = manager.read_original("codex").unwrap().unwrap();
        assert_eq!(content, "model = \"original\"\n");
    }

    #[test]
    fn original_records_a_missing_file_as_not_existing() {
        let (_dir, manager, config) = setup();
        assert!(manager.ensure_original("codex", &config, &[]).unwrap());
        let (meta, content) = manager.read_original("codex").unwrap().unwrap();
        assert!(!meta.existed);
        assert!(content.is_empty());
    }

    #[test]
    fn write_original_removes_file_bridge_created() {
        let (_dir, manager, config) = setup();
        // Original state: no file.
        manager.ensure_original("codex", &config, &[]).unwrap();
        // Bridge then creates it.
        std::fs::write(&config, "model = \"managed\"\n").unwrap();

        manager.write_original("codex", &config).unwrap();
        assert!(
            !config.exists(),
            "restoring a non-existent original removes the file"
        );
    }

    #[test]
    fn write_original_restores_content_when_it_existed() {
        let (_dir, manager, config) = setup();
        std::fs::write(&config, "model = \"original\"\n").unwrap();
        manager.ensure_original("codex", &config, &[]).unwrap();
        std::fs::write(&config, "model = \"managed\"\n").unwrap();

        manager.write_original("codex", &config).unwrap();
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "model = \"original\"\n"
        );
    }

    #[test]
    fn write_original_without_backup_is_actionable() {
        let (_dir, manager, config) = setup();
        let err = manager.write_original("codex", &config).unwrap_err();
        assert!(err.hint.is_some());
        assert!(err.message.contains("original backup"));
    }

    #[test]
    fn original_metadata_records_hash() {
        let (_dir, manager, config) = setup();
        std::fs::write(&config, "a = 1\n").unwrap();
        manager.ensure_original("codex", &config, &[]).unwrap();
        let (meta, _) = manager.read_original("codex").unwrap().unwrap();
        assert_eq!(meta.sha256, sha256_hex(b"a = 1\n"));
        assert_eq!(meta.bytes, 6);
    }

    #[test]
    fn original_backups_are_per_agent() {
        let (_dir, manager, codex_cfg) = setup();
        let claude_cfg = codex_cfg.with_file_name("settings.json");
        std::fs::write(&codex_cfg, "codex-original\n").unwrap();
        std::fs::write(&claude_cfg, "claude-original\n").unwrap();

        manager.ensure_original("codex", &codex_cfg, &[]).unwrap();
        manager.ensure_original("claude", &claude_cfg, &[]).unwrap();

        assert_eq!(
            manager.read_original("codex").unwrap().unwrap().1,
            "codex-original\n"
        );
        assert_eq!(
            manager.read_original("claude").unwrap().unwrap().1,
            "claude-original\n"
        );
    }

    #[test]
    fn backup_payload_never_contains_a_secret() {
        // The config files Bridge backs up never hold keys; assert the manager
        // does not add any of its own.
        let (_dir, manager, config) = setup();
        std::fs::write(&config, "model = \"m\"\n").unwrap();
        manager
            .ensure_original("codex", &config, &["model".into()])
            .unwrap();

        let dir = manager.original_path("codex");
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            for forbidden in ["sk-", "Authorization", "Bearer", "api_key"] {
                assert!(
                    !text.contains(forbidden),
                    "backup {} contains {forbidden}",
                    entry.path().display()
                );
            }
        }
    }
}
