//! File logging with mandatory redaction and bounded retention.
//!
//! Design notes:
//! * the log file is the only sink — no network, no crash service (PRD §39);
//! * a custom [`Layer`] redacts every formatted record before it is written, so
//!   a careless `info!("{err}")` cannot leak a key;
//! * retention runs at startup only, never on a background timer, so the app
//!   keeps near-zero idle CPU (PRD §68).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use crate::diagnostics::redact::redact_text;
use crate::error::{BridgeError, BridgeResult};

/// Log verbosity, persisted in `app.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
}

impl LogLevel {
    pub fn as_filter(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }
}

/// Maximum age of a log file before pruning.
const MAX_LOG_AGE_DAYS: i64 = 7;
/// Maximum total size of the log directory.
const MAX_LOG_TOTAL_BYTES: u64 = 20 * 1024 * 1024;

/// Keeps the non-blocking writer alive for the process lifetime.
pub struct LogGuard {
    _guard: tracing_appender::non_blocking::WorkerGuard,
    dir: PathBuf,
    file: PathBuf,
}

impl LogGuard {
    pub fn log_dir(&self) -> &Path {
        &self.dir
    }

    pub fn active_file(&self) -> &Path {
        &self.file
    }
}

/// Wraps an inner writer and redacts each complete line before writing.
struct RedactingWriter<W: Write> {
    inner: W,
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // tracing writes ANSI-free UTF-8 lines; lossy conversion is safe here
        // and guarantees we never panic on a malformed slice.
        let text = String::from_utf8_lossy(buf);
        let redacted = redact_text(&text);
        self.inner.write_all(redacted.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// A `MakeWriter` adapter that wraps each produced writer in the redactor.
///
/// `W` is the inner `MakeWriter` (for example `tracing_appender`'s
/// `NonBlocking`), and `W::Writer` is the concrete `Write` it hands out.
struct RedactingMakeWriter<W> {
    inner: W,
}

impl<'a, W> tracing_subscriber::fmt::MakeWriter<'a> for RedactingMakeWriter<W>
where
    W: tracing_subscriber::fmt::MakeWriter<'a>,
{
    type Writer = RedactingWriter<W::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        // The inner writer is already internally synchronized, so building a
        // fresh wrapper per record is safe and cheap.
        RedactingWriter {
            inner: self.inner.make_writer(),
        }
    }
}

static LOG_GUARD: OnceLock<PathBuf> = OnceLock::new();

/// Initialise logging into `dir`.
///
/// Returns a guard that must be held for the process lifetime; dropping it
/// stops the background flusher.
pub fn init_logging(dir: &Path, level: LogLevel) -> BridgeResult<LogGuard> {
    std::fs::create_dir_all(dir).map_err(|err| {
        BridgeError::io("Could not create the log folder.").with_details(err.to_string())
    })?;

    prune_logs(dir);

    let file_appender = tracing_appender::rolling::daily(dir, "eclyzer-bridge.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level.as_filter()));

    let make_writer = RedactingMakeWriter {
        inner: non_blocking,
    };

    // Local wall-clock timestamps make log correlation with user reports much
    // easier than UTC. `chrono` needs its `clock` feature, which is enabled by
    // default in the workspace dependency.
    let timer = tracing_subscriber::fmt::time::ChronoLocal::rfc_3339();

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(make_writer)
        .with_ansi(false)
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(true)
        .with_timer(timer);

    // `try_init` fails if a subscriber is already installed (tests, hot reload).
    // That is not an error condition for the app.
    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .try_init();

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let active = dir.join(format!("eclyzer-bridge.log.{today}"));

    let _ = LOG_GUARD.set(dir.to_path_buf());

    Ok(LogGuard {
        _guard: guard,
        dir: dir.to_path_buf(),
        file: active,
    })
}

/// Delete log files older than 7 days, then trim total size to 20 MB.
pub fn prune_logs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    let mut files: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        files.push((path, modified, meta.len()));
    }

    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(60 * 60 * 24 * MAX_LOG_AGE_DAYS as u64);

    let mut kept: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();
    for (path, modified, len) in files {
        if modified < cutoff {
            let _ = std::fs::remove_file(&path);
        } else {
            kept.push((path, modified, len));
        }
    }

    let total: u64 = kept.iter().map(|(_, _, len)| *len).sum();
    if total <= MAX_LOG_TOTAL_BYTES {
        return;
    }

