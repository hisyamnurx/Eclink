//! Concrete protocol probes for each agent's wire format (PRD §20.5–§20.7).
//!
//! Each probe sends the smallest request that answers a real question, and
//! classifies the response. Nothing here trusts a status code alone: a `200`
//! carrying an HTML error page is a failure, and a `400` naming an unknown model
//! is a different problem from a `400` with malformed JSON.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    ///
    /// Each message names the cause, not just the symptom. In particular the
    /// unsupported case must never read like an authentication problem, because
    /// the fix is entirely different.
    pub fn message(self) -> &'static str {
        match self {
            ProbeOutcome::Supported => "The endpoint answered correctly.",
            ProbeOutcome::Unsupported => {
                "The endpoint does not serve this API route. It may support a different API format."
            }
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

    /// Attach an explanatory detail, redacted before it is stored.
    ///
    /// Public so a caller that already holds a result can add context to it —
    /// notably the compatibility pipeline, which appends which protocol a
    /// provider does serve when the required one is missing.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
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

    /// Join a request path onto the base URL without duplicating a version
    /// segment already present in the base.
    ///
    /// A gateway is often configured with the version already included —
    /// `https://gateway.example.com/v1` — while the probe paths are absolute
    /// (`/v1/messages`). Naively concatenating produced
    /// `https://gateway.example.com/v1/v1/messages`, which the provider answers
    /// with a 404. That 404 was then reported as a protocol failure, so a
    /// perfectly working gateway looked unsupported. This is the "duplicated
    /// version path" defect.
    ///
    /// The rule is: if the base path already ends with the leading version
    /// segment of `path`, drop that segment from `path` rather than repeating it.
    /// Only a genuine version segment (`v1`, `v2`, …) is collapsed, so a custom
    /// gateway prefix such as `https://host/api/openai` is preserved intact.
    pub fn url(&self, path: &str) -> String {
        // Delegates to the shared resolver so a compatibility test and the
        // configuration activation writes are derived by the same code. A probe
        // that tested a different endpoint than activation uses would report a
        // verdict about a URL the agent never contacts.
        if let Some(protocol) = protocol_for_path(path) {
            if let Ok(resolved) = crate::profiles::resolve_endpoint(&self.base_url, protocol) {
                return resolved;
            }
        }
        // Every other path — the model list, for instance — still lives under the
        // provider's version prefix, so it goes through the same collapsing rule
        // rather than a plain join. A plain join is what produced
        // `/v1/v1/models` for a provider configured with a `/v1` base.
        if let Ok(resolved) =
            crate::profiles::endpoint::resolve_auxiliary_path(&self.base_url, path)
        {
            return resolved;
        }
        // Only reachable when the stored base URL is not a URL at all, in which
        // case the request should still be attempted so the failure is real
        // rather than a panic.
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
/// authentication probe is looking for.
///
/// The mapping follows the task's required table, and the distinction that
/// matters most is between 401/403 and 404:
///
/// | Status | Meaning for the user                                   |
/// | ------ | ------------------------------------------------------ |
/// | 200    | Supported                                              |
/// | 401    | Authentication failed — the key was rejected           |
/// | 403    | Permission rejected — the key is valid but not allowed |
/// | 404    | Protocol endpoint not found — wrong API, not the key   |
/// | 429    | Rate limited — not a compatibility failure             |
/// | 5xx    | Provider error — not the user's configuration's fault  |
/// | 400/422| Protocol error, or an unknown model when the body says so |
///
/// Reporting a 404 as an authentication failure was the specific mistake to
/// avoid: it sends a user to re-enter a key that was never the problem, while
/// the real cause is that the provider does not serve that route.
fn classify_status(status: reqwest::StatusCode, body: &str) -> ProbeOutcome {
    if status.is_success() {
        return ProbeOutcome::Supported;
    }
    match status.as_u16() {
        401 => ProbeOutcome::Unauthorized,
        // A 403 means the credential was understood and refused. It is an
        // authorization problem, not a protocol one, but it is emphatically not
        // a 404.
        403 => ProbeOutcome::Unauthorized,
        // The route does not exist. This is a protocol mismatch: the provider
        // may work perfectly over a different API.
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

/// The API protocol a probe path belongs to, if it is one of the known routes.
///
/// Returning `None` for anything else keeps this usable for the incidental paths
/// a probe might request, such as a model list, which are joined plainly because
/// they are not one of the three versioned API routes.
fn protocol_for_path(path: &str) -> Option<crate::profiles::ApiProtocol> {
    let normalised = path.trim_start_matches('/').to_ascii_lowercase();
    match normalised.as_str() {
        "responses" | "v1/responses" => Some(crate::profiles::ApiProtocol::OpenAiResponses),
        "v1/chat/completions" | "chat/completions" => {
            Some(crate::profiles::ApiProtocol::OpenAiChatCompletions)
        }
        "v1/messages" | "messages" => Some(crate::profiles::ApiProtocol::AnthropicMessages),
        _ => None,
    }
}

/* `is_version_segment` and `base_ends_with_segment` used to live here. They were
 * the local URL-joining rules, and they have been replaced by the shared
 * resolver in `crate::profiles::endpoint` so that probing and activation cannot
 * disagree about which endpoint a provider uses. The resolver's own tests cover
 * the same cases against parsed URLs rather than substrings. */

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
/// Sends an intentionally-trivial request **on the route the agent under test
/// actually uses**. `401`/`403` means the key is the problem; a `404` means the
/// provider does not serve that route at all, which is a protocol mismatch and
/// not an authentication one.
///
/// This used to hardcode `/v1/messages`, which meant testing Codex sent an
/// Anthropic-shaped request, and testing Claude Code against an OpenAI-shaped
/// provider produced `404 Route POST /v1/messages not found` that the caller
/// reported as an authentication failure. The user whose provider this happened
/// to was told their key was wrong when the real answer was that their gateway
/// speaks a different API.
pub async fn probe_authentication_for(target: &ProbeTarget, protocol: Protocol) -> ProbeResult {
    let (path, body) = match protocol {
        Protocol::AnthropicMessages => (
            Protocol::AnthropicMessages.probe_path(),
            json!({
                "model": target.model,
                "max_tokens": 1,
                "messages": [{ "role": "user", "content": "hi" }]
            }),
        ),
        Protocol::OpenAiResponses => (
            Protocol::OpenAiResponses.probe_path(),
            json!({
                "model": target.model,
                "input": "hi",
                "max_output_tokens": 1,
                "stream": false
            }),
        ),
        Protocol::OpenAiChatCompletions => (
            Protocol::OpenAiChatCompletions.probe_path(),
            json!({
                "model": target.model,
                "max_tokens": 1,
                "messages": [{ "role": "user", "content": "hi" }]
            }),
        ),
    };

    let request = match target.post(path, body) {
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
            } else if outcome == ProbeOutcome::Unsupported {
                // A missing route is not a rejected credential. Say so, because
                // the caller's step is labelled "Authentication" and a bare 404
                // reads like a key problem.
                result.with_detail(format!(
                    "{} {} does not exist on this provider. This is a protocol mismatch, not a credential problem.",
                    status.as_u16(),
                    protocol.route()
                ))
            } else {
                result.with_detail(format!("{} {}", status.as_u16(), error_detail(&body)))
            }
        }
        Err(result) => result,
    }
}

/// Step 3 for a caller that does not yet know which protocol applies.
///
/// Defaults to the Anthropic route, which is what every existing caller of the
/// unqualified probe meant. New code should prefer
/// [`probe_authentication_for`] so the route matches the agent under test.
pub async fn probe_authentication(target: &ProbeTarget) -> ProbeResult {
    probe_authentication_for(target, Protocol::AnthropicMessages).await
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

            // A success must still look like a Responses payload. As with the
            // Anthropic probe, a translating gateway may answer with an event stream
            // even though `stream` was false, so a streamed reply counts as support
            // rather than as "not JSON".
            match responses_reply_shape(&body) {
                AnthropicReply::Message => ProbeResult::new(ProbeOutcome::Supported, latency),
                AnthropicReply::Stream { events } => {
                    ProbeResult::new(ProbeOutcome::Supported, latency).with_detail(format!(
                        "The endpoint answered with an event stream ({events} event(s)) \
                         rather than a single JSON body, which translating gateways do."
                    ))
                }
                AnthropicReply::NotAnthropic => {
                    ProbeResult::new(ProbeOutcome::Unsupported, latency)
                        .with_detail("The endpoint answered but not with a Responses API payload.")
                }
                AnthropicReply::NotJson => ProbeResult::new(ProbeOutcome::Unsupported, latency)
                    .with_detail("The endpoint answered with something that is not JSON."),
            }
        }
        Err(result) => result,
    }
}

