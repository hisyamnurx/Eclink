//! Local diagnostics: logging, redaction, and the support bundle (PRD §37, §38).
//!
//! There is no remote log sink and no crash reporter. Everything stays under
//! `%LOCALAPPDATA%\EclyzerBridge\logs\` and is pruned to 7 days / 20 MB.

pub mod log;
pub mod redact;
pub mod report;

pub use log::{
    clear_logs, init_logging, prune_logs, read_recent_log_lines, read_recent_logs, LogGuard,
    LogLevel,
};
pub use redact::{fingerprint, redact_header, redact_text, REDACTED};
pub use report::{build_report, DiagnosticsReport};
