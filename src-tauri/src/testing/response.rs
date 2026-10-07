//! Reading a provider's reply, whatever framing it arrives in.
//!
//! Three callers need to understand a model's answer: the compatibility probes, the
//! streaming probe, and the Playground. They previously each decided for themselves
//! what a valid reply looked like, and they disagreed.
//!
//! The disagreement had a visible cost. A gateway that accepts Anthropic Messages
//! and translates to another API upstream - the gateway does exactly this - answers with
//! `Content-Type: text/event-stream` **even when the request did not ask to stream**,
//! because emitting the translation incrementally is simpler. The compatibility
//! probes were taught to accept that; the Playground was not, and kept running
//! `serde_json::from_str` over an event stream. The same profile would pass 7/7
//! checks and then fail in the Playground with "the provider's reply was not in the
//! format this endpoint should use".
//!
//! That is not a provider problem, it is two parsers disagreeing. This module is the
//! single answer for both: decide by the framing that actually arrived, never by the
//! `stream` flag the caller sent, because the flag is a request and the framing is
//! the fact.
//!
//! Deliberately no translation happens here. The upstream protocol is invisible to
//! the client and is not this program's concern; it only reads what the client-facing
//! endpoint returned.

use serde_json::Value;

/// How a reply was framed on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// A single JSON document.
    Json,
    /// A stream of server-sent events.
    EventStream,
}

impl Framing {
    /// A short label for the interface, which shows how a reply arrived.
    pub fn label(self) -> &'static str {
        match self {
            Framing::Json => "JSON",
            Framing::EventStream => "SSE",
        }
    }
}

/// A model's reply, already reconstructed into what a person should see.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedReply {
    /// The visible assistant text, with streaming deltas already joined.
    pub text: String,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    /// Whether the reply arrived as an event stream.
    pub framing: Option<Framing>,
    /// Set when the stream carried an explicit error event.
    pub stream_error: Option<String>,
}

impl ParsedReply {
    fn json(text: String, input: Option<u32>, output: Option<u32>) -> Self {
        Self {
            text,
            input_tokens: input,
            output_tokens: output,
            framing: Some(Framing::Json),
            stream_error: None,
        }
    }
}

/// Whether a body is a server-sent event stream.
///
/// Decided by shape rather than by `Content-Type`, because the content type is not
/// always available at the point a body is inspected and a gateway is not obliged to
/// set it correctly. A body that opens with an SSE field, or carries one on its own
/// line, is a stream.
pub fn is_event_stream(body: &str) -> bool {
    let trimmed = body.trim_start();
    trimmed.starts_with("event:")
        || trimmed.starts_with("data:")
        || trimmed.starts_with(":")
        || body.contains("\ndata:")
        || body.contains("\nevent:")
}

/// One decoded server-sent event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` name, when the stream supplied one.
    pub name: Option<String>,
    /// The joined `data:` payload.
    pub data: String,
}

/// Split a stream into its events.
///
/// Follows the event-stream rules that matter here: events are separated by a blank
/// line, `data:` lines accumulate within one event joined by newlines, a line
/// beginning with `:` is a comment used as a keep-alive, and an absent `event:` name
/// is legal. Malformed frames are skipped rather than treated as fatal, because a
/// keep-alive or a comment in the middle of a good stream is normal.
pub fn parse_sse(body: &str) -> Vec<SseEvent> {
    let mut events = Vec::new();
    let mut name: Option<String> = None;
    let mut data: Vec<String> = Vec::new();

    let mut flush = |name: &mut Option<String>, data: &mut Vec<String>| {
        if !data.is_empty() {
            events.push(SseEvent {
                name: name.take(),
                data: data.join("\n"),
            });
        } else {
            // An event with a name but no data carries nothing to read.
            *name = None;
        }
        data.clear();
    };

    for line in body.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            flush(&mut name, &mut data);
            continue;
        }
        if line.starts_with(':') {
            // A comment, conventionally used as a keep-alive.
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => name = Some(value.to_string()),
            "data" => data.push(value.to_string()),
            // `id` and `retry` carry no user-visible content.
            _ => {}
        }
    }
    flush(&mut name, &mut data);
    events
}

