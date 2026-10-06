//! DPAPI-sealed secret storage (PRD §13, §44.3).
//!
//! `secrets.dat` layout:
//!
//! ```json
//! {
//!   "version": 1,
//!   "records": {
//!     "<secret-id>": { "createdAt": "...", "updatedAt": "...", "blob": "base64…" }
//!   }
//! }
//! ```
//!
//! Each `blob` is the DPAPI output for that one record, so rotating one key
//! rewrites only that record and a corrupt record cannot take down the file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::config::fs::{harden_permissions, read_to_string_opt, write_atomic};
use crate::error::{BridgeError, BridgeResult};
use crate::secrets::crypto::{Cipher, DpapiCipher};

/// Schema version for `secrets.dat`.
pub const SECRETS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretRecord {
    pub created_at: String,
    pub updated_at: String,
    /// Base64 of the DPAPI-sealed bytes.
    pub blob: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStoreData {
    pub version: u32,
    #[serde(default)]
    pub records: BTreeMap<String, SecretRecord>,
}

impl Default for SecretStoreData {
    fn default() -> Self {
        Self {
            version: SECRETS_SCHEMA_VERSION,
            records: BTreeMap::new(),
        }
    }
}

/// Encrypted secret store backed by a [`Cipher`].
pub struct SecretStore {
    path: PathBuf,
    cipher: Box<dyn Cipher>,
}

impl SecretStore {
    /// Production constructor: DPAPI Current User scope.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            cipher: Box::new(DpapiCipher::new()),
        }
    }

    /// Test constructor with an injectable cipher.
    pub fn with_cipher(path: impl Into<PathBuf>, cipher: Box<dyn Cipher>) -> Self {
        Self {
            path: path.into(),
            cipher,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cipher_name(&self) -> &'static str {
        self.cipher.name()
    }

    fn load_raw(&self) -> BridgeResult<SecretStoreData> {
        let Some(raw) = read_to_string_opt(&self.path)? else {
            return Ok(SecretStoreData::default());
        };
        if raw.trim().is_empty() {
            return Ok(SecretStoreData::default());
        }
        serde_json::from_str(&raw).map_err(|err| {
            BridgeError::new(
                crate::error::ErrorKind::Secret,
                "The stored API keys could not be read.",
            )
            .with_hint(
                "The secret file may be damaged. Deleting secrets.dat removes saved keys so you can re-enter them.",
            )
            .with_details(err.to_string())
        })
    }

    fn save_raw(&self, data: &SecretStoreData) -> BridgeResult<()> {
        let json = serde_json::to_string(data).map_err(|err| {
            BridgeError::internal("Could not serialise the secret store.")
                .with_details(err.to_string())
        })?;
        write_atomic(&self.path, json.as_bytes())?;
        let _ = harden_permissions(&self.path);
        Ok(())
    }

    /// Create or replace the secret at `id`.
    pub fn put(&self, id: &str, secret: &str) -> BridgeResult<()> {
        if id.trim().is_empty() {
            return Err(BridgeError::validation("The secret reference is missing."));
        }
        if secret.is_empty() {
            return Err(BridgeError::validation("The API key is empty."));
        }

        let sealed = self.cipher.seal(secret.as_bytes())?;
        let blob = base64::engine::general_purpose::STANDARD.encode(sealed);

        let mut data = self.load_raw()?;
        let now = chrono::Local::now().to_rfc3339();
        let created_at = data
            .records
            .get(id)
            .map(|record| record.created_at.clone())
            .unwrap_or_else(|| now.clone());

        data.records.insert(
            id.to_string(),
            SecretRecord {
                created_at,
                updated_at: now,
                blob,
            },
        );
        data.version = SECRETS_SCHEMA_VERSION;
        self.save_raw(&data)
    }

    /// Resolve a secret. Only called by code that genuinely needs the value.
    pub fn get(&self, id: &str) -> BridgeResult<Option<String>> {
        let data = self.load_raw()?;
        let Some(record) = data.records.get(id) else {
            return Ok(None);
        };
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(record.blob.as_bytes())
            .map_err(|err| {
                BridgeError::new(
                    crate::error::ErrorKind::Secret,
                    "The stored API key is damaged.",
                )
                .with_hint("Re-enter the API key for this profile.")
                .with_details(err.to_string())
            })?;
        let plaintext = self.cipher.open(&sealed)?;
        let value = String::from_utf8(plaintext).map_err(|_| {
            BridgeError::new(
                crate::error::ErrorKind::Secret,
                "The stored API key is not valid text.",
            )
            .with_hint("Re-enter the API key for this profile.")
        })?;
        Ok(Some(value))
    }

    /// Whether a record exists, without decrypting it.
    pub fn exists(&self, id: &str) -> BridgeResult<bool> {
        Ok(self.load_raw()?.records.contains_key(id))
    }

    /// A masked fingerprint for display, e.g. `****abcd`.
    ///
    /// This is the only representation of an existing key that ever reaches the
    /// frontend (PRD §14, §51).
    pub fn fingerprint(&self, id: &str) -> BridgeResult<Option<String>> {
        Ok(self.get(id)?.map(|secret| crate::diagnostics::redact::fingerprint(&secret)))
    }

    /// Delete a secret. Missing records are not an error.
    pub fn delete(&self, id: &str) -> BridgeResult<bool> {
        let mut data = self.load_raw()?;
        let removed = data.records.remove(id).is_some();
        if removed {
            self.save_raw(&data)?;
        }
        Ok(removed)
    }

    /// Number of stored secrets, for diagnostics.
    pub fn count(&self) -> BridgeResult<usize> {
        Ok(self.load_raw()?.records.len())
    }

    /// Secret IDs currently present, for orphan detection.
    pub fn ids(&self) -> BridgeResult<Vec<String>> {
        Ok(self.load_raw()?.records.keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::crypto::PlaintextCipher;

    fn store() -> (tempfile::TempDir, SecretStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::with_cipher(
            dir.path().join("secrets.dat"),
            Box::new(PlaintextCipher::new()),
        );
        (dir, store)
    }

    #[test]
    fn put_and_get_round_trips() {
        let (_dir, store) = store();
        store.put("s1", "sk-abcdef0123456789").unwrap();
        assert_eq!(store.get("s1").unwrap().unwrap(), "sk-abcdef0123456789");
    }

    #[test]
    fn secret_is_not_plaintext_on_disk() {
        let (_dir, store) = store();
        let secret = "sk-plaintext-must-not-appear-9876543210";
        store.put("s1", secret).unwrap();

        let raw = std::fs::read_to_string(store.path()).unwrap();
        assert!(
            !raw.contains(secret),
            "secrets.dat must never contain the raw key: {raw}"
        );
        // The record id is fine to store.
        assert!(raw.contains("s1"));
    }

    #[test]
    fn missing_secret_returns_none() {
        let (_dir, store) = store();
        assert!(store.get("nope").unwrap().is_none());
        assert!(!store.exists("nope").unwrap());
    }

    #[test]
    fn delete_removes_secret() {
        let (_dir, store) = store();
        store.put("s1", "value-1234567890").unwrap();
        assert!(store.delete("s1").unwrap());
        assert!(store.get("s1").unwrap().is_none());
        assert!(!store.delete("s1").unwrap(), "second delete is a no-op");
    }

    #[test]
    fn fingerprint_masks_all_but_tail() {
        let (_dir, store) = store();
        store.put("s1", "sk-abcdefghijklmnop").unwrap();
        let fp = store.fingerprint("s1").unwrap().unwrap();
        assert!(fp.starts_with("****"), "{fp}");
        assert_eq!(fp.len(), 8);
        assert!(!fp.contains("abcdefghij"), "fingerprint leaked body: {fp}");
    }

    #[test]
    fn update_preserves_created_at() {
        let (_dir, store) = store();
        store.put("s1", "first-value-123456").unwrap();
        let data = store.load_raw().unwrap();
        let created = data.records.get("s1").unwrap().created_at.clone();

        std::thread::sleep(std::time::Duration::from_millis(5));
        store.put("s1", "second-value-12345").unwrap();
        let data = store.load_raw().unwrap();
        assert_eq!(data.records.get("s1").unwrap().created_at, created);
        assert_eq!(store.get("s1").unwrap().unwrap(), "second-value-12345");
    }

    #[test]
    fn rejects_empty_secret() {
        let (_dir, store) = store();
        assert!(store.put("s1", "").is_err());
        assert!(store.put("", "value").is_err());
    }

    #[test]
    fn corrupt_file_gives_actionable_error() {
        let (_dir, store) = store();
        std::fs::write(store.path(), b"not json at all").unwrap();
        let err = store.get("s1").unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Secret);
        assert!(err.hint.is_some());
    }

    #[test]
    fn count_and_ids_track_records() {
        let (_dir, store) = store();
        store.put("a", "value-a-12345678").unwrap();
        store.put("b", "value-b-12345678").unwrap();
        assert_eq!(store.count().unwrap(), 2);
        let mut ids = store.ids().unwrap();
        ids.sort();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.dat");
        {
            let store = SecretStore::with_cipher(&path, Box::new(PlaintextCipher::new()));
            store.put("persisted", "sk-persist-check-1234").unwrap();
        }
        let reopened = SecretStore::with_cipher(&path, Box::new(PlaintextCipher::new()));
        assert_eq!(
            reopened.get("persisted").unwrap().unwrap(),
            "sk-persist-check-1234"
        );
    }
}
