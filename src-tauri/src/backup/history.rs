//! Configuration history (PRD §27).
//!
//! A local, append-only ledger of mutations. It records what changed and which
//! snapshot can undo it. It never records a secret, and it is capped so the file
//! cannot grow without bound.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::fs::{read_to_string_opt, write_atomic};
use crate::error::{BridgeError, BridgeResult};

/// Maximum history entries retained per agent.
pub const HISTORY_LIMIT: usize = 200;

/// One recorded configuration change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub agent: String,
    pub created_at: String,
    /// Human sentence, e.g. "Activated DeepSeek".
    pub label: String,
    /// Machine code, e.g. `activate_profile`.
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    /// Snapshot to restore in order to undo this change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    /// Whether the operation succeeded. Failed attempts are recorded too so a
    /// user can see why nothing changed.
    #[serde(default)]
    pub success: bool,
    /// Redacted summary of any error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Persistent history ledger.
pub struct HistoryStore {
    path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: Vec<HistoryEntry>,
}

impl HistoryStore {
    /// Create a store rooted at the application backup directory.
    pub fn new(backups_root: impl Into<PathBuf>) -> Self {
        Self {
            path: backups_root.into().join("history.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> BridgeResult<HistoryFile> {
        let Some(raw) = read_to_string_opt(&self.path)? else {
            return Ok(HistoryFile {
                version: 1,
                entries: Vec::new(),
            });
        };
        if raw.trim().is_empty() {
            return Ok(HistoryFile {
                version: 1,
                entries: Vec::new(),
            });
        }
        serde_json::from_str(&raw)
            .or_else(|err| {
                // History is diagnostic data: losing it must never block a restore.
                tracing::warn!(
                    target: "eclink",
                    error = %err,
                    "history.json is unreadable; starting a fresh ledger"
                );
                Ok::<HistoryFile, serde_json::Error>(HistoryFile {
                    version: 1,
                    entries: Vec::new(),
                })
            })
            .map_err(BridgeError::from)
    }

    fn save(&self, file: &HistoryFile) -> BridgeResult<()> {
        let json = serde_json::to_string_pretty(file).map_err(|err| {
            BridgeError::internal("Could not save the configuration history.")
                .with_details(err.to_string())
        })?;
        // The backups root may not exist yet on a fresh install.
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_atomic(&self.path, json.as_bytes())
    }

    /// Append an entry, pruning the oldest beyond [`HISTORY_LIMIT`].
    pub fn record(&self, mut entry: HistoryEntry) -> BridgeResult<HistoryEntry> {
        if entry.id.is_empty() {
            entry.id = uuid::Uuid::new_v4().to_string();
        }
        if entry.created_at.is_empty() {
            entry.created_at = chrono::Local::now().to_rfc3339();
        }
        // Defense in depth: an error string stored here is redacted again.
        if let Some(error) = &entry.error {
            entry.error = Some(crate::diagnostics::redact::redact_text(error));
        }

        let mut file = self.load()?;
        file.entries.push(entry.clone());
        if file.entries.len() > HISTORY_LIMIT {
            let excess = file.entries.len() - HISTORY_LIMIT;
            file.entries.drain(0..excess);
        }
        file.version = 1;
        self.save(&file)?;
        Ok(entry)
    }

    /// Record a successful mutation.
    pub fn record_success(
        &self,
        agent: &str,
        reason: &crate::adapters::MutationReason,
        snapshot_id: Option<&str>,
    ) -> BridgeResult<HistoryEntry> {
        self.record(HistoryEntry {
            id: String::new(),
            agent: agent.to_string(),
            created_at: String::new(),
            label: reason.label(),
            reason: reason.code().to_string(),
            profile_id: reason.profile_id().map(str::to_string),
            profile_name: match reason {
                crate::adapters::MutationReason::ActivateProfile { profile_name, .. } => {
                    Some(profile_name.clone())
                }
                _ => None,
            },
            snapshot_id: snapshot_id.map(str::to_string),
            success: true,
            error: None,
        })
    }

    /// Record a failed mutation attempt.
    pub fn record_failure(
        &self,
        agent: &str,
        reason: &crate::adapters::MutationReason,
        error: &str,
    ) -> BridgeResult<HistoryEntry> {
        self.record(HistoryEntry {
            id: String::new(),
            agent: agent.to_string(),
            created_at: String::new(),
            label: reason.label(),
            reason: reason.code().to_string(),
            profile_id: reason.profile_id().map(str::to_string),
            profile_name: None,
            snapshot_id: None,
            success: false,
            error: Some(error.to_string()),
        })
    }

    /// All entries, newest first. `agent` filters to one agent when supplied.
    pub fn list(&self, agent: Option<&str>) -> BridgeResult<Vec<HistoryEntry>> {
        let mut entries = self.load()?.entries;
        if let Some(agent) = agent {
            entries.retain(|entry| entry.agent == agent);
        }
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(entries)
    }

    /// The most recent *successful* mutation for `agent`, used by Undo
    /// (PRD §28).
    pub fn latest_successful(&self, agent: &str) -> BridgeResult<Option<HistoryEntry>> {
        let mut entries: Vec<HistoryEntry> = self
            .load()?
            .entries
            .into_iter()
            .filter(|entry| entry.agent == agent && entry.success && entry.snapshot_id.is_some())
            .collect();
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(entries.into_iter().next())
    }

    /// Clear the ledger for one agent, or everything when `agent` is `None`.
    pub fn clear(&self, agent: Option<&str>) -> BridgeResult<usize> {
        let mut file = self.load()?;
        let before = file.entries.len();
        match agent {
            Some(agent) => file.entries.retain(|entry| entry.agent != agent),
            None => file.entries.clear(),
        }
        let removed = before - file.entries.len();
        self.save(&file)?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::MutationReason;

    fn store() -> (tempfile::TempDir, HistoryStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(dir.path().join("backups"));
        (dir, store)
    }

    fn reason(name: &str) -> MutationReason {
        MutationReason::ActivateProfile {
            profile_id: format!("id-{name}"),
            profile_name: name.to_string(),
        }
    }

    #[test]
    fn records_and_lists_newest_first() {
        let (_dir, store) = store();
        store
            .record_success("codex", &reason("DeepSeek"), Some("s1"))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        store
            .record_success("codex", &reason("Gateway"), Some("s2"))
            .unwrap();

        let list = store.list(Some("codex")).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].label, "Activated Gateway");
        assert_eq!(list[1].label, "Activated DeepSeek");
    }

    #[test]
    fn filters_by_agent() {
        let (_dir, store) = store();
        store
            .record_success("codex", &reason("A"), Some("s1"))
            .unwrap();
        store
            .record_success("claude", &reason("B"), Some("s2"))
            .unwrap();
        assert_eq!(store.list(Some("codex")).unwrap().len(), 1);
        assert_eq!(store.list(Some("claude")).unwrap().len(), 1);
        assert_eq!(store.list(None).unwrap().len(), 2);
    }

    #[test]
    fn failed_attempts_are_recorded_but_not_undo_targets() {
        let (_dir, store) = store();
        store
            .record_success("codex", &reason("Good"), Some("s1"))
            .unwrap();
        store
            .record_failure("codex", &MutationReason::RestoreDefault, "boom")
            .unwrap();

        let list = store.list(Some("codex")).unwrap();
        assert_eq!(list.len(), 2, "failures are visible in history");

        let undo_target = store.latest_successful("codex").unwrap().unwrap();
        assert_eq!(undo_target.label, "Activated Good");
        assert_eq!(undo_target.snapshot_id.as_deref(), Some("s1"));
    }

    #[test]
    fn latest_successful_ignores_entries_without_snapshot() {
        let (_dir, store) = store();
        store
            .record(HistoryEntry {
                id: String::new(),
                agent: "codex".into(),
                created_at: String::new(),
                label: "No snapshot".into(),
                reason: "activate_profile".into(),
                profile_id: None,
                profile_name: None,
                snapshot_id: None,
                success: true,
                error: None,
            })
            .unwrap();
        assert!(store.latest_successful("codex").unwrap().is_none());
    }

    #[test]
    fn retention_caps_entries() {
        let (_dir, store) = store();
        for i in 0..(HISTORY_LIMIT + 25) {
            store
                .record_success("codex", &reason(&format!("P{i}")), Some("s"))
                .unwrap();
        }
        let list = store.list(Some("codex")).unwrap();
        assert_eq!(list.len(), HISTORY_LIMIT);
        // The newest entry survives the prune.
        assert_eq!(list[0].profile_name.as_deref(), Some("P224"));
    }

    #[test]
    fn stored_errors_are_redacted() {
        let (_dir, store) = store();
        store
            .record_failure(
                "codex",
                &MutationReason::RestoreDefault,
                "request failed with Authorization: Bearer sk-abcdef0123456789abcdef",
            )
            .unwrap();
        let raw = std::fs::read_to_string(store.path()).unwrap();
        assert!(
            !raw.contains("sk-abcdef0123456789abcdef"),
            "history leaked a secret: {raw}"
        );
        assert!(raw.contains("[REDACTED]"));
    }

    #[test]
    fn history_file_never_stores_prompts_or_keys() {
        let (_dir, store) = store();
        store
            .record_success("codex", &reason("P"), Some("s"))
            .unwrap();
        let raw = std::fs::read_to_string(store.path()).unwrap();
        for forbidden in ["sk-", "Bearer", "prompt", "apiKey"] {
            assert!(
                !raw.contains(forbidden),
                "history contains {forbidden}: {raw}"
            );
        }
    }

    #[test]
    fn corrupt_history_does_not_block_operation() {
        let (_dir, store) = store();
        // Create the parent so the corrupt file can be planted; the store must
        // then recover rather than propagate a parse error.
        std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        std::fs::write(store.path(), b"not json").unwrap();
        // Must not error: history is a convenience, not a source of truth.
        assert!(store.list(Some("codex")).unwrap().is_empty());
        assert!(store
            .record_success("codex", &reason("X"), Some("s"))
            .is_ok());
        // The ledger recovers and is readable afterwards.
        assert_eq!(store.list(Some("codex")).unwrap().len(), 1);
    }

    #[test]
    fn writing_into_a_missing_directory_creates_it() {
        // A fresh install has no `backups/` folder yet; recording must still work.
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(dir.path().join("backups"));
        assert!(store
            .record_success("codex", &reason("X"), Some("s"))
            .is_ok());
        assert!(store.path().exists());
    }

    #[test]
    fn clear_scopes_to_agent() {
        let (_dir, store) = store();
        store
            .record_success("codex", &reason("A"), Some("s1"))
            .unwrap();
        store
            .record_success("claude", &reason("B"), Some("s2"))
            .unwrap();
        assert_eq!(store.clear(Some("codex")).unwrap(), 1);
        assert_eq!(store.list(Some("codex")).unwrap().len(), 0);
        assert_eq!(store.list(Some("claude")).unwrap().len(), 1);
    }

    #[test]
    fn every_restore_reason_has_a_label() {
        for reason in [
            MutationReason::RestoreDefault,
            MutationReason::RestoreOriginal,
            MutationReason::RestoreSnapshot {
                snapshot_id: "s".into(),
            },
            MutationReason::UndoLastChange,
        ] {
            assert!(!reason.label().is_empty());
        }
    }
}