/// Extract a streamed error message, if the stream reported one.
///
/// An explicit error event is a real answer from the provider, so it has to be
/// surfaced as a provider error rather than parsed as missing content.
pub fn stream_error(events: &[SseEvent]) -> Option<String> {
    for event in events {
        let is_error = event.name.as_deref() == Some("error")
            || serde_json::from_str::<Value>(&event.data)
                .ok()
                .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
                .is_some_and(|t| t == "error");
        if is_error {
            let message = serde_json::from_str::<Value>(&event.data)
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.get("message"))
                        .or_else(|| v.get("message"))
                        .and_then(|m| m.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| event.data.clone());
            return Some(message);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Anthropic Messages
// ---------------------------------------------------------------------------

/// Read an Anthropic Messages reply in either framing.
pub fn parse_anthropic(body: &str) -> ParsedReply {
    if is_event_stream(body) {
        return parse_anthropic_stream(&parse_sse(body));
    }
    match serde_json::from_str::<Value>(body) {
        Ok(value) => {
            let text = value
                .get("content")
                .and_then(Value::as_array)
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|block| block.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            let usage = value.get("usage");
            ParsedReply::json(
                text,
                usage
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32),
                usage
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32),
            )
        }
        // Not JSON and not a stream: the caller turns this into a readable error.
        Err(_) => ParsedReply::default(),
    }
}

/// Reconstruct the assistant text from Anthropic event-stream events.
///
/// The visible text lives in `content_block_delta` events with a `text_delta`
/// payload. Everything else that the gateway sends - `message_start`,
/// `content_block_start`, `content_block_stop`, `message_delta`, `message_stop`, and
/// `ping` - is metadata that must be accepted without contributing text. Treating
/// those as malformed is what made a working reply look broken.
fn parse_anthropic_stream(events: &[SseEvent]) -> ParsedReply {
    let mut text = String::new();
    let mut input_tokens = None;
    let mut output_tokens = None;

    for event in events {
        let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
            // A frame we cannot read is skipped; the rest of the stream is still
            // useful, and a keep-alive or an unknown future event is not an error.
            continue;
        };
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "content_block_delta" => {
                // `text_delta` carries visible text. `input_json_delta` carries a
                // tool's arguments, which is not assistant prose.
                if let Some(delta) = value.get("delta") {
                    if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
                        if let Some(chunk) = delta.get("text").and_then(Value::as_str) {
                            text.push_str(chunk);
                        }
                    }
                }
            }
            "message_start" => {
                if let Some(usage) = value.get("message").and_then(|m| m.get("usage")) {
                    input_tokens = usage
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .map(|v| v as u32);
                    output_tokens = usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .map(|v| v as u32);
                }
            }
            "message_delta" => {
                if let Some(usage) = value.get("usage") {
                    if let Some(out) = usage.get("output_tokens").and_then(Value::as_u64) {
                        output_tokens = Some(out as u32);
                    }
                }
            }
            // `content_block_start`, `content_block_stop`, `message_stop`, `ping`:
            // valid events with nothing a user should read.
            _ => {}
        }
    }

    ParsedReply {
        text,
        input_tokens,
        output_tokens,
        framing: Some(Framing::EventStream),
        stream_error: stream_error(events),
    }
}

// ---------------------------------------------------------------------------
// OpenAI Responses
// ---------------------------------------------------------------------------

/// Read a Responses reply in either framing.
pub fn parse_responses(body: &str) -> ParsedReply {
    if is_event_stream(body) {
        return parse_responses_stream(&parse_sse(body));
    }
    match serde_json::from_str::<Value>(body) {
        Ok(value) => {
            let convenience = value
                .get("output_text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let text = if !convenience.trim().is_empty() {
                convenience
            } else {
                value
                    .get("output")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .flat_map(|item| {
                                item.get("content")
                                    .and_then(Value::as_array)
                                    .map(Vec::as_slice)
                                    .unwrap_or(&[])
                            })
                            .filter_map(|part| part.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .unwrap_or_default()
            };
            let usage = value.get("usage");
            ParsedReply::json(
                text,
                usage
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32),
                usage
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32),
            )
        }
        Err(_) => ParsedReply::default(),
    }
}

