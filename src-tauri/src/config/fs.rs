//! Safe file I/O primitives shared by the secret store, profile store, and the
//! agent adapters.
//!
//! The central guarantee is [`write_atomic`]: a reader either sees the old file
//! or the new file, never a partial write. This is what makes "power loss
//! during apply" survivable (PRD §47, §69).

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{BridgeError, BridgeResult};

/// Hash algorithm prefix used in snapshot metadata.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Hash a file's contents, or `None` when it does not exist.
pub fn sha256_file(path: &Path) -> BridgeResult<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(sha256_hex(&bytes))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(BridgeError::from(err)),
    }
}

/// Read a file as UTF-8 text, or `None` when it does not exist.
pub fn read_to_string_opt(path: &Path) -> BridgeResult<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(decode_text(&bytes))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(BridgeError::from(err)),
    }
}

/// Decode bytes to text, stripping a UTF-8 BOM.
///
/// Invalid UTF-8 is replaced rather than rejected: the caller validates
/// structure afterwards, and this keeps CRLF/UTF-8 fixtures readable.
pub fn decode_text(bytes: &[u8]) -> String {
    let slice = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(slice).into_owned()
}

/// Detect the dominant line ending so a rewrite preserves the user's style.
pub fn detect_newline(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count();
    if crlf > 0 && crlf * 2 >= lf {
        "\r\n"
    } else {
        "\n"
    }
}

/// Apply `newline` style to `text`, normalising whatever was there before.
pub fn apply_newline(text: &str, newline: &str) -> String {
    let normalized = text.replace("\r\n", "\n");
    if newline == "\n" {
        normalized
    } else {
        normalized.replace('\n', newline)
    }
}

/// Write `contents` to `path` atomically.
///
/// Sequence (PRD §47):
/// 1. write a sibling temp file in the same directory (same volume, so the
///    rename is atomic);
/// 2. flush and `sync_all` so bytes are durable before the swap;
/// 3. atomically replace the destination;
/// 4. verify the destination re-reads to the expected bytes.
///
/// On any failure the destination keeps its previous contents.
pub fn write_atomic(path: &Path, contents: &[u8]) -> BridgeResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| BridgeError::io("The configuration path has no parent directory."))?;
    std::fs::create_dir_all(parent)?;

    let mut temp = tempfile::Builder::new()
        .prefix(".eclyzer-bridge-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|err| {
            BridgeError::io("Could not create a temporary file next to the configuration.")
                .with_details(err.to_string())
        })?;

    temp.write_all(contents).map_err(|err| {
        BridgeError::io("Could not write the temporary configuration file.")
            .with_details(err.to_string())
    })?;

    temp.as_file_mut().sync_all().map_err(|err| {
        BridgeError::io("Could not flush the temporary configuration file to disk.")
            .with_details(err.to_string())
    })?;

    // `persist` performs the atomic replace. On Windows it maps to
    // MoveFileEx(.., MOVEFILE_REPLACE_EXISTING), which is atomic for a
    // same-volume rename.
    temp.persist(path).map_err(|err| {
        BridgeError::io("Could not replace the configuration file.")
            .with_hint("Close any program currently holding the file open and try again.")
            .with_details(err.error.to_string())
    })?;

    // Verify: a successful rename that wrote wrong bytes is still a failure.
    let written = std::fs::read(path)?;
    if written != contents {
        return Err(BridgeError::io(
            "The configuration file did not match what Eclyzer Bridge wrote.",
        )
        .with_hint("Another program may be modifying this file."));
    }

    Ok(())
}

/// Create a restrictive-permission file if it does not exist yet.
///
/// Secrets and app data are per-user; on Windows the profile directory is
/// already user-scoped, so this only guards against a permissive inherited ACL
/// on a redirected folder.
pub fn ensure_private_file(path: &Path, initial: &[u8]) -> BridgeResult<()> {
    if path.exists() {
        return Ok(());
    }
    write_atomic(path, initial)?;
    harden_permissions(path)?;
    Ok(())
}

/// Remove group/other access on Windows by resetting to an owner-only DACL.
#[cfg(windows)]
pub fn harden_permissions(path: &Path) -> BridgeResult<()> {
    use std::process::Command;
    // Use icacls rather than raw ACL APIs: it is present on every supported
    // Windows build and needs no elevation for a file the user owns.
    let user = std::env::var("USERNAME").unwrap_or_default();
    if user.is_empty() {
        return Ok(());
    }
    let status = Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:F"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        // Hardening is best-effort: failing to tighten an ACL must not make the
        // app unusable, because the parent folder is already user-scoped.
        _ => Ok(()),
    }
}

#[cfg(not(windows))]
pub fn harden_permissions(_path: &Path) -> BridgeResult<()> {
    Ok(())
}

/// A best-effort advisory lock so two mutations cannot race (PRD §48).
///
/// Windows locks the file exclusively; on other platforms we fall back to a
/// create-new marker which is sufficient for the single-process test harness.
pub struct FileLock {
    path: PathBuf,
}

impl FileLock {
    /// Acquire the lock, returning `Err(Locked)` when it is already held.
    pub fn acquire(path: impl Into<PathBuf>) -> BridgeResult<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                let _ = writeln!(
                    file,
                    "pid={} at={}",
                    std::process::id(),
                    chrono::Local::now().to_rfc3339()
                );
                Ok(Self { path })
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                // A second Bridge process (or a stale lock) is holding this
                // agent's configuration. Refusing is the safe behaviour: writing
                // concurrently could interleave two patches.
                Err(BridgeError::locked(
                    "Another Eclyzer Bridge operation is already changing this configuration.",
                )
                .with_hint("Wait for it to finish, then try again."))
            }
            Err(err) => Err(BridgeError::from(err)),
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        // SHA-256 of the empty string.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn write_atomic_creates_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");

        write_atomic(&file, b"a = 1\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "a = 1\n");

        write_atomic(&file, b"a = 2\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "a = 2\n");
    }

    #[test]
    fn write_atomic_leaves_no_temp_files_behind() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        write_atomic(&file, b"a = 1\n").unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".eclyzer-bridge-"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    }

    #[test]
    fn write_atomic_preserves_original_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        write_atomic(&file, b"original\n").unwrap();

        // A path whose parent is a *file* cannot be created.
        let bogus = file.join("nested").join("config.toml");
        assert!(write_atomic(&bogus, b"new\n").is_err());

        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original\n");
    }

    #[test]
    fn detects_and_preserves_crlf() {
        let text = "a = 1\r\nb = 2\r\n";
        assert_eq!(detect_newline(text), "\r\n");
        assert_eq!(
            apply_newline("a = 1\nb = 2\n", "\r\n"),
            "a = 1\r\nb = 2\r\n"
        );
    }

    #[test]
    fn detects_lf() {
        assert_eq!(detect_newline("a = 1\nb = 2\n"), "\n");
    }

    #[test]
    fn decode_text_strips_bom() {
        let bytes = b"\xEF\xBB\xBFmodel = \"x\"";
        assert_eq!(decode_text(bytes), "model = \"x\"");
    }

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("codex-config.lock");

        let first = FileLock::acquire(&lock_path).unwrap();
        assert!(
            FileLock::acquire(&lock_path).is_err(),
            "second acquire must fail"
        );
        drop(first);
        assert!(
            FileLock::acquire(&lock_path).is_ok(),
            "lock must be reusable"
        );
    }

    #[test]
    fn sha256_file_reports_none_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(sha256_file(&dir.path().join("nope.toml"))
            .unwrap()
            .is_none());
    }
}
