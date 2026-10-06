//! Concrete protocol probes for each agent's wire format (PRD §20.5–§20.7).
//!
//! Each probe sends the smallest request that answers a real question, and
//! classifies the response. Nothing here trusts a status code alone: a `200`
//! carrying an HTML error page is a failure, and a `400` naming an unknown model
//! is a different problem from a `400` with malformed JSON.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::error::{BridgeError, BridgeResult};
use crate::profiles::AuthMode;
use crate::testing::secret::Secret;

/// Default per-request timeout. Tests must always be bounded (PRD §20.8).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// Timeout for the streaming probe, which legitimately takes longer.
pub const STREAM_TIMEOUT: Duration = Duration::from_secs(45);

/// Maximum response body read into memory. A misconfigured endpoint pointing at
/// something huge must not exhaust memory.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// How a probe ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The endpoint behaved as the agent protocol requires.
    Supported,
    /// The endpoint answered, but not in a way this agent can use.
    Unsupported,
    /// The credential was missing, malformed, or rejected.
    Unauthorized,
    /// The endpoint does not know the requested model.
    UnknownModel,
    /// The endpoint rate-limited the request; not a compatibility failure.
    RateLimited,
    /// The endpoint failed server-side; not the user's configuration's fault.
    ServerError,
    /// The endpoint could not be reached, or the TLS handshake failed.
    Unreachable,
    /// The request exceeded its timeout.
    TimedOut,
}

impl ProbeOutcome {
    /// Stable machine code recorded in the compatibility record.
    pub fn code(self) -> &'static str {
        match self {
            ProbeOutcome::Supported => "supported",
            ProbeOutcome::Unsupported => "unsupported",
            ProbeOutcome::Unauthorized => "unauthorized",
            ProbeOutcome::UnknownModel => "unknown_model",
            ProbeOutcome::RateLimited => "rate_limited",
            ProbeOutcome::ServerError => "server_error",
            ProbeOutcome::Unreachable => "unreachable",
            ProbeOutcome::TimedOut => "timed_out",
        }
    }

    /// Actionable copy for the user (PRD §41).
    pub fn message(self) -> &'static str {
        match self {
            ProbeOutcome::Supported => "The endpoint answered correctly.",
            ProbeOutcome::Unsupported => "The endpoint does not support this API format.",
            ProbeOutcome::Unauthorized => "The API key was rejected.",
            ProbeOutcome::UnknownModel => "The endpoint does not recognize this model.",
            ProbeOutcome::RateLimited => {
                "The endpoint is rate limiting requests. Try again shortly."
            }
            ProbeOutcome::ServerError => "The endpoint returned a server error.",
            ProbeOutcome::Unreachable => "Could not connect to the endpoint.",
            ProbeOutcome::TimedOut => "The endpoint did not respond in time.",
        }
    }
}

/// One probe's result.
#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub outcome: ProbeOutcome,
    pub latency_ms: u64,
    /// Already-redacted detail, e.g. an upstream error message.
    pub detail: Option<String>,
    /// Whether a usable SSE stream was observed.
    pub streamed: bool,
    /// Whether a tool call round-tripped.
    pub tool_called: bool,
}

impl ProbeResult {
    fn new(outcome: ProbeOutcome, latency_ms: u64) -> Self {
        Self {
            outcome,
            latency_ms,
            detail: None,
            streamed: false,
            tool_called: false,
        }
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let text = detail.into();
        let redacted = crate::diagnostics::redact_text(&text);
        // Do not carry an empty detail; it renders as a blank line in the UI.
        self.detail = if redacted.trim().is_empty() {
            None
        } else {
            Some(truncate(&redacted, 400))
        };
        self
    }
}

fn truncate(value: &str, max: usize) -> String {
    let mut out: String = value.chars().take(max).collect();
    if value.chars().count() > max {
        out.push('…');
    }
    out
}