    // Drop oldest first until the directory fits the budget.
    kept.sort_by_key(|(_, modified, _)| *modified);
    let mut remaining = total;
    for (path, _, len) in kept {
        if remaining <= MAX_LOG_TOTAL_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            remaining = remaining.saturating_sub(len);
        }
    }
}

/// Delete every log file. Used by the "Clear Logs" settings action.
///
/// The active log file is held open by the appender, so on Windows it may
/// refuse to be deleted. That is not an error worth surfacing: the remaining
/// files are cleared and the current one keeps growing normally.
pub fn clear_logs(dir: &Path) -> BridgeResult<u64> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // No log directory yet means there is nothing to clear.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => {
            return Err(
                BridgeError::io("Could not open the log folder.").with_details(err.to_string())
            )
        }
    };
    let mut removed = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Read the tail of the most recent log file, redacted, as lines.
///
/// `max_lines` bounds the result so a large log cannot stall the UI.
pub fn read_recent_log_lines(dir: &Path, max_lines: usize) -> BridgeResult<Vec<String>> {
    let text = read_recent_logs(dir, max_lines.saturating_mul(512).max(4096))?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines.len() > max_lines {
        lines.drain(..lines.len() - max_lines);
    }
    Ok(lines)
}

/// Read the tail of the most recent log file, redacted.
///
/// `max_bytes` bounds how much of the file is read; the tail is taken so the
/// most recent entries survive.
pub fn read_recent_logs(dir: &Path, max_bytes: usize) -> BridgeResult<String> {
    let mut newest: Option<(PathBuf, std::time::SystemTime)> = None;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(String::new());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if newest.as_ref().is_none_or(|(_, best)| modified > *best) {
            newest = Some((path, modified));
        }
    }

    let Some((path, _)) = newest else {
        return Ok(String::new());
    };

    let data = std::fs::read(&path)?;
    let start = data.len().saturating_sub(max_bytes);
    let slice = &data[start..];

    // Starting mid-line would show a torn record; drop the partial first line
    // unless we are at the true start of the file.
    let text = String::from_utf8_lossy(slice);
    let text = if start > 0 {
        match text.find('\n') {
            Some(index) => text[index + 1..].to_string(),
            // A single enormous line: better to show nothing than a fragment.
            None => String::new(),
        }
    } else {
        text.to_string()
    };

    // Redact again even though the writer already redacts: a log file could have
    // been edited or restored from elsewhere.
    Ok(redact_text(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The redacting writer must scrub a secret before it reaches the file.
    #[test]
    fn redacting_writer_masks_secrets() {
        let mut sink: Vec<u8> = Vec::new();
        {
            let mut writer = RedactingWriter { inner: &mut sink };
            writer
                .write_all(b"Authorization: Bearer sk-abcdef0123456789abcdef\n")
                .unwrap();
            writer.flush().unwrap();
        }
        let text = String::from_utf8(sink).unwrap();
        assert!(!text.contains("sk-abcdef0123456789abcdef"), "{text}");
        assert!(text.contains("[REDACTED]"), "{text}");
    }

    #[test]
    fn prune_removes_old_files_and_keeps_recent() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("eclyzer-bridge.log.2000-01-01");
        std::fs::write(&old, b"old").unwrap();
        let recent = dir.path().join("eclyzer-bridge.log.recent");
        std::fs::write(&recent, b"recent").unwrap();

        // Backdate the old file well past the retention window.
        let old_time =
            std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 60 * 24 * 30);
        filetime_set(&old, old_time);

        prune_logs(dir.path());

        assert!(!old.exists(), "old log must be pruned");
        assert!(recent.exists(), "recent log must survive");
    }

    /// Minimal mtime setter so tests do not need an extra dependency.
    fn filetime_set(path: &Path, time: std::time::SystemTime) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        let duration = time
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let _ = file.set_modified(time);
        let _ = duration;
    }

    #[test]
    fn clear_logs_removes_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.log"), b"x").unwrap();
        std::fs::write(dir.path().join("b.log"), b"y").unwrap();
        let removed = clear_logs(dir.path()).unwrap();
        assert_eq!(removed, 2);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn read_recent_logs_returns_redacted_tail() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("eclyzer-bridge.log.2026-01-01"),
            b"info: start\nAuthorization: Bearer sk-abcdef0123456789abcdef\n",
        )
        .unwrap();
        let out = read_recent_logs(dir.path(), 4096).unwrap();
        assert!(out.contains("start"));
        assert!(!out.contains("sk-abcdef0123456789abcdef"), "{out}");
    }
}