/// Reconstruct text from Responses event-stream events.
fn parse_responses_stream(events: &[SseEvent]) -> ParsedReply {
    let mut text = String::new();
    let mut input_tokens = None;
    let mut output_tokens = None;

    for event in events {
        let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
            continue;
        };
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            // Incremental text, which is what a streamed answer uses.
            "response.output_text.delta" => {
                if let Some(chunk) = value.get("delta").and_then(Value::as_str) {
                    text.push_str(chunk);
                }
            }
            // A completed message item, for a gateway that sends whole parts.
            "response.output_item.done" => {
                if let Some(item) = value.get("item") {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        for part in parts {
                            if let Some(t) = part.get("text").and_then(Value::as_str) {
                                text.push_str(t);
                            }
                        }
                    }
                }
            }
            "response.completed" => {
                if let Some(usage) = value.get("response").and_then(|r| r.get("usage")) {
                    input_tokens = usage
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .map(|v| v as u32);
                    output_tokens = usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .map(|v| v as u32);
                }
            }
            _ => {}
        }
    }

    ParsedReply {
        text,
        input_tokens,
        output_tokens,
        framing: Some(Framing::EventStream),
        stream_error: stream_error(events),
    }
}

// ---------------------------------------------------------------------------
// OpenAI Chat Completions
// ---------------------------------------------------------------------------

/// Read a Chat Completions reply in either framing.
pub fn parse_chat(body: &str) -> ParsedReply {
    if is_event_stream(body) {
        return parse_chat_stream(&parse_sse(body));
    }
    match serde_json::from_str::<Value>(body) {
        Ok(value) => {
            let text = value
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("message"))
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let usage = value.get("usage");
            ParsedReply::json(
                text,
                usage
                    .and_then(|u| u.get("prompt_tokens"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32),
                usage
                    .and_then(|u| u.get("completion_tokens"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32),
            )
        }
        Err(_) => ParsedReply::default(),
    }
}

/// Reconstruct text from Chat Completions event-stream events.
fn parse_chat_stream(events: &[SseEvent]) -> ParsedReply {
    let mut text = String::new();
    let mut input_tokens = None;
    let mut output_tokens = None;

    for event in events {
        // `[DONE]` is the terminator, not a payload.
        if event.data.trim() == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
            continue;
        };
        if let Some(delta) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
            .and_then(|c| c.get("delta"))
        {
            if let Some(chunk) = delta.get("content").and_then(Value::as_str) {
                text.push_str(chunk);
            }
        }
        if let Some(usage) = value.get("usage") {
            input_tokens = usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .map(|v| v as u32)
                .or(input_tokens);
            output_tokens = usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .map(|v| v as u32)
                .or(output_tokens);
        }
    }

    ParsedReply {
        text,
        input_tokens,
        output_tokens,
        framing: Some(Framing::EventStream),
        stream_error: stream_error(events),
    }
}

/// Read a reply for `protocol`, in whichever framing arrived.
pub fn parse_for(protocol: crate::profiles::ApiProtocol, body: &str) -> ParsedReply {
    match protocol {
        crate::profiles::ApiProtocol::AnthropicMessages => parse_anthropic(body),
        crate::profiles::ApiProtocol::OpenAiResponses => parse_responses(body),
        crate::profiles::ApiProtocol::OpenAiChatCompletions => parse_chat(body),
    }
}

/// Whether a body is a reply this module could make sense of at all.
///
/// Used by a probe to distinguish "the endpoint answered something unrelated" from
/// "the endpoint answered a stream I understand". Neither is JSON, but only one of
/// them means the protocol is supported.
pub fn looks_like_a_reply(protocol: crate::profiles::ApiProtocol, body: &str) -> bool {
    if is_event_stream(body) {
        let events = parse_sse(body);
        if events.is_empty() {
            return false;
        }
        // A stream has to name the protocol somewhere to count.
        let joined = body.to_ascii_lowercase();
        return match protocol {
            crate::profiles::ApiProtocol::AnthropicMessages => {
                joined.contains("message_start")
                    || joined.contains("content_block")
                    || joined.contains("\"type\":\"message\"")
            }
            crate::profiles::ApiProtocol::OpenAiResponses => {
                joined.contains("response.created")
                    || joined.contains("response.output")
                    || joined.contains("response.completed")
                    || joined.contains("\"object\":\"response\"")
            }
            crate::profiles::ApiProtocol::OpenAiChatCompletions => {
                joined.contains("chat.completion") || joined.contains("\"choices\"")
            }
        };
    }
    serde_json::from_str::<Value>(body).is_ok()
}