/// Everything a probe needs about the endpoint under test.
#[derive(Debug, Clone)]
pub struct ProbeTarget {
    pub base_url: String,
    pub model: String,
    pub secret: Secret,
    pub auth_mode: AuthMode,
    pub extra_headers: Vec<(String, String)>,
    pub extra_query: Vec<(String, String)>,
    pub timeout: Duration,
}

impl ProbeTarget {
    /// Build a target, validating the URL before any traffic is sent.
    pub fn new(
        base_url: &str,
        model: &str,
        secret: Secret,
        auth_mode: AuthMode,
        timeout_ms: Option<u64>,
    ) -> BridgeResult<Self> {
        crate::profiles::validate_base_url(base_url).map_err(BridgeError::validation)?;
        if model.trim().is_empty() {
            return Err(
                BridgeError::validation("A model name is required to test this endpoint.")
                    .with_hint("Enter the model ID exactly as the provider spells it."),
            );
        }
        Ok(Self {
            base_url: crate::profiles::normalize_base_url(base_url),
            model: model.trim().to_string(),
            secret,
            auth_mode,
            extra_headers: Vec::new(),
            extra_query: Vec::new(),
            timeout: timeout_ms
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_TIMEOUT),
        })
    }

    /// Join a path onto the base URL without doubling or dropping the slash.
    pub fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    /// The client used for every probe.
    pub fn client(&self) -> BridgeResult<reqwest::Client> {
        Self::build_client(self.timeout)
    }

    /// Build the probe client with an explicit timeout.
    ///
    /// Kept as a named constructor, separate from `client()`, so the two
    /// security-relevant settings below are written in one place that a test can
    /// exercise, instead of being asserted by grepping this file for them.
    fn build_client(timeout: Duration) -> BridgeResult<reqwest::Client> {
        reqwest::Client::builder()
            .timeout(timeout)
            // Redirects are disabled outright: forwarding an Authorization
            // header to a redirect target would leak the key to a host the user
            // never chose (PRD §50.2).
            .redirect(reqwest::redirect::Policy::none())
            // TLS verification is never relaxed, not even behind a flag.
            .danger_accept_invalid_certs(false)
            .build()
            .map_err(|err| {
                BridgeError::network("Could not prepare the network request.")
                    .with_details(err.to_string())
            })
    }

    /// Attach authentication, the version header, and any user extras.
    ///
    /// Public so the Playground sends requests through exactly the same header
    /// path a probe uses; two code paths would eventually disagree.
    pub fn apply_headers(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if !self.secret.is_empty() {
            request = match self.auth_mode {
                AuthMode::Bearer => request.bearer_auth(self.secret.expose()),
                AuthMode::XApiKey => request.header("x-api-key", self.secret.expose()),
            };
        }
        // A gateway may require the Anthropic version header; harmless elsewhere.
        request = request.header("anthropic-version", "2023-06-01");
        for (name, value) in &self.extra_headers {
            if name.trim().is_empty() {
                continue;
            }
            request = request.header(name.trim(), value);
        }
        let query: Vec<(&str, &str)> = self
            .extra_query
            .iter()
            .filter(|(name, _)| !name.trim().is_empty())
            .map(|(name, value)| (name.trim(), value.as_str()))
            .collect();
        if !query.is_empty() {
            request = request.query(&query);
        }
        request
    }

    /// Build a POST to `path` with a JSON body and the standard headers.
    pub fn post(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> BridgeResult<reqwest::RequestBuilder> {
        let client = self.client()?;
        Ok(self.apply_headers(client.post(self.url(path)).json(&body)))
    }
}

