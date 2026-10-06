//! Snapshot store with bounded retention (PRD §26).

use std::path::{Path, PathBuf};

use crate::backup::SnapshotMeta;
use crate::config::fs::{read_to_string_opt, sha256_hex, write_atomic};
use crate::error::{BridgeError, BridgeResult};

/// How many snapshots are retained per agent (PRD §26.4).
pub const SNAPSHOT_RETENTION: usize = 20;

/// A snapshot returned to callers: metadata plus the captured content.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub meta: SnapshotMeta,
    pub content: String,
}

/// Stores snapshots under `<backups>/<agent>/snapshots/`.
#[derive(Clone)]
pub struct SnapshotStore {
    root: PathBuf,
}

impl SnapshotStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn snapshot_dir(&self, agent: &str) -> PathBuf {
        self.root.join(agent).join("snapshots")
    }

    /// Capture `config_path` as a new snapshot, then prune old ones.
    pub fn create(
        &self,
        agent: &str,
        config_path: &Path,
        reason: &str,
        profile_id: Option<&str>,
        managed_keys: &[String],
    ) -> BridgeResult<Snapshot> {
        let content = read_to_string_opt(config_path)?.unwrap_or_default();

        let id = uuid::Uuid::new_v4().to_string();
        let created_at = chrono::Local::now().to_rfc3339();
        let sha256 = sha256_hex(content.as_bytes());
        let file_name = format!("{id}.content");

        let dir = self.snapshot_dir(agent);
        std::fs::create_dir_all(&dir)?;

        let meta = SnapshotMeta {
            id: id.clone(),
            agent: agent.to_string(),
            created_at,
            reason: reason.to_string(),
            profile_id: profile_id.map(str::to_string),
            sha256,
            file_name: file_name.clone(),
            bytes: content.len() as u64,
            managed_keys: managed_keys.to_vec(),
        };

        // Payload first, then metadata: a crash between the two leaves an
        // orphan payload, which is harmless, rather than metadata pointing at
        // nothing, which would break restore.
        write_atomic(&dir.join(&file_name), content.as_bytes())?;
        let meta_json = serde_json::to_string_pretty(&meta).map_err(|err| {
            BridgeError::internal("Could not record the snapshot.").with_details(err.to_string())
        })?;
        write_atomic(&dir.join(format!("{id}.json")), meta_json.as_bytes())?;

        self.prune(agent)?;

        tracing::info!(
            target: "eclyzer_bridge",
            agent,
            snapshot = %id,
            reason,
            "snapshot created"
        );

        Ok(Snapshot { meta, content })
    }

    /// All snapshots for `agent`, newest first.
    pub fn list(&self, agent: &str) -> BridgeResult<Vec<SnapshotMeta>> {
        let dir = self.snapshot_dir(agent);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(Vec::new());
        };

        let mut metas = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(raw) = read_to_string_opt(&path)? else {
                continue;
            };
            match serde_json::from_str::<SnapshotMeta>(&raw) {
                Ok(meta) => metas.push(meta),
                Err(err) => {
                    // One unreadable snapshot must not hide the rest.
                    tracing::warn!(
                        target: "eclyzer_bridge",
                        path = %path.display(),
                        error = %err,
                        "skipping unreadable snapshot metadata"
                    );
                }
            }
        }

        metas.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(metas)
    }

    /// Load a snapshot by id.
    pub fn get(&self, agent: &str, id: &str) -> BridgeResult<Option<Snapshot>> {
        let dir = self.snapshot_dir(agent);
        let meta_path = dir.join(format!("{id}.json"));
        let Some(raw) = read_to_string_opt(&meta_path)? else {
            return Ok(None);
        };
        let meta: SnapshotMeta = serde_json::from_str(&raw).map_err(|err| {
            BridgeError::internal("This snapshot's metadata is unreadable.")
                .with_details(err.to_string())
        })?;
        let content = read_to_string_opt(&dir.join(&meta.file_name))?.unwrap_or_default();
        Ok(Some(Snapshot { meta, content }))
    }

    /// The most recent snapshot for `agent`.
    pub fn latest(&self, agent: &str) -> BridgeResult<Option<Snapshot>> {
        let Some(newest) = self.list(agent)?.into_iter().next() else {
            return Ok(None);
        };
        self.get(agent, &newest.id)
    }

    /// Prune to the newest [`SNAPSHOT_RETENTION`] entries.
    pub fn prune(&self, agent: &str) -> BridgeResult<usize> {
        let metas = self.list(agent)?;
        if metas.len() <= SNAPSHOT_RETENTION {
            return Ok(0);
        }

        let dir = self.snapshot_dir(agent);
        let mut removed = 0;
        for meta in metas.into_iter().skip(SNAPSHOT_RETENTION) {
            let _ = std::fs::remove_file(dir.join(format!("{}.json", meta.id)));
            let _ = std::fs::remove_file(dir.join(&meta.file_name));
            removed += 1;
        }

        if removed > 0 {
            tracing::info!(target: "eclyzer_bridge", agent, removed, "pruned old snapshots");
        }
        Ok(removed)
    }

    /// Delete every snapshot for `agent` (used only by explicit user action).
    pub fn clear(&self, agent: &str) -> BridgeResult<usize> {
        let dir = self.snapshot_dir(agent);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(0);
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            if entry.path().is_file() && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, SnapshotStore, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().join("backups"));
        let config = dir.path().join("config.toml");
        (dir, store, config)
    }

    #[test]
    fn create_and_read_back() {
        let (_dir, store, config) = setup();
        std::fs::write(&config, "model = \"a\"\n").unwrap();
        let snap = store
            .create(
                "codex",
                &config,
                "activate_profile",
                Some("p1"),
                &["model".into()],
            )
            .unwrap();

        let loaded = store.get("codex", &snap.meta.id).unwrap().unwrap();
        assert_eq!(loaded.content, "model = \"a\"\n");
        assert_eq!(loaded.meta.reason, "activate_profile");
        assert_eq!(loaded.meta.profile_id.as_deref(), Some("p1"));
        assert_eq!(loaded.meta.sha256, sha256_hex(b"model = \"a\"\n"));
    }

    #[test]
    fn list_is_newest_first() {
        let (_dir, store, config) = setup();
        for i in 0..3 {
            std::fs::write(&config, format!("v = {i}\n")).unwrap();
            store
                .create("codex", &config, "activate_profile", None, &[])
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let list = store.list("codex").unwrap();
        assert_eq!(list.len(), 3);
        assert!(list[0].created_at >= list[1].created_at);
    }

    #[test]
    fn retention_keeps_only_twenty() {
        let (_dir, store, config) = setup();
        for i in 0..25 {
            std::fs::write(&config, format!("v = {i}\n")).unwrap();
            store
                .create("codex", &config, "activate_profile", None, &[])
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let list = store.list("codex").unwrap();
        assert_eq!(list.len(), SNAPSHOT_RETENTION, "retention must cap at 20");

        // The newest snapshot must be the last one written.
        let newest = store.latest("codex").unwrap().unwrap();
        assert_eq!(newest.content, "v = 24\n");
    }

    #[test]
    fn pruning_removes_payload_files_too() {
        let (_dir, store, config) = setup();
        for i in 0..24 {
            std::fs::write(&config, format!("v = {i}\n")).unwrap();
            store
                .create("codex", &config, "activate_profile", None, &[])
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let dir = store.snapshot_dir("codex");
        let payloads = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("content"))
            .count();
        assert_eq!(
            payloads, SNAPSHOT_RETENTION,
            "orphan payloads must be pruned"
        );
    }

    #[test]
    fn snapshots_are_isolated_per_agent() {
        let (_dir, store, config) = setup();
        std::fs::write(&config, "codex\n").unwrap();
        store
            .create("codex", &config, "activate_profile", None, &[])
            .unwrap();
        assert_eq!(store.list("claude").unwrap().len(), 0);
    }

    #[test]
    fn missing_snapshot_returns_none() {
        let (_dir, store, _config) = setup();
        assert!(store.get("codex", "does-not-exist").unwrap().is_none());
    }

    #[test]
    fn missing_config_snapshots_empty_content() {
        let (_dir, store, config) = setup();
        let snap = store
            .create("codex", &config, "activate_profile", None, &[])
            .unwrap();
        assert_eq!(snap.content, "");
        assert_eq!(snap.meta.bytes, 0);
    }

    #[test]
    fn corrupt_metadata_does_not_hide_valid_snapshots() {
        let (_dir, store, config) = setup();
        std::fs::write(&config, "good\n").unwrap();
        let snap = store
            .create("codex", &config, "activate_profile", None, &[])
            .unwrap();
        std::fs::write(store.snapshot_dir("codex").join("broken.json"), b"{oops").unwrap();

        let list = store.list("codex").unwrap();
        assert_eq!(list.len(), 1, "the valid snapshot must still be listed");
        assert_eq!(list[0].id, snap.meta.id);
    }

    #[test]
    fn clear_removes_everything() {
        let (_dir, store, config) = setup();
        std::fs::write(&config, "x\n").unwrap();
        store.create("codex", &config, "r", None, &[]).unwrap();
        assert!(store.clear("codex").unwrap() > 0);
        assert_eq!(store.list("codex").unwrap().len(), 0);
    }
}