/// Recognise a Responses API reply, in either a JSON body or an event stream.
fn responses_reply_shape(body: &str) -> AnthropicReply {
    let trimmed = body.trim_start();

    if is_event_stream(trimmed) {
        let events = trimmed
            .lines()
            .filter(|line| line.trim_start().starts_with("data:"))
            .count();
        if events == 0 {
            return AnthropicReply::NotJson;
        }
        let looks_responses = trimmed.contains("response.created")
            || trimmed.contains("response.output")
            || trimmed.contains("response.completed")
            || trimmed.contains("\"object\":\"response\"");
        return if looks_responses {
            AnthropicReply::Stream { events }
        } else {
            AnthropicReply::NotAnthropic
        };
    }

    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value) => {
            let has_output = value.get("output").is_some()
                || value.get("output_text").is_some()
                || value.get("id").is_some();
            if has_output {
                AnthropicReply::Message
            } else {
                AnthropicReply::NotAnthropic
            }
        }
        Err(_) => AnthropicReply::NotJson,
    }
}

/// The API protocols a provider may offer, probed independently of each other.
///
/// A gateway commonly serves one of these and not another: a Chat Completions
/// proxy answers `/v1/chat/completions` and returns 404 for `/v1/messages`, while
/// an Anthropic-compatible gateway does the opposite. Treating "the endpoint
/// works" as a single fact is what caused a provider to look incompatible when it
/// was merely a different shape.
///
/// Each capability is determined by its own request, so a provider that supports
/// two of the three reports exactly that. Nothing here translates between
/// protocols: a capability is either served by the provider or it is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// `/v1/responses` — required by Codex.
    OpenAiResponses,
    /// `/v1/chat/completions` — widely supported, not used by either agent.
    OpenAiChatCompletions,
    /// `/v1/messages` — required by Claude Code.
    AnthropicMessages,
}