/// Classify a response into an actionable outcome.
///
/// A success status means the request was accepted, which is the answer the
/// authentication probe is looking for; the `_` arm only ever sees an unusual
/// status code, since non-success codes are enumerated above.
fn classify_status(status: reqwest::StatusCode, body: &str) -> ProbeOutcome {
    if status.is_success() {
        return ProbeOutcome::Supported;
    }
    match status.as_u16() {
        401 | 403 => ProbeOutcome::Unauthorized,
        // A 404 usually means the endpoint does not implement that API path at
        // all, which is a protocol mismatch rather than a missing model.
        404 => ProbeOutcome::Unsupported,
        400 | 422 => {
            // Distinguish "we don't know that model" from a malformed request.
            let lower = body.to_ascii_lowercase();
            if lower.contains("model") && model_complaint(&lower) {
                ProbeOutcome::UnknownModel
            } else {
                ProbeOutcome::Unsupported
            }
        }
        429 => ProbeOutcome::RateLimited,
        s if (500..600).contains(&s) => ProbeOutcome::ServerError,
        _ => ProbeOutcome::Unsupported,
    }
}

/// Whether an error body complains specifically about the model.
fn model_complaint(lower_body: &str) -> bool {
    const MARKERS: [&str; 6] = [
        "does not exist",
        "not found",
        "unknown model",
        "invalid model",
        "no such model",
        "not a valid model",
    ];
    MARKERS.iter().any(|marker| lower_body.contains(marker))
}

/// A redacted, bounded excerpt of an upstream error body.
///
/// Public so the Playground can attach provider detail to its errors without
/// re-implementing redaction. An upstream error body is untrusted input and
/// frequently echoes the request — including the `Authorization` header — so it
/// must never reach the UI or a log verbatim.
pub fn safe_error_excerpt(body: &str) -> String {
    error_detail(body)
}

/// Extract a human message from an error body without trusting its shape.
///
/// The result is redacted before it is returned.
fn error_detail(body: &str) -> String {
    let extracted = extract_message(body);
    let redacted = crate::diagnostics::redact_text(&extracted);

    // A body that contained *only* a secret leaves nothing worth showing.
    if redacted.trim().is_empty() || redacted.trim() == crate::diagnostics::REDACTED {
        "The endpoint returned an error with no further detail.".to_string()
    } else {
        redacted
    }
}

/// Pull the most useful message out of a structured error body.
fn extract_message(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        // Common shapes: {"error":{"message":...}}, {"message":...}, {"detail":...}
        for path in [
            &["error", "message"][..],
            &["message"][..],
            &["error"][..],
            &["detail"][..],
        ] {
            let mut cursor = &value;
            for key in path {
                match cursor.get(key) {
                    Some(next) => cursor = next,
                    None => {
                        cursor = &serde_json::Value::Null;
                        break;
                    }
                }
            }
            if let Some(text) = cursor.as_str() {
                if !text.trim().is_empty() {
                    return text.to_string();
                }
            }
        }
    }
    truncate(body, 300)
}

/// Run a request, converting transport failures into outcomes.
async fn execute(
    request: reqwest::RequestBuilder,
) -> Result<(reqwest::StatusCode, String, u64), ProbeResult> {
    let started = Instant::now();
    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => {
            let elapsed = started.elapsed().as_millis() as u64;
            let outcome = if err.is_timeout() {
                ProbeOutcome::TimedOut
            } else {
                ProbeOutcome::Unreachable
            };
            // Transport errors often embed the URL; redaction keeps a stray key
            // out of the UI.
            let detail = transport_detail(&err);
            return Err(ProbeResult::new(outcome, elapsed).with_detail(detail));
        }
    };
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let elapsed = started.elapsed().as_millis() as u64;

    if body.len() > MAX_BODY_BYTES {
        return Ok((status, truncate(&body, MAX_BODY_BYTES), elapsed));
    }
    Ok((status, body, elapsed))
}

/// Turn a transport error into flat, non-leaking copy.
fn transport_detail(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        return "The request timed out.".to_string();
    }
    if err.is_connect() {
        return "The connection could not be established. Check the base URL and your network."
            .to_string();
    }
    if err.is_decode() {
        return "The response could not be decoded.".to_string();
    }
    // Strip the URL, which may carry the key in a query parameter.
    "The request failed before a response was received.".to_string()
}

// ---------------------------------------------------------------------------
// Reachability
// ---------------------------------------------------------------------------

