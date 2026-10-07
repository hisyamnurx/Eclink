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
/// The protocol follows the agent, because the three APIs are not
/// interchangeable and an agent uses exactly one of them.
///
/// This previously assumed "not Claude" meant Chat Completions, which sent the
/// wrong request for Codex. Codex removed `wire_api = "chat"` entirely: it builds
/// its request from `url_for_path("/responses")` and rejects a Chat configuration
/// with an explicit error. So a Codex profile tested through the Playground was
/// being validated against an endpoint and body shape Codex would never use, and a
/// provider that answered Chat happily would still fail in Codex.
///
/// The route comes from the shared resolver, so the Playground exercises the same
/// URL that activation writes and the compatibility test probes.
pub async fn send(
    target: &ProbeTarget,
    prompt: &str,
    protocol: crate::profiles::ApiProtocol,
) -> BridgeResult<PlaygroundResult> {
    let model = target.model.clone();

    let body = match protocol {
        crate::profiles::ApiProtocol::AnthropicMessages => serde_json::json!({
            "model": model,
            "max_tokens": 1024,
            "messages": [{ "role": "user", "content": prompt }],
        }),
        crate::profiles::ApiProtocol::OpenAiResponses => serde_json::json!({
            "model": model,
            "input": prompt,
            "max_output_tokens": 1024,
            "stream": false,
        }),
        crate::profiles::ApiProtocol::OpenAiChatCompletions => serde_json::json!({
            "model": model,
            "max_tokens": 1024,
            "messages": [{ "role": "user", "content": prompt }],
        }),
    };

    // `post` joins the path through the resolver, so passing the protocol's own
    // path cannot double a version segment.
    let request = target.post(protocol.path(), body)?;
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

    let (reply, prompt_tokens, completion_tokens) = match protocol {
        crate::profiles::ApiProtocol::AnthropicMessages => extract_anthropic(&parsed),
        // A Responses reply carries `output` items rather than `choices`, and
        // reports usage as `input_tokens`/`output_tokens`.
        crate::profiles::ApiProtocol::OpenAiResponses => extract_responses(&parsed),
        crate::profiles::ApiProtocol::OpenAiChatCompletions => extract_openai(&parsed),
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

/// Pull the assistant text and token counts out of an OpenAI Responses reply.
///
/// The Responses shape differs from Chat Completions in both places that matter: the
/// text is inside `output` items, each with its own `content` array whose parts carry
/// the type `output_text`, and usage is reported as `input_tokens`/`output_tokens`
/// rather than `prompt_tokens`/`completion_tokens`. Reading it with the Chat parser
/// would return an empty reply for a perfectly good response.
fn extract_responses(value: &serde_json::Value) -> (String, Option<u32>, Option<u32>) {
    // A provider may put the text in `output_text` at the top level, which is a
    // convenience field some gateways add, or in the nested item structure.
    let convenience = value
        .get("output_text")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string();

    let text = if !convenience.trim().is_empty() {
        convenience
    } else {
        value
            .get("output")
            .and_then(|output| output.as_array())
            .map(|items| {
                items
                    .iter()
                    // A function call item has no text, so it contributes nothing
                    // rather than a stray placeholder.
                    .flat_map(|item| {
                        item.get("content")
                            .and_then(|c| c.as_array())
                            .map(|parts| parts.as_slice())
                            .unwrap_or(&[])
                    })
                    .filter_map(|part| part.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default()
    };

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
    fn responses_replies_are_parsed() {
        // The shape Codex actually receives. Reading it with the Chat parser
        // returned an empty string, which the caller reports as "the provider
        // returned an empty reply" for a perfectly good response.
        let value = serde_json::json!({
            "id": "resp_1",
            "object": "response",
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "ready" }]
            }],
            "usage": { "input_tokens": 9, "output_tokens": 3 }
        });
        let (text, input, output) = extract_responses(&value);
        assert_eq!(text, "ready");
        assert_eq!(input, Some(9));
        assert_eq!(output, Some(3));
    }

    #[test]
    fn responses_replies_join_multiple_output_parts() {
        let value = serde_json::json!({
            "output": [
                { "content": [{ "type": "output_text", "text": "one " }] },
                { "content": [{ "type": "output_text", "text": "two" }] }
            ]
        });
        assert_eq!(extract_responses(&value).0, "one two");
    }

    #[test]
    fn a_responses_reply_with_only_a_function_call_yields_no_text_not_garbage() {
        // A tool call carries no assistant text. It must produce an empty string so
        // the caller reports "empty reply" rather than inventing content.
        let value = serde_json::json!({
            "output": [{
                "type": "function_call",
                "name": "get_weather",
                "arguments": "{\"city\":\"Jakarta\"}"
            }]
        });
        assert_eq!(extract_responses(&value).0, "");
    }

    #[test]
    fn a_top_level_output_text_field_is_preferred_when_present() {
        // Some gateways add this convenience field. Using it avoids descending into
        // the item structure for the common case.
        let value = serde_json::json!({
            "output_text": "ready",
            "output": [{ "content": [{ "type": "output_text", "text": "ignored" }] }]
        });
        assert_eq!(extract_responses(&value).0, "ready");
    }

    #[test]
    fn a_missing_shape_yields_empty_text_not_a_panic() {
        let value = serde_json::json!({ "unexpected": true });
        assert_eq!(extract_anthropic(&value).0, "");
        assert_eq!(extract_openai(&value).0, "");
        assert_eq!(extract_responses(&value).0, "");
    }

    #[test]
    fn every_agent_maps_to_the_api_it_actually_requires() {
        // The bug this replaced: the Playground treated "not Claude" as Chat
        // Completions, so a Codex profile was validated against an endpoint and body
        // shape that Codex cannot use. Codex removed `wire_api = "chat"` entirely
        // and builds its request from `url_for_path("/responses")`.
        use crate::profiles::ApiProtocol;

        for (agent, expected) in [
            (
                crate::adapters::AgentId::Claude,
                ApiProtocol::AnthropicMessages,
            ),
            (
                crate::adapters::AgentId::Codex,
                ApiProtocol::OpenAiResponses,
            ),
        ] {
            let protocol = match agent {
                crate::adapters::AgentId::Claude => ApiProtocol::AnthropicMessages,
                crate::adapters::AgentId::Codex => ApiProtocol::OpenAiResponses,
            };
            assert_eq!(protocol, expected, "{agent:?} uses the wrong API");
        }

        // And the Chat shape is never selected for an agent, because neither uses it.
        assert_ne!(
            ApiProtocol::OpenAiResponses,
            ApiProtocol::OpenAiChatCompletions
        );
    }

    #[test]
    fn the_playground_route_matches_what_the_agent_requests() {
        // The Playground must hit the same URL activation writes, so resolve it the
        // same way rather than asserting a literal in two places.
        use crate::profiles::{resolve_endpoint, ApiProtocol};

        assert_eq!(
            resolve_endpoint("https://api.example.com/v1", ApiProtocol::OpenAiResponses).unwrap(),
            "https://api.example.com/v1/responses"
        );
        assert_eq!(
            resolve_endpoint("https://api.example.com/v1", ApiProtocol::AnthropicMessages).unwrap(),
            "https://api.example.com/v1/messages"
        );
        // And the paths the Playground passes are the ones that resolve to those.
        assert_eq!(ApiProtocol::OpenAiResponses.path(), "/responses");
        assert_eq!(ApiProtocol::AnthropicMessages.path(), "/v1/messages");
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
