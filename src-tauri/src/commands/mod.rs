//! Domain-specific Tauri commands (PRD §34).
//!
//! ## Why there is no generic escape hatch
//!
//! Every command here answers a product question. There is deliberately **no**
//! `run_command`, `read_file`, or `write_file`. A generic command would make the
//! webview a trust boundary: any script that reached it could read `secrets.dat`
//! or write to a user's shell profile. The frontend is treated as untrusted
//! input, and every payload is validated in Rust.

mod agents;
mod diagnostics;
mod launch;
mod profiles;
mod recovery;
mod settings;
mod update;

pub use agents::*;
pub use diagnostics::*;
pub use launch::*;
pub use profiles::*;
pub use recovery::*;
pub use settings::*;
pub use update::*;

use serde::{Deserialize, Serialize};

use crate::adapters::AgentId;
use crate::error::{BridgeError, BridgeResult};

/// An error as the frontend receives it.
///
/// Serialising the domain error rather than a string keeps the UI able to react
/// to `kind` — for example, offering a restore action on `config_invalid`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub kind: crate::error::ErrorKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl From<BridgeError> for CommandError {
    fn from(err: BridgeError) -> Self {
        Self {
            kind: err.kind,
            message: err.message,
            hint: err.hint,
            // `BridgeError` already redacts details at construction time.
            details: err.details,
        }
    }
}

/// The result type every command returns.
pub type CommandResult<T> = Result<T, CommandError>;

/// Parse an agent identifier coming from the frontend, which is untrusted.
pub(crate) fn parse_agent(value: &str) -> BridgeResult<AgentId> {
    AgentId::parse(value).ok_or_else(|| {
        BridgeError::validation(format!("\"{value}\" is not a supported coding agent."))
            .with_hint("Supported agents are Codex and Claude Code.")
    })
}

/// Reject an empty or oversized identifier before it reaches the filesystem.
pub(crate) fn require_id(value: &str, what: &str) -> BridgeResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(BridgeError::validation(format!("{what} is required.")));
    }
    if trimmed.len() > 128 {
        return Err(BridgeError::validation(format!(
            "{what} is too long (maximum 128 characters)."
        )));
    }
    // Identifiers are used in file names and log lines; keep them boring.
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(BridgeError::validation(format!(
            "{what} may contain only letters, numbers, hyphens, and underscores."
        )));
    }
    Ok(trimmed.to_string())
}

/// Run a blocking task off the UI thread, mapping a panic into a clean error.
///
/// Filesystem and process work is synchronous; running it on the async runtime
/// would stall the window. A panic inside the task is reported as an internal
/// error rather than poisoning the command.
pub(crate) async fn blocking<T, F>(task: F) -> CommandResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> BridgeResult<T> + Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(task).await {
        Ok(result) => result.map_err(CommandError::from),
        Err(err) => Err(CommandError::from(
            BridgeError::internal("The operation could not be completed.")
                .with_details(err.to_string()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_parsing_accepts_known_names() {
        assert_eq!(parse_agent("codex").unwrap(), AgentId::Codex);
        assert_eq!(parse_agent("claude").unwrap(), AgentId::Claude);
        assert_eq!(parse_agent("CLAUDE").unwrap(), AgentId::Claude);
    }

    #[test]
    fn agent_parsing_rejects_unknown_names() {
        let err = parse_agent("../../etc/passwd").unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Validation);
    }

    #[test]
    fn identifiers_reject_path_traversal() {
        // An id becomes part of a file name, so traversal must be impossible.
        for hostile in ["../escape", "a/b", "a\\b", "a b", "a\0b"] {
            assert!(
                require_id(hostile, "Profile id").is_err(),
                "{hostile} must be rejected"
            );
        }
    }

    #[test]
    fn identifiers_reject_empty_and_oversized_values() {
        assert!(require_id("", "Profile id").is_err());
        assert!(require_id("   ", "Profile id").is_err());
        assert!(require_id(&"a".repeat(129), "Profile id").is_err());
    }

    #[test]
    fn identifiers_accept_a_uuid() {
        let id = uuid::Uuid::new_v4().to_string();
        assert_eq!(require_id(&id, "Profile id").unwrap(), id);
    }

    #[test]
    fn command_errors_preserve_the_kind_for_the_ui() {
        let err = BridgeError::config_invalid("bad config").with_hint("fix it");
        let command_error: CommandError = err.into();
        assert_eq!(command_error.kind, crate::error::ErrorKind::ConfigInvalid);
        assert_eq!(command_error.hint.as_deref(), Some("fix it"));
    }
}