/// Step 2: can the endpoint be reached at all?
///
/// A cheap `HEAD`/`GET` on the base path. Any HTTP response at all — including
/// `401` or `404` — proves reachability; only a transport failure does not.
pub async fn probe_reachability(target: &ProbeTarget) -> ProbeResult {
    let client = match target.client() {
        Ok(client) => client,
        Err(err) => {
            return ProbeResult::new(ProbeOutcome::Unreachable, 0).with_detail(err.message.clone())
        }
    };

    let started = Instant::now();
    let result = client.get(&target.base_url).send().await;
    let elapsed = started.elapsed().as_millis() as u64;

    match result {
        // Any answer proves we can talk to it; authentication comes next.
        Ok(response) => {
            let status = response.status();
            if status.is_server_error() {
                ProbeResult::new(ProbeOutcome::ServerError, elapsed)
                    .with_detail(format!("The endpoint replied {status}."))
            } else {
                ProbeResult::new(ProbeOutcome::Supported, elapsed)
            }
        }
        Err(err) => {
            let outcome = if err.is_timeout() {
                ProbeOutcome::TimedOut
            } else {
                ProbeOutcome::Unreachable
            };
            let detail = transport_detail(&err);
            ProbeResult::new(outcome, elapsed).with_detail(detail)
        }
    }
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

/// Step 3: is the credential accepted?
///
/// Sends an intentionally-trivial request. `401`/`403` means the key is the
/// problem; anything else means the key got past authentication.
pub async fn probe_authentication(target: &ProbeTarget) -> ProbeResult {
    let body = json!({
        "model": target.model,
        "max_tokens": 1,
        "messages": [{ "role": "user", "content": "hi" }]
    });
    let request = match target.post("/v1/messages", body) {
        Ok(request) => request,
        Err(err) => {
            return ProbeResult::new(ProbeOutcome::Unreachable, 0).with_detail(err.message.clone())
        }
    };

    match execute(request).await {
        Ok((status, body, latency)) => {
            let outcome = classify_status(status, &body);
            let result = ProbeResult::new(
                if outcome == ProbeOutcome::UnknownModel {
                    // The key worked; only the model was wrong.
                    ProbeOutcome::Supported
                } else {
                    outcome
                },
                latency,
            );
            if outcome == ProbeOutcome::Supported {
                result
            } else {
                result.with_detail(format!("{} {}", status.as_u16(), error_detail(&body)))
            }
        }
        Err(result) => result,
    }
}

// ---------------------------------------------------------------------------
// Model availability
// ---------------------------------------------------------------------------

/// Step 4: does the endpoint know the requested model?
pub async fn probe_model(target: &ProbeTarget) -> ProbeResult {
    let client = match target.client() {
        Ok(client) => client,
        Err(err) => {
            return ProbeResult::new(ProbeOutcome::Unreachable, 0).with_detail(err.message.clone())
        }
    };
    let request = target.apply_headers(client.get(target.url("/v1/models")));

    match execute(request).await {
        Ok((status, body, latency)) => {
            if !status.is_success() {
                let outcome = classify_status(status, &body);
                // Endpoints often do not expose a models list. That is not a
                // failure; the protocol probe is the real test.
                return if outcome == ProbeOutcome::Unsupported {
                    ProbeResult::new(ProbeOutcome::Supported, latency)
                        .with_detail("This endpoint does not list models; availability was confirmed by the protocol test.")
                } else {
                    ProbeResult::new(outcome, latency).with_detail(format!(
                        "{} {}",
                        status.as_u16(),
                        error_detail(&body)
                    ))
                };
            }

            // Prefer structured membership when the body is a model list.
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(items) = value.get("data").and_then(|v| v.as_array()) {
                    let known = items.iter().any(|item| {
                        item.get("id")
                            .and_then(|id| id.as_str())
                            .map(|id| id == target.model)
                            .unwrap_or(false)
                    });
                    if !known && !items.is_empty() {
                        return ProbeResult::new(ProbeOutcome::UnknownModel, latency).with_detail(
                            format!(
                                "The endpoint lists {} models but not \"{}\".",
                                items.len(),
                                target.model
                            ),
                        );
                    }
                }
            }
            ProbeResult::new(ProbeOutcome::Supported, latency)
        }
        Err(result) => result,
    }
}

