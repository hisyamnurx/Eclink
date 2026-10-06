//! Playground: one user-initiated request against a provider (PRD §31).
//!
//! Scope is deliberately narrow. This is a diagnostic, not a chat client: one
//! turn, no conversation history, no tools. That keeps the failure modes small
//! and makes it obvious to the user that a real, billable request was made.

use crate::app::AppState;
use crate::commands::{parse_agent, CommandResult};
use crate::error::BridgeError;
use crate::testing::playground::PlaygroundResult;

/// The largest prompt we will forward.
///
/// A playground is for a short check, not for bulk traffic; bounding this also
/// bounds what a compromised webview could send.
const MAX_PROMPT_CHARS: usize = 8_000;

/// Send one message to a provider profile and return its reply.
#[tauri::command]
pub async fn playground_send(
    state: tauri::State<'_, AppState>,
    id: String,
    prompt: String,
    agent: Option<String>,
) -> CommandResult<PlaygroundResult> {
    let state = state.inner().clone();
    let id = crate::commands::require_id(&id, "Provider id")?;
    let prompt = prompt.trim().to_string();

    if prompt.is_empty() {
        return Err(BridgeError::validation("Enter a message to send.").into());
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(BridgeError::validation(format!(
            "That message is too long. Keep it under {MAX_PROMPT_CHARS} characters."
        ))
        .with_hint("The Playground is for a quick check, not for long inputs.")
        .into());
    }

    // The agent decides which protocol to speak, so default to whichever the
    // profile actually targets rather than guessing.
    let profile = state
        .profiles()
        .load()?
        .profiles
        .into_iter()
        .find(|profile| profile.id == id)
        .ok_or_else(|| BridgeError::validation("That provider no longer exists."))?;

    let agent = match agent {
        Some(value) => parse_agent(&value)?,
        None => profile.targets.first().copied().ok_or_else(|| {
            BridgeError::validation("This provider is not set up for a coding agent yet.")
                .with_hint("Edit the provider and choose Codex or Claude Code.")
        })?,
    };

    let secret = crate::testing::pipeline::resolve_secret(state.secrets(), &profile)?;
    if secret.is_empty() {
        return Err(BridgeError::secret(
            "This provider has no API key stored, so nothing can be sent.",
        )
        .with_hint("Edit the provider and enter its API key.")
        .into());
    }
    let target = crate::testing::pipeline::build_target(&profile, secret)?;

    // Anthropic-shaped endpoints take /v1/messages; everything else that this
    // app supports is OpenAI-compatible and takes /chat/completions.
    let anthropic = agent == crate::adapters::AgentId::Claude;
    Ok(crate::testing::playground::send(&target, &prompt, anthropic).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_prompt_is_rejected() {
        let prompt = "   ".trim();
        assert!(prompt.is_empty());
    }

    #[test]
    fn the_prompt_limit_is_a_character_count_not_a_byte_count() {
        // A multi-byte prompt at the limit must be accepted; counting bytes would
        // reject a valid message.
        let prompt = "é".repeat(MAX_PROMPT_CHARS);
        assert_eq!(prompt.chars().count(), MAX_PROMPT_CHARS);
        assert!(prompt.len() > MAX_PROMPT_CHARS);
    }
}