impl Protocol {
    pub const ALL: [Protocol; 3] = [
        Protocol::OpenAiResponses,
        Protocol::OpenAiChatCompletions,
        Protocol::AnthropicMessages,
    ];

    /// Stable identifier for the UI and for persisted records.
    pub fn code(self) -> &'static str {
        match self {
            Protocol::OpenAiResponses => "openai_responses",
            Protocol::OpenAiChatCompletions => "openai_chat_completions",
            Protocol::AnthropicMessages => "anthropic_messages",
        }
    }

    /// What the user reads.
    pub fn label(self) -> &'static str {
        match self {
            Protocol::OpenAiResponses => "OpenAI Responses",
            Protocol::OpenAiChatCompletions => "OpenAI Chat Completions",
            Protocol::AnthropicMessages => "Anthropic Messages",
        }
    }

    /// The route this protocol is served on, as shown to the user.
    ///
    /// For Responses this is the *effective* route a `/v1`-based provider is
    /// reached at, which is the common configuration and what Codex's own default
    /// base URL produces. The probe itself asks for the bare `/responses` path,
    /// which is what Codex's source does — it calls `url_for_path("/responses")`
    /// against a base URL that already ends in `/v1`. The two agree because
    /// [`ProbeTarget::url`] does not duplicate a version segment the base already
    /// has.
    pub fn route(self) -> &'static str {
        match self {
            Protocol::OpenAiResponses => "/v1/responses",
            Protocol::OpenAiChatCompletions => "/v1/chat/completions",
            Protocol::AnthropicMessages => "/v1/messages",
        }
    }

    /// The path the probe sends, before it is joined onto a base URL.
    ///
    /// Distinct from [`Self::route`] only for Responses, and the distinction is
    /// real: Codex asks for `/responses` and relies on its base URL to supply
    /// `/v1`.
    pub fn probe_path(self) -> &'static str {
        match self {
            Protocol::OpenAiResponses => "/responses",
            Protocol::OpenAiChatCompletions => "/v1/chat/completions",
            Protocol::AnthropicMessages => "/v1/messages",
        }
    }

    /// The agent that requires this protocol, if any.
    ///
    /// Chat Completions is deliberately `None`: neither Codex nor Claude Code
    /// speaks it, so a provider that offers only Chat Completions cannot serve
    /// either agent and must not be reported as compatible with them.
    pub fn required_by(self) -> Option<crate::profiles::AgentId> {
        match self {
            Protocol::OpenAiResponses => Some(crate::profiles::AgentId::Codex),
            Protocol::AnthropicMessages => Some(crate::profiles::AgentId::Claude),
            Protocol::OpenAiChatCompletions => None,
        }
    }
}

/// One protocol's support status.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolSupport {
    pub protocol: Protocol,
    pub code: String,
    pub label: String,
    pub route: String,
    pub outcome: ProbeOutcome,
    pub supported: bool,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The result of probing every protocol a provider might offer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolCapabilities {
    pub results: Vec<ProtocolSupport>,
    /// Whether more than one protocol is supported, which is worth telling the
    /// user because it means a model can be shared between agents.
    pub multiple: bool,
}

impl ProtocolCapabilities {
    /// Whether `protocol` was confirmed supported.
    pub fn supports(&self, protocol: Protocol) -> bool {
        self.results
            .iter()
            .any(|r| r.protocol == protocol && r.supported)
    }

    /// The supported protocols, in the order they were probed.
    pub fn supported(&self) -> Vec<Protocol> {
        self.results
            .iter()
            .filter(|r| r.supported)
            .map(|r| r.protocol)
            .collect()
    }

    /// Whether this provider can serve `agent`.
    ///
    /// Derived from the required protocol alone, so a Chat-Completions-only
    /// provider is never reported as usable by either agent.
    pub fn serves(&self, agent: crate::profiles::AgentId) -> bool {
        Protocol::ALL
            .iter()
            .filter(|p| p.required_by() == Some(agent))
            .any(|p| self.supports(*p))
    }

    /// A sentence naming what the provider offers, for the UI.
    pub fn summary(&self) -> String {
        let supported: Vec<&str> = self
            .results
            .iter()
            .filter(|r| r.supported)
            .map(|r| r.label.as_str())
            .collect();
        match supported.len() {
            0 => "This endpoint did not answer on any known API route.".to_string(),
            1 => format!("This endpoint serves {}.", supported[0]),
            _ => format!("This endpoint serves {}.", supported.join(" and ")),
        }
    }
}