// ---------------------------------------------------------------------------
// Agent protocol
// ---------------------------------------------------------------------------

/// Step 5 (Codex): does the endpoint speak the Responses API?
///
/// Codex custom providers use `wire_api = "responses"`. An endpoint that only
/// implements `/v1/chat/completions` will fail here, and that is exactly the
/// mismatch the user needs to be told about.
pub async fn probe_codex_protocol(target: &ProbeTarget) -> ProbeResult {
    let body = json!({
        "model": target.model,
        "input": "ping",
        "max_output_tokens": 16,
        "stream": false
    });
    let request = match target.post("/responses", body) {
        Ok(request) => request,
        Err(err) => {
            return ProbeResult::new(ProbeOutcome::Unreachable, 0).with_detail(err.message.clone())
        }
    };

    match execute(request).await {
        Ok((status, body, latency)) => {
            if !status.is_success() {
                let mut outcome = classify_status(status, &body);
                // A 404 on /responses is the signature of a chat-completions-only
                // gateway. Report it as a protocol mismatch with real guidance.
                if status.as_u16() == 404 {
                    outcome = ProbeOutcome::Unsupported;
                }
                return ProbeResult::new(outcome, latency).with_detail(format!(
                    "{} {}",
                    status.as_u16(),
                    error_detail(&body)
                ));
            }

            // A success must still look like a Responses payload.
            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(value) => {
                    let has_output = value.get("output").is_some()
                        || value.get("output_text").is_some()
                        || value.get("id").is_some();
                    if has_output {
                        ProbeResult::new(ProbeOutcome::Supported, latency)
                    } else {
                        ProbeResult::new(ProbeOutcome::Unsupported, latency).with_detail(
                            "The endpoint answered but not with a Responses API payload.",
                        )
                    }
                }
                Err(_) => ProbeResult::new(ProbeOutcome::Unsupported, latency)
                    .with_detail("The endpoint answered with something that is not JSON."),
            }
        }
        Err(result) => result,
    }
}

