//! A single user-initiated request to a provider (PRD §31).
//!
//! Reuses the probe target so the Playground sends credentials through exactly
//! the same header path as Test Connection, and never logs the key.

use serde::Serialize;

use crate::error::{BridgeError, BridgeResult};
use crate::testing::probes::ProbeTarget;

/// The provider's reply to a single playground message.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaygroundResult {
    pub text: String,
    pub latency_ms: u64,
    pub model: String,
    pub streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u32>,
}
/// The longest reply we will hold in memory for display.
const MAX_REPLY_CHARS: usize = 100_000;

/// Send one message and return the provider's reply.
///
/// Chooses the protocol from the target's auth mode, which the caller derives
/// from the agent the profile targets: Anthropic-style endpoints take
/// `/v1/messages`, OpenAI-compatible ones take `/chat/completions`.
pub async fn send(
    target: &ProbeTarget,
    prompt: &str,
    anthropic: bool,
) -> BridgeResult<PlaygroundResult> {
    let model = target.model.clone();

    let (path, body) = if anthropic {
        (
            "v1/messages",
            serde_json::json!({
                "model": model,
                "max_tokens": 1024,
                "messages": [{ "role": "user", "content": prompt }],
            }),
        )
    } else {
        (
            "chat/completions",
            serde_json::json!({
                "model": model,
                "messages": [{ "role": "user", "content": prompt }],
            }),
        )
    };

    let request = target.post(path, body)?;
    let started = std::time::Instant::now();

    let response = request.send().await.map_err(|err| {
        // A transport failure must never leak the URL, which may carry a key in
        // a query parameter on some gateways.
        if err.is_timeout() {
            BridgeError::network("The provider did not respond in time.")
                .with_hint("Check that the endpoint is reachable, then try again.")
        } else {
            BridgeError::network("Could not reach the provider.")
                .with_hint("Check the base URL and your internet connection.")
                .with_details(err.to_string())
        }
    })?;

    let latency_ms = started.elapsed().as_millis() as u64;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(describe_failure(status, &text));
    }

    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
        BridgeError::provider_error(
            "The provider's reply was not in the format this endpoint should use.",
        )
        .with_hint("The endpoint may not be compatible with this agent's protocol.")
        .with_details(err.to_string())
    })?;

    let (reply, prompt_tokens, completion_tokens) = if anthropic {
        extract_anthropic(&parsed)
    } else {
        extract_openai(&parsed)
    };

    if reply.trim().is_empty() {
        return Err(
            BridgeError::provider_error("The provider returned an empty reply.")
                .with_hint("Try a different model, or check the provider's status page."),
        );
    }

    Ok(PlaygroundResult {
        text: truncate_chars(&reply, MAX_REPLY_CHARS),
        latency_ms,
        model,
        // A non-streaming playground call: saying otherwise would be a lie.
        streaming: false,
        prompt_tokens,
        completion_tokens,
    })
}

/// Map a non-success response onto an actionable error.
fn describe_failure(status: reqwest::StatusCode, body: &str) -> BridgeError {
    let detail = crate::testing::probes::safe_error_excerpt(body);
    match status.as_u16() {
        401 | 403 => BridgeError::auth("The provider rejected your API key.")
            .with_hint("Check that the key is correct and still active, then save it again."),
        404 => {
            BridgeError::provider_error("The provider does not recognize that endpoint or model.")
                .with_hint("Check the base URL and the model ID.")
                .with_details(detail)
        }
        429 => BridgeError::rate_limited("The provider is rate limiting this key.")
            .with_hint("Wait a moment, then try again.")
            .with_details(detail),
        s if (500..600).contains(&s) => {
            BridgeError::provider_error("The provider reported a server error.")
                .with_hint("This is on the provider's side. Try again shortly.")
                .with_details(detail)
        }
        _ => BridgeError::provider_error("The provider rejected the request.").with_details(detail),
    }
}

/// Pull the assistant text and token counts out of an Anthropic-shaped reply.
fn extract_anthropic(value: &serde_json::Value) -> (String, Option<u32>, Option<u32>) {
    let text = value
        .get("content")
        .and_then(|content| content.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    let usage = value.get("usage");
    (
        text,
        usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
    )
}

/// Pull the assistant text and token counts out of an OpenAI-shaped reply.
fn extract_openai(value: &serde_json::Value) -> (String, Option<u32>, Option<u32>) {
    let text = value
        .get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .unwrap_or_default()
        .to_string();

    let usage = value.get("usage");
    (
        text,
        usage
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        usage
            .and_then(|u| u.get("completion_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
    )
}

/// Truncate on a character boundary, never mid-codepoint.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("\n\n[reply truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_replies_are_parsed() {
        let value = serde_json::json!({
            "content": [{ "type": "text", "text": "ready" }],
            "usage": { "input_tokens": 7, "output_tokens": 2 }
        });
        let (text, input, output) = extract_anthropic(&value);
        assert_eq!(text, "ready");
        assert_eq!(input, Some(7));
        assert_eq!(output, Some(2));
    }

    #[test]
    fn anthropic_replies_join_multiple_blocks() {
        let value = serde_json::json!({
            "content": [
                { "type": "text", "text": "one " },
                { "type": "text", "text": "two" }
            ]
        });
        let (text, _, _) = extract_anthropic(&value);
        assert_eq!(text, "one two");
    }

    #[test]
    fn openai_replies_are_parsed() {
        let value = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "ready" } }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 1 }
        });
        let (text, input, output) = extract_openai(&value);
        assert_eq!(text, "ready");
        assert_eq!(input, Some(5));
        assert_eq!(output, Some(1));
    }

    #[test]
    fn a_missing_shape_yields_empty_text_not_a_panic() {
        let value = serde_json::json!({ "unexpected": true });
        assert_eq!(extract_anthropic(&value).0, "");
        assert_eq!(extract_openai(&value).0, "");
    }

    #[test]
    fn truncation_respects_codepoint_boundaries() {
        // Slicing by byte would panic here; the limit is six characters.
        let text = "日本語のテキストです";
        let out = truncate_chars(text, 3);
        assert!(out.starts_with("日本語"));
        assert!(out.contains("[reply truncated]"));
    }

    #[test]
    fn short_text_is_returned_unchanged() {
        assert_eq!(truncate_chars("hello", 100), "hello");
    }

    #[test]
    fn an_auth_failure_maps_to_the_auth_kind() {
        let err = describe_failure(reqwest::StatusCode::UNAUTHORIZED, "bad key");
        assert_eq!(err.kind, crate::error::ErrorKind::Auth);
    }

    #[test]
    fn a_rate_limit_maps_to_rate_limited() {
        let err = describe_failure(reqwest::StatusCode::TOO_MANY_REQUESTS, "");
        assert_eq!(err.kind, crate::error::ErrorKind::RateLimited);
    }

    #[test]
    fn a_server_error_is_reported_as_a_provider_problem() {
        let err = describe_failure(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "oops");
        assert_eq!(err.kind, crate::error::ErrorKind::ProviderError);
        assert!(err.hint.unwrap().contains("provider"));
    }
}