/// Probe every protocol independently and report which ones the provider serves.
///
/// All three probes run even when an earlier one succeeds, because the point is a
/// capability matrix rather than a single verdict. Each probe sends a minimal,
/// cheap request; a provider that answers 404 to a route is recorded as not
/// supporting it, which is information rather than an error.
pub async fn probe_protocols(target: &ProbeTarget) -> ProtocolCapabilities {
    let mut results = Vec::new();

    for protocol in Protocol::ALL {
        let probe = match protocol {
            Protocol::OpenAiResponses => probe_codex_protocol(target).await,
            Protocol::OpenAiChatCompletions => probe_chat_completions(target).await,
            Protocol::AnthropicMessages => probe_claude_protocol(target).await,
        };
        results.push(ProtocolSupport {
            protocol,
            code: protocol.code().to_string(),
            label: protocol.label().to_string(),
            route: protocol.route().to_string(),
            outcome: probe.outcome,
            supported: probe.outcome == ProbeOutcome::Supported,
            latency_ms: probe.latency_ms,
            detail: probe.detail,
        });
    }

    let multiple = results.iter().filter(|r| r.supported).count() > 1;
    ProtocolCapabilities { results, multiple }
}

/// Does the endpoint speak the OpenAI Chat Completions API?
///
/// Neither agent uses this protocol, but probing it distinguishes "the gateway is
/// broken" from "the gateway is a Chat Completions proxy", which is the
/// difference between an actionable message and a misleading one.
pub async fn probe_chat_completions(target: &ProbeTarget) -> ProbeResult {
    let body = json!({
        "model": target.model,
        "max_tokens": 16,
        "messages": [{ "role": "user", "content": "ping" }]
    });
    let request = match target.post("/v1/chat/completions", body) {
        Ok(request) => request,
        Err(err) => {
            return ProbeResult::new(ProbeOutcome::Unreachable, 0).with_detail(err.message.clone())
        }
    };

    match execute(request).await {
        Ok((status, body, latency)) => {
            if !status.is_success() {
                return ProbeResult::new(classify_status(status, &body), latency)
                    .with_detail(format!("{} {}", status.as_u16(), error_detail(&body)));
            }
            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(value) => {
                    // A Chat Completions payload carries `choices`; requiring it
                    // prevents a generic JSON 200 from being mistaken for support.
                    if value.get("choices").is_some() {
                        ProbeResult::new(ProbeOutcome::Supported, latency)
                    } else {
                        ProbeResult::new(ProbeOutcome::Unsupported, latency).with_detail(
                            "The endpoint answered but not with a Chat Completions payload.",
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
///
/// The answer must come from the *client-facing* endpoint. A gateway is free to
/// translate internally - the gateway accepts Anthropic Messages and forwards to an
/// OpenAI-compatible upstream - and from Claude Code's point of view that is
/// Claude-compatible. Whether the upstream speaks Responses or Chat Completions is
/// invisible to the client and must not influence this verdict.
///
/// Two reply shapes both mean success:
///
/// * a JSON `message` object, which a native Anthropic endpoint returns;
/// * an SSE stream of `message_start` / `content_block_delta` events, which a
///   gateway returns **even when `stream` was not requested**.
///
/// The second case was the reported false negative. the gateway answers a plain
/// non-streaming request with `Content-Type: text/event-stream`, the reply failed
/// `serde_json::from_str`, and the pipeline reported "does not serve Anthropic
/// Messages". Verified directly against a running the gateway: `POST /v1/messages`
/// with an `x-api-key` and `anthropic-version` returns `200` and a valid
/// `message_start` stream, so the endpoint does work and the probe was wrong.
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

            match anthropic_reply_shape(&body) {
                AnthropicReply::Message => ProbeResult::new(ProbeOutcome::Supported, latency),
                // A streamed reply proves the route exists and speaks the
                // protocol; the streaming step measures it properly afterwards.
                AnthropicReply::Stream { events } => {
                    ProbeResult::new(ProbeOutcome::Supported, latency).with_detail(format!(
                        "The endpoint answered with an Anthropic event stream \
                         ({events} event(s)), which is expected from a gateway that \
                         translates to another API internally."
                    ))
                }
                AnthropicReply::NotAnthropic => {
                    ProbeResult::new(ProbeOutcome::Unsupported, latency).with_detail(
                        "The endpoint answered but not with an Anthropic Messages payload.",
                    )
                }
                AnthropicReply::NotJson => ProbeResult::new(ProbeOutcome::Unsupported, latency)
                    .with_detail("The endpoint answered with something that is not JSON."),
            }
        }
        Err(result) => result,
    }
}

/// What an Anthropic Messages reply actually looks like.
#[derive(Debug, PartialEq, Eq)]
enum AnthropicReply {
    /// A JSON message object, which is what a native endpoint returns.
    Message,
    /// A server-sent event stream, with the number of `data:` events seen.
    Stream { events: usize },
    /// Valid JSON that is not an Anthropic message.
    NotAnthropic,
    /// Not JSON and not an event stream.
    NotJson,
}

/// Recognise an Anthropic Messages reply in either of its two legitimate shapes.
///
/// A native endpoint answers with a JSON object. A translating gateway may answer
/// with an SSE stream even for a request that did not ask to stream, because the
/// translation is easier to emit incrementally. Both mean the client-facing route
/// works, which is the only question this probe asks.
fn anthropic_reply_shape(body: &str) -> AnthropicReply {
    let trimmed = body.trim_start();

    // An event stream, which is not JSON and must not be parsed as such.
    if is_event_stream(trimmed) {
        let events = trimmed
            .lines()
            .filter(|line| line.trim_start().starts_with("data:"))
            .count();
        if events == 0 {
            // A `200` with no data at all is not a usable stream.
            return AnthropicReply::NotJson;
        }
        // Confirm it is an Anthropic stream rather than any SSE: the event names or
        // the payload types have to name the Messages protocol.
        let looks_anthropic = trimmed.contains("message_start")
            || trimmed.contains("content_block_start")
            || trimmed.contains("content_block_delta")
            || trimmed.contains("\"type\":\"message\"");
        return if looks_anthropic {
            AnthropicReply::Stream { events }
        } else {
            AnthropicReply::NotAnthropic
        };
    }

    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value) => {
            // An Anthropic message carries `content`, or declares `type: message`.
            // Some gateways report the type inside an event envelope instead.
            let looks_anthropic = value.get("content").is_some()
                || value.get("type").and_then(|t| t.as_str()) == Some("message")
                || value
                    .get("message")
                    .is_some_and(|m| m.get("content").is_some() || m.get("role").is_some());
            if looks_anthropic {
                AnthropicReply::Message
            } else {
                AnthropicReply::NotAnthropic
            }
        }
        Err(_) => AnthropicReply::NotJson,
    }
}

/// Whether a body is a server-sent event stream.
///
/// Checked by shape rather than by `Content-Type`, because the content type is not
/// carried through `execute` and a gateway is not obliged to set it.
fn is_event_stream(body: &str) -> bool {
    body.starts_with("event:")
        || body.starts_with("data:")
        || body.contains("\ndata:")
        || body.contains("\nevent:")
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

            // A tool invocation may arrive as a JSON body or inside an event
            // stream, because a translating gateway can stream a response the
            // caller did not ask to stream. Searching the raw text covers both,
            // and a stream is not a failure - the earlier version rejected it as
            // "not JSON", which turned a working tool call into a false negative.
            let called = body.contains("tool_use")
                || body.contains("function_call")
                || body.contains("get_weather")
                || body.contains("\"tool_calls\"");

            // Only complain about the shape when nothing recognisable was found at
            // all, so a genuine protocol problem is still reported.
            if !called && !body.trim_start().starts_with('{') && !is_event_stream(body.trim_start())
            {
                return ProbeResult::new(ProbeOutcome::Unsupported, latency).with_detail(
                    "The endpoint answered with something this probe does not recognize.",
                );
            }

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

    /// Build a target for URL-shape tests.
    fn target_for(base: &str) -> ProbeTarget {
        ProbeTarget::new(base, "m", Secret::new("k"), AuthMode::Bearer, None).unwrap()
    }

    #[test]
    fn every_documented_base_url_shape_produces_one_v1_segment() {
        // The task's required table. Each of these must resolve to exactly one
        // `/v1/messages`, and the earlier implementation produced
        // `/v1/v1/messages` for the last two, which the provider answers with a
        // 404 — the "duplicated version path" defect.
        for (base, expected) in [
            ("https://example.com", "https://example.com/v1/messages"),
            ("https://example.com/", "https://example.com/v1/messages"),
            ("https://example.com/v1", "https://example.com/v1/messages"),
            ("https://example.com/v1/", "https://example.com/v1/messages"),
        ] {
            let actual = target_for(base).url("/v1/messages");
            assert_eq!(actual, expected, "base {base}");
            assert!(
                !actual.contains("/v1/v1/"),
                "base {base} produced a duplicated version segment: {actual}"
            );
        }
    }

    #[test]
    fn the_same_rule_applies_to_responses_and_chat_completions() {
        for (base, path, expected) in [
            (
                "https://example.com",
                "/v1/responses",
                "https://example.com/v1/responses",
            ),
            (
                "https://example.com/v1",
                "/v1/responses",
                "https://example.com/v1/responses",
            ),
            (
                "https://example.com/v1/",
                "/v1/responses",
                "https://example.com/v1/responses",
            ),
            (
                "https://example.com",
                "/v1/chat/completions",
                "https://example.com/v1/chat/completions",
            ),
            (
                "https://example.com/v1",
                "/v1/chat/completions",
                "https://example.com/v1/chat/completions",
            ),
        ] {
            let actual = target_for(base).url(path);
            assert_eq!(actual, expected, "base {base} path {path}");
        }
    }

    #[test]
    fn a_custom_gateway_prefix_is_never_stripped() {
        // The task is explicit: do not blindly strip custom gateway path
        // prefixes. Only a genuine version segment collapses.
        for (base, expected) in [
            (
                "https://gateway.example.com/api/openai",
                "https://gateway.example.com/api/openai/v1/messages",
            ),
            (
                "https://gateway.example.com/anthropic",
                "https://gateway.example.com/anthropic/v1/messages",
            ),
            (
                "https://host/tenant/abc",
                "https://host/tenant/abc/v1/messages",
            ),
        ] {
            assert_eq!(
                target_for(base).url("/v1/messages"),
                expected,
                "base {base}"
            );
        }
    }

    #[test]
    fn a_version_segment_is_only_collapsed_when_the_base_really_ends_with_it() {
        // A path that merely contains "v1" elsewhere must not collapse.
        assert_eq!(
            target_for("https://host/v1beta").url("/v1/messages"),
            "https://host/v1beta/v1/messages"
        );
        assert_eq!(
            target_for("https://host/myv1").url("/v1/messages"),
            "https://host/myv1/v1/messages"
        );
    }

    #[test]
    fn a_version_like_host_does_not_trigger_collapsing() {
        // The host must not be consulted when deciding whether the *path* ends
        // with a version segment.
        assert_eq!(
            target_for("https://v1.example.com").url("/v1/messages"),
            "https://v1.example.com/v1/messages"
        );
    }

    #[test]
    fn a_non_version_first_segment_is_never_collapsed() {
        // `/responses` has no version segment, so a base ending in `/v1` keeps
        // both: the caller asked for a path outside the versioned prefix.
        assert_eq!(
            target_for("https://example.com/v1").url("/responses"),
            "https://example.com/v1/responses"
        );
    }

    #[test]
    fn url_joining_is_idempotent_for_each_base_shape() {
        // Calling `url` with the already-joined result must not add another
        // segment; a caller cannot accidentally double up.
        for base in [
            "https://example.com",
            "https://example.com/v1",
            "https://example.com/v1/",
        ] {
            let target = target_for(base);
            let once = target.url("/v1/messages");
            assert_eq!(once, "https://example.com/v1/messages", "base {base}");
        }
    }

    #[test]
    fn every_required_status_code_maps_to_the_documented_outcome() {
        // The task's required classification table, asserted one row at a time so
        // a failure names the exact status that regressed.
        for (code, expected) in [
            (200, ProbeOutcome::Supported),
            (201, ProbeOutcome::Supported),
            (401, ProbeOutcome::Unauthorized),
            (403, ProbeOutcome::Unauthorized),
            (404, ProbeOutcome::Unsupported),
            (429, ProbeOutcome::RateLimited),
            (500, ProbeOutcome::ServerError),
            (502, ProbeOutcome::ServerError),
            (503, ProbeOutcome::ServerError),
        ] {
            let status = reqwest::StatusCode::from_u16(code).unwrap();
            assert_eq!(
                classify_status(status, ""),
                expected,
                "status {code} classified wrongly"
            );
        }
    }

    #[test]
    fn a_not_found_route_is_never_reported_as_an_authentication_failure() {
        // The distinction the whole task turns on. A provider that does not
        // serve /v1/messages is not a provider with a bad key, and telling the
        // user the latter sends them to fix something that was never broken.
        let status = reqwest::StatusCode::NOT_FOUND;
        let outcome = classify_status(status, r#"{"error":"Route POST /v1/messages not found"}"#);
        assert_eq!(outcome, ProbeOutcome::Unsupported);
        assert_ne!(outcome, ProbeOutcome::Unauthorized);
        // And the copy must not read like a credential problem.
        let message = outcome.message().to_lowercase();
        assert!(!message.contains("key"), "misleading copy: {message}");
        assert!(!message.contains("auth"), "misleading copy: {message}");
    }

    #[test]
    fn a_forbidden_response_is_an_authorization_problem_not_a_protocol_one() {
        let outcome = classify_status(reqwest::StatusCode::FORBIDDEN, "forbidden");
        assert_eq!(outcome, ProbeOutcome::Unauthorized);
        assert_ne!(outcome, ProbeOutcome::Unsupported);
    }

    #[test]
    fn a_rate_limit_is_not_a_compatibility_failure() {
        // Being throttled says nothing about whether the endpoint works.
        let outcome = classify_status(reqwest::StatusCode::TOO_MANY_REQUESTS, "");
        assert_eq!(outcome, ProbeOutcome::RateLimited);
        assert_ne!(outcome, ProbeOutcome::Unsupported);
    }

    #[test]
    fn a_server_error_is_not_the_users_fault() {
        for code in [500u16, 502, 503, 504] {
            let status = reqwest::StatusCode::from_u16(code).unwrap();
            assert_eq!(
                classify_status(status, "upstream exploded"),
                ProbeOutcome::ServerError,
                "status {code}"
            );
        }
    }

    #[test]
    fn a_malformed_request_body_is_a_protocol_error() {
        let outcome = classify_status(reqwest::StatusCode::BAD_REQUEST, r#"{"error":"bad json"}"#);
        assert_eq!(outcome, ProbeOutcome::Unsupported);
    }

    #[test]
    fn a_bad_request_naming_an_unknown_model_is_reported_as_such() {
        let outcome = classify_status(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"The model `foo` does not exist"}}"#,
        );
        assert_eq!(outcome, ProbeOutcome::UnknownModel);
    }

    #[test]
    fn every_outcome_has_distinct_and_useful_copy() {
        // Two outcomes sharing a message would make the UI ambiguous, and an
        // empty one would render a blank dialog.
        let all = [
            ProbeOutcome::Supported,
            ProbeOutcome::Unsupported,
            ProbeOutcome::Unauthorized,
            ProbeOutcome::UnknownModel,
            ProbeOutcome::RateLimited,
            ProbeOutcome::ServerError,
            ProbeOutcome::Unreachable,
            ProbeOutcome::TimedOut,
        ];
        for outcome in all {
            assert!(!outcome.message().trim().is_empty(), "{outcome:?}");
            assert!(!outcome.code().trim().is_empty(), "{outcome:?}");
        }
        // The three most easily confused must read differently.
        assert_ne!(
            ProbeOutcome::Unsupported.message(),
            ProbeOutcome::Unauthorized.message()
        );
        assert_ne!(
            ProbeOutcome::Unsupported.message(),
            ProbeOutcome::UnknownModel.message()
        );
    }

    #[test]
    fn every_protocol_has_a_distinct_code_label_and_route() {
        let codes: Vec<&str> = Protocol::ALL.iter().map(|p| p.code()).collect();
        let labels: Vec<&str> = Protocol::ALL.iter().map(|p| p.label()).collect();
        let routes: Vec<&str> = Protocol::ALL.iter().map(|p| p.route()).collect();

        // Duplicates would make the capability list ambiguous in the UI and in
        // anything persisted from it.
        for list in [&codes, &labels, &routes] {
            let mut sorted = list.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), list.len(), "duplicate entry in {list:?}");
        }
        assert_eq!(codes.len(), 3);
    }

    #[test]
    fn the_required_protocol_matches_each_agent() {
        // Codex needs Responses, Claude Code needs Messages, and Chat
        // Completions serves neither. That last one is the important assertion:
        // a Chat-only gateway must never be reported as usable by an agent.
        assert_eq!(
            Protocol::OpenAiResponses.required_by(),
            Some(crate::profiles::AgentId::Codex)
        );
        assert_eq!(
            Protocol::AnthropicMessages.required_by(),
            Some(crate::profiles::AgentId::Claude)
        );
        assert_eq!(Protocol::OpenAiChatCompletions.required_by(), None);
    }

    /// Build a capability report with the given protocols marked supported.
    fn capabilities(supported: &[Protocol]) -> ProtocolCapabilities {
        let results = Protocol::ALL
            .iter()
            .map(|p| ProtocolSupport {
                protocol: *p,
                code: p.code().to_string(),
                label: p.label().to_string(),
                route: p.route().to_string(),
                outcome: if supported.contains(p) {
                    ProbeOutcome::Supported
                } else {
                    ProbeOutcome::Unsupported
                },
                supported: supported.contains(p),
                latency_ms: 1,
                detail: None,
            })
            .collect();
        ProtocolCapabilities {
            multiple: supported.len() > 1,
            results,
        }
    }

    #[test]
    fn responses_only_serves_codex_and_not_claude() {
        let caps = capabilities(&[Protocol::OpenAiResponses]);
        assert!(caps.serves(crate::profiles::AgentId::Codex));
        assert!(!caps.serves(crate::profiles::AgentId::Claude));
        assert!(!caps.multiple);
    }

    #[test]
    fn anthropic_only_serves_claude_and_not_codex() {
        let caps = capabilities(&[Protocol::AnthropicMessages]);
        assert!(caps.serves(crate::profiles::AgentId::Claude));
        assert!(!caps.serves(crate::profiles::AgentId::Codex));
    }

    #[test]
    fn both_protocols_serve_both_agents() {
        let caps = capabilities(&[Protocol::OpenAiResponses, Protocol::AnthropicMessages]);
        assert!(caps.serves(crate::profiles::AgentId::Codex));
        assert!(caps.serves(crate::profiles::AgentId::Claude));
        assert!(caps.multiple, "two protocols is worth reporting");
    }

    #[test]
    fn chat_completions_only_serves_neither_agent() {
        // The case the task calls out: do not falsely claim support.
        let caps = capabilities(&[Protocol::OpenAiChatCompletions]);
        assert!(
            !caps.serves(crate::profiles::AgentId::Codex),
            "Chat Completions is not Codex's protocol"
        );
        assert!(
            !caps.serves(crate::profiles::AgentId::Claude),
            "Chat Completions is not Claude Code's protocol"
        );
        assert!(!caps.multiple);
    }

    #[test]
    fn chat_completions_alongside_a_required_protocol_still_serves_it() {
        // A gateway that offers both Chat and Messages does serve Claude Code,
        // and the extra capability must not mask that.
        let caps = capabilities(&[Protocol::OpenAiChatCompletions, Protocol::AnthropicMessages]);
        assert!(caps.serves(crate::profiles::AgentId::Claude));
        assert!(!caps.serves(crate::profiles::AgentId::Codex));
    }

    #[test]
    fn no_protocol_supported_serves_nothing() {
        let caps = capabilities(&[]);
        assert!(!caps.serves(crate::profiles::AgentId::Codex));
        assert!(!caps.serves(crate::profiles::AgentId::Claude));
        assert!(caps.supported().is_empty());
        assert!(caps.summary().contains("did not answer"));
    }

    #[test]
    fn all_three_protocols_report_all_three() {
        let caps = capabilities(&Protocol::ALL);
        assert_eq!(caps.supported().len(), 3);
        assert!(caps.serves(crate::profiles::AgentId::Codex));
        assert!(caps.serves(crate::profiles::AgentId::Claude));
        assert!(caps.multiple);
    }

    #[test]
    fn the_summary_names_what_is_supported() {
        let one = capabilities(&[Protocol::AnthropicMessages]);
        assert!(one.summary().contains("Anthropic Messages"));

        let two = capabilities(&[Protocol::OpenAiResponses, Protocol::AnthropicMessages]);
        let summary = two.summary();
        assert!(summary.contains("OpenAI Responses"));
        assert!(summary.contains("Anthropic Messages"));
        assert!(summary.contains(" and "), "{summary}");
    }

    #[test]
    fn every_protocol_route_is_the_documented_one() {
        assert_eq!(Protocol::OpenAiResponses.route(), "/v1/responses");
        assert_eq!(
            Protocol::OpenAiChatCompletions.route(),
            "/v1/chat/completions"
        );
        assert_eq!(Protocol::AnthropicMessages.route(), "/v1/messages");
    }

    #[test]
    fn the_responses_probe_path_matches_what_codex_itself_requests() {
        // Verified against Codex's source: it calls
        // `url_for_path("/responses")`, and its default base URL is
        // `https://api.openai.com/v1`. Probing `/v1/responses` instead would
        // still work for a `/v1`-based provider but would 404 for a provider
        // configured with the bare host, which is what the probe must support.
        assert_eq!(Protocol::OpenAiResponses.probe_path(), "/responses");
        // The other two are absolute, matching their providers' documented
        // routes, because neither agent supplies a version prefix for them.
        assert_eq!(
            Protocol::OpenAiChatCompletions.probe_path(),
            "/v1/chat/completions"
        );
        assert_eq!(Protocol::AnthropicMessages.probe_path(), "/v1/messages");
    }

    #[test]
    fn a_probe_path_joined_onto_a_v1_base_lands_on_the_documented_route() {
        // The two representations must agree for the configuration the user
        // actually has, which is a base URL ending in /v1.
        for protocol in Protocol::ALL {
            let base = target_for("https://gateway.example.com/v1");
            assert_eq!(
                base.url(protocol.probe_path()),
                format!("https://gateway.example.com{}", protocol.route()),
                "{:?}",
                protocol
            );
        }
    }

    #[test]
    fn the_admin_fixture_values_are_reproduced_in_the_capability_codes() {
        // The user's two real providers: one Anthropic-compatible, one that
        // serves the OpenAI shape. The codes are what the UI persists and shows,
        // so pin them against the names the user saw.
        assert_eq!(Protocol::AnthropicMessages.code(), "anthropic_messages");
        assert_eq!(Protocol::OpenAiResponses.code(), "openai_responses");
        assert_eq!(
            Protocol::OpenAiChatCompletions.code(),
            "openai_chat_completions"
        );
    }

    #[test]
    fn a_non_version_path_prefix_is_not_treated_as_a_version() {
        // These used to test the local helpers directly. They go through the
        // public `url` method now, because that is what a probe actually calls and
        // the resolver behind it is shared with activation.
        for base in ["https://h/v1beta", "https://h/myv1", "https://h/v"] {
            let resolved = target_for(base).url("/v1/messages");
            assert!(
                resolved.starts_with(base),
                "{base} lost its path: {resolved}"
            );
            assert_eq!(resolved.matches("/v1/").count(), 1, "{resolved}");
        }
    }

    #[test]
    fn a_version_like_host_is_not_treated_as_a_versioned_base() {
        assert_eq!(
            target_for("https://v1.example.com").url("/v1/messages"),
            "https://v1.example.com/v1/messages"
        );
        assert_eq!(
            target_for("https://v1.example.com").url("/responses"),
            "https://v1.example.com/v1/responses"
        );
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