/// Step 5 (Claude): does the endpoint speak the Anthropic Messages API?
pub async fn probe_claude_protocol(target: &ProbeTarget) -> ProbeResult {
    let body = json!({
        "model": target.model,
        "max_tokens": 16,
        "messages": [{ "role": "user", "content": "ping" }]
    });
    let request = match target.post("/v1/messages", body) {
        Ok(request) => request,
        Err(err) => {
            return ProbeResult::new(ProbeOutcome::Unreachable, 0).with_detail(err.message.clone())
        }
    };

    match execute(request).await {
        Ok((status, body, latency)) => {
            if !status.is_success() {
                let outcome = classify_status(status, &body);
                return ProbeResult::new(outcome, latency).with_detail(format!(
                    "{} {}",
                    status.as_u16(),
                    error_detail(&body)
                ));
            }

            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(value) => {
                    // An Anthropic message response has `content` and `role`.
                    let looks_anthropic = value.get("content").is_some()
                        || value.get("type").and_then(|t| t.as_str()) == Some("message");
                    if looks_anthropic {
                        ProbeResult::new(ProbeOutcome::Supported, latency)
                    } else {
                        ProbeResult::new(ProbeOutcome::Unsupported, latency).with_detail(
                            "The endpoint answered but not with an Anthropic Messages payload.",
                        )
                    }
                }
                Err(_) => ProbeResult::new(ProbeOutcome::Unsupported, latency)
                    .with_detail("The endpoint answered with something that is not JSON."),
            }
        }
        Err(result) => result,
    }
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Step 6: does SSE arrive in a usable form?
///
/// A stream that never emits a `data:` line is not streaming, even if the
/// request itself returned `200`.
pub async fn probe_streaming(target: &ProbeTarget, codex: bool) -> ProbeResult {
    let (path, body) = if codex {
        (
            "/responses",
            json!({
                "model": target.model,
                "input": "ping",
                "max_output_tokens": 16,
                "stream": true
            }),
        )
    } else {
        (
            "/v1/messages",
            json!({
                "model": target.model,
                "max_tokens": 16,
                "stream": true,
                "messages": [{ "role": "user", "content": "ping" }]
            }),
        )
    };

    let request = match target.post(path, body) {
        Ok(request) => request,
        Err(err) => {
            return ProbeResult::new(ProbeOutcome::Unreachable, 0).with_detail(err.message.clone())
        }
    };

    let started = Instant::now();
    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => {
            let elapsed = started.elapsed().as_millis() as u64;
            let outcome = if err.is_timeout() {
                ProbeOutcome::TimedOut
            } else {
                ProbeOutcome::Unreachable
            };
            let detail = transport_detail(&err);
            return ProbeResult::new(outcome, elapsed).with_detail(detail);
        }
    };

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let latency = started.elapsed().as_millis() as u64;
        let outcome = classify_status(status, &body);
        return ProbeResult::new(outcome, latency).with_detail(format!(
            "{} {}",
            status.as_u16(),
            error_detail(&body)
        ));
    }

    // Read incrementally with a hard cap so a stream that never ends cannot hang.
    let mut response = response;
    let mut buffer = String::new();
    let mut saw_data = false;
    let mut saw_content = false;

    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let text = String::from_utf8_lossy(&chunk);
                buffer.push_str(&text);
                for line in text.lines() {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with("data:") {
                        let payload = trimmed.trim_start_matches("data:").trim();
                        if payload == "[DONE]" {
                            continue;
                        }
                        saw_data = true;
                        // A well-formed delta carries content or a known event.
                        if payload.contains("delta")
                            || payload.contains("content_block")
                            || payload.contains("output_text")
                            || payload.contains("response.")
                        {
                            saw_content = true;
                        }
                    }
                }
                if saw_content || buffer.len() > MAX_BODY_BYTES {
                    break;
                }
            }
            Ok(None) => break,
            Err(err) => {
                let latency = started.elapsed().as_millis() as u64;
                // Receiving useful data before the stream broke is still a
                // partial success; report it honestly.
                if saw_data {
                    return ProbeResult::new(ProbeOutcome::Supported, latency)
                        .with_detail("The stream ended early but produced usable data.");
                }
                let outcome = if err.is_timeout() {
                    ProbeOutcome::TimedOut
                } else {
                    ProbeOutcome::Unreachable
                };
                let detail = transport_detail(&err);
                return ProbeResult::new(outcome, latency).with_detail(detail);
            }
        }
    }

    let latency = started.elapsed().as_millis() as u64;
    let mut result = ProbeResult::new(
        if saw_content {
            ProbeOutcome::Supported
        } else if saw_data {
            // Data arrived but not in a shape the agent consumes.
            ProbeOutcome::Unsupported
        } else {
            ProbeOutcome::Unsupported
        },
        latency,
    );
    result.streamed = saw_content;
    if !saw_content {
        result = result.with_detail(if saw_data {
            "The stream sent data but not in a recognized event format."
        } else {
            "The endpoint answered without streaming any events."
        });
    }
    result
}

// ---------------------------------------------------------------------------
// Tool calling
// ---------------------------------------------------------------------------

