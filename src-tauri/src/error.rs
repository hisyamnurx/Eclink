//! Error taxonomy for Eclyzer Bridge.
//!
//! Every variant carries user-actionable copy. The UI renders `message()` and
//! `hint()` directly; raw upstream detail is preserved separately so the
//! "View Details" disclosure can show it without polluting the primary error
//! text (PRD §41, §46).

use serde::{Deserialize, Serialize};

pub type BridgeResult<T> = Result<T, BridgeError>;

/// Machine-readable error category, used by the UI to pick an icon/tone and by
/// tests to assert on failure classes without string matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Invalid input supplied by the caller (frontend is untrusted).
    Validation,
    /// Filesystem/permission problem.
    Io,
    /// Existing config could not be parsed; Bridge refuses to touch it.
    ConfigInvalid,
    /// Config changed on disk since Bridge last read it.
    ConfigConflict,
    /// Secret store failure (DPAPI, corrupt file).
    Secret,
    /// Network/protocol failure while talking to a provider.
    Network,
    /// Provider rejected credentials.
    Auth,
    /// Requested model is not available at the endpoint.
    ModelNotFound,
    /// Endpoint does not speak the protocol the agent requires.
    Incompatible,
    /// Rate limited or quota exhausted.
    RateLimited,
    /// Upstream provider/server error.
    ProviderError,
    /// Agent binary not found or not usable.
    AgentUnavailable,
    /// Operation blocked by organization-managed configuration.
    ManagedPolicy,
    /// Two mutations attempted at once for the same agent.
    Locked,
    /// Update subsystem failure.
    Update,
    /// Anything not otherwise classified.
    Internal,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Validation => "validation",
            ErrorKind::Io => "io",
            ErrorKind::ConfigInvalid => "config_invalid",
            ErrorKind::ConfigConflict => "config_conflict",
            ErrorKind::Secret => "secret",
            ErrorKind::Network => "network",
            ErrorKind::Auth => "auth",
            ErrorKind::ModelNotFound => "model_not_found",
            ErrorKind::Incompatible => "incompatible",
            ErrorKind::RateLimited => "rate_limited",
            ErrorKind::ProviderError => "provider_error",
            ErrorKind::AgentUnavailable => "agent_unavailable",
            ErrorKind::ManagedPolicy => "managed_policy",
            ErrorKind::Locked => "locked",
            ErrorKind::Update => "update",
            ErrorKind::Internal => "internal",
        }
    }
}

/// A structured, serializable error crossing the Tauri IPC boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeError {
    pub kind: ErrorKind,
    /// One-line headline, e.g. "Authentication failed".
    pub message: String,
    /// Actionable follow-up, e.g. "Check the key and try again."
    pub hint: Option<String>,
    /// Optional technical detail, already secret-redacted.
    pub details: Option<String>,
}

impl BridgeError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hint: None,
            details: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        // Defense in depth: nothing reaches the UI unredacted, even if a caller
        // forgot. Redaction is idempotent.
        self.details = Some(crate::diagnostics::redact::redact_text(&details.into()));
        self
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Validation, message)
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Io, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    pub fn config_invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ConfigInvalid, message)
    }

    pub fn config_conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ConfigConflict, message)
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Network, message)
    }

    pub fn incompatible(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Incompatible, message)
    }

    pub fn managed_policy(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ManagedPolicy, message)
    }

    pub fn locked(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Locked, message)
    }

    /// A credential problem: missing, unreadable, or unusable.
    ///
    /// Messages built with this constructor must never contain the secret
    /// itself — only a description of what is wrong with it.
    pub fn secret(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Secret, message)
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Auth, message)
    }

    pub fn update(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Update, message)
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, " ({hint})")?;
        }
        Ok(())
    }
}

impl std::error::Error for BridgeError {}

impl From<std::io::Error> for BridgeError {
    fn from(err: std::io::Error) -> Self {
        // Every I/O failure is reported as `Io`; the specific cause is carried
        // in the message and hint so the UI can still give useful guidance.
        let kind = ErrorKind::Io;
        let message = match err.kind() {
            std::io::ErrorKind::PermissionDenied => "Eclyzer Bridge cannot write to the config file.",
            std::io::ErrorKind::NotFound => "The requested file or folder was not found.",
            _ => "A file operation failed.",
        };
        let hint = match err.kind() {
            std::io::ErrorKind::PermissionDenied => Some("Check file permissions."),
            std::io::ErrorKind::NotFound => Some("Verify the path and try again."),
            _ => Some("Check the log for details."),
        };
        Self {
            kind,
            message: message.to_string(),
            hint: hint.map(str::to_string),
            details: Some(crate::diagnostics::redact::redact_text(&err.to_string())),
        }
    }
}

impl From<serde_json::Error> for BridgeError {
    fn from(err: serde_json::Error) -> Self {
        Self {
            kind: ErrorKind::ConfigInvalid,
            message: "Configuration issue detected.".to_string(),
            hint: Some("Eclyzer Bridge won't modify this file until it can be read safely.".into()),
            details: Some(crate::diagnostics::redact::redact_text(&err.to_string())),
        }
    }
}

impl From<reqwest::Error> for BridgeError {
    fn from(err: reqwest::Error) -> Self {
        // `reqwest::Error` Display never includes request headers, but we still
        // route it through the redactor for defense in depth.
        let details = crate::diagnostics::redact::redact_text(&err.to_string());
        let mut out = if err.is_timeout() {
            Self::network("The provider did not respond in time.")
                .with_hint("Check the base URL and your network connection, then try again.")
        } else if err.is_connect() {
            Self::network("Could not connect to the provider.")
                .with_hint("Check the base URL and your network connection.")
        } else if err.is_decode() {
            Self::network("The provider returned a response Eclyzer Bridge could not read.")
                .with_hint("The endpoint may not speak this API format.")
        } else {
            Self::network("The request to the provider failed.")
                .with_hint("Check the base URL and try again.")
        };
        out.details = Some(details);
        out
    }
}

impl From<anyhow::Error> for BridgeError {
    fn from(err: anyhow::Error) -> Self {
        Self::internal("An unexpected error occurred.")
            .with_details(crate::diagnostics::redact::redact_text(&format!("{err:#}")))
    }
}