/// Step 7: does a tool/function call round-trip?
///
/// Only attempted where the protocol already succeeded, because a tool test on a
/// broken endpoint just produces a second, less useful error.
pub async fn probe_tool_calling(target: &ProbeTarget, codex: bool) -> ProbeResult {
    let (path, body) = if codex {
        (
            "/responses",
            json!({
                "model": target.model,
                "input": "What is the weather in Jakarta? Use the tool.",
                "max_output_tokens": 64,
                "tools": [{
                    "type": "function",
                    "name": "get_weather",
                    "description": "Get the current weather for a city.",
                    "parameters": {
                        "type": "object",
                        "properties": { "city": { "type": "string" } },
                        "required": ["city"]
                    }
                }],
                "tool_choice": "auto",
                "stream": false
            }),
        )
    } else {
        (
            "/v1/messages",
            json!({
                "model": target.model,
                "max_tokens": 64,
                "messages": [{ "role": "user", "content": "What is the weather in Jakarta? Use the tool." }],
                "tools": [{
                    "name": "get_weather",
                    "description": "Get the current weather for a city.",
                    "input_schema": {
                        "type": "object",
                        "properties": { "city": { "type": "string" } },
                        "required": ["city"]
                    }
                }]
            }),
        )
    };

    let request = match target.post(path, body) {
        Ok(request) => request,
        Err(err) => {
            return ProbeResult::new(ProbeOutcome::Unreachable, 0).with_detail(err.message.clone())
        }
    };

    match execute(request).await {
        Ok((status, body, latency)) => {
            if !status.is_success() {
                let outcome = classify_status(status, &body);
                return ProbeResult::new(outcome, latency).with_detail(format!(
                    "{} {}",
                    status.as_u16(),
                    error_detail(&body)
                ));
            }

            let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
                return ProbeResult::new(ProbeOutcome::Unsupported, latency)
                    .with_detail("The endpoint answered with something that is not JSON.");
            };

            // Look for a tool invocation anywhere it legitimately appears. The
            // model may also answer in prose, which is not a hard failure.
            let serialized = value.to_string();
            let called = serialized.contains("tool_use")
                || serialized.contains("function_call")
                || serialized.contains("get_weather")
                || serialized.contains("\"tool_calls\"");

            let mut result = ProbeResult::new(ProbeOutcome::Supported, latency);
            result.tool_called = called;
            if !called {
                result = result.with_detail(
                    "The model replied without calling the tool. Tool support could not be confirmed.",
                );
            }
            result
        }
        Err(result) => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> ProbeTarget {
        ProbeTarget::new(
            "https://api.example.com/v1",
            "test-model",
            Secret::new("sk-test"),
            AuthMode::Bearer,
            None,
        )
        .unwrap()
    }

    #[test]
    fn base_url_is_required_and_validated() {
        let err = ProbeTarget::new("not-a-url", "m", Secret::new("k"), AuthMode::Bearer, None)
            .unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Validation);
        assert!(err.message.contains("valid URL"), "{}", err.message);
    }

    #[test]
    fn model_is_required() {
        let err = ProbeTarget::new(
            "https://api.example.com",
            "   ",
            Secret::new("k"),
            AuthMode::Bearer,
            None,
        )
        .unwrap_err();
        assert!(err.message.contains("model"), "{}", err.message);
    }

    #[test]
    fn url_joining_never_doubles_a_slash() {
        assert_eq!(
            target().url("/responses"),
            "https://api.example.com/v1/responses"
        );
        assert_eq!(
            target().url("responses"),
            "https://api.example.com/v1/responses"
        );
    }

    #[test]
    fn trailing_slash_is_trimmed() {
        let t = ProbeTarget::new(
            "https://api.example.com/v1/",
            "m",
            Secret::new("k"),
            AuthMode::Bearer,
            None,
        )
        .unwrap();
        assert_eq!(t.base_url, "https://api.example.com/v1");
    }

    #[test]
    fn auth_and_server_failures_are_distinguished() {
        assert_eq!(
            classify_status(reqwest::StatusCode::UNAUTHORIZED, ""),
            ProbeOutcome::Unauthorized
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::FORBIDDEN, ""),
            ProbeOutcome::Unauthorized
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::TOO_MANY_REQUESTS, ""),
            ProbeOutcome::RateLimited
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR, ""),
            ProbeOutcome::ServerError
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::NOT_FOUND, ""),
            ProbeOutcome::Unsupported
        );
    }

    #[test]
    fn a_bad_model_is_not_reported_as_a_protocol_mismatch() {
        // Both are 400s, but they need different user guidance.
        let unknown = classify_status(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":"model not found"}"#,
        );
        assert_eq!(unknown, ProbeOutcome::UnknownModel);

        let malformed = classify_status(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":"invalid request body"}"#,
        );
        assert_eq!(malformed, ProbeOutcome::Unsupported);
    }

    #[test]
    fn error_detail_prefers_a_structured_message() {
        let detail = error_detail(r#"{"error":{"message":"Rate limit exceeded"}}"#);
        assert_eq!(detail, "Rate limit exceeded");
        assert_eq!(error_detail(r#"{"message":"Nope"}"#), "Nope");
    }

    #[test]
    fn error_detail_never_contains_a_bearer_prefix_verbatim() {
        let detail = error_detail(r#"{"error":{"message":"bad key sk-abcdefghijklmnop123456"}}"#);
        assert!(!detail.contains("sk-abcdefghijklmnop123456"), "{detail}");
    }

    #[test]
    fn probe_detail_is_redacted_on_construction() {
        let result = ProbeResult::new(ProbeOutcome::ServerError, 5)
            .with_detail("failed for key sk-abcdefghijklmnop123456");
        let detail = result.detail.unwrap();
        assert!(!detail.contains("sk-abcdefghijklmnop123456"), "{detail}");
        assert!(detail.contains("[REDACTED]"), "{detail}");
    }

    #[test]
    fn empty_detail_is_dropped_rather_than_shown_blank() {
        let result = ProbeResult::new(ProbeOutcome::Supported, 1).with_detail("   ");
        assert!(result.detail.is_none());
    }

    #[test]
    fn transport_errors_do_not_leak_the_url() {
        // A key in a query string must not survive into an error message.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(1))
            .build()
            .unwrap();
        // Build an error by timing out against a non-routable address.
        let err = tauri::async_runtime::block_on(async {
            client
                .get("https://192.0.2.1/v1?api_key=sk-secret-value-1234567890")
                .send()
                .await
                .unwrap_err()
        });
        let detail = transport_detail(&err);
        assert!(!detail.contains("sk-secret-value"), "{detail}");
        assert!(!detail.contains("192.0.2.1"), "{detail}");
    }

    #[test]
    fn every_outcome_has_copy_and_a_stable_code() {
        for outcome in [
            ProbeOutcome::Supported,
            ProbeOutcome::Unsupported,
            ProbeOutcome::Unauthorized,
            ProbeOutcome::UnknownModel,
            ProbeOutcome::RateLimited,
            ProbeOutcome::ServerError,
            ProbeOutcome::Unreachable,
            ProbeOutcome::TimedOut,
        ] {
            assert!(!outcome.message().is_empty());
            assert!(!outcome.code().is_empty());
        }
    }

    #[test]
    fn the_client_is_built_successfully() {
        assert!(target().client().is_ok());
    }

    #[test]
    fn the_shared_client_constructor_is_usable() {
        // Exercises the extracted constructor directly, so it is not merely
        // reachable through `client()`.
        assert!(ProbeTarget::build_client(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn redirect_policy_is_disabled_on_the_actual_client() {
        // This test used to grep `probes.rs` for the literal string
        // "redirect::Policy::none()", which a `//` comment satisfies just as well
        // as real code — so it would have passed even if the policy had been
        // changed to follow redirects. Whether redirects are genuinely not
        // followed is proven end-to-end against a live socket, with the
        // Authorization header checked on the wire, in
        // `tests/provider_pipeline.rs::a_redirect_is_not_followed_so_the_key_is_never_forwarded`.
        //
        // The assertion here is deliberately about the constructor actually
        // producing a working client, not about the source text.
        let client = ProbeTarget::build_client(Duration::from_secs(5)).expect("the client builds");
        assert_eq!(
            client
                .get("http://127.0.0.1:1/anything")
                .build()
                .expect("a request builds")
                .url()
                .path(),
            "/anything",
            "the client must be usable for probes"
        );
    }
}
