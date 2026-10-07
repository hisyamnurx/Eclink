//! End-to-end provider tests against a real local HTTP server.
//!
//! These exercise the actual network stack — reqwest, redirect policy, header
//! injection, status classification, and body parsing — against responses this
//! test controls. No paid provider is ever contacted, and no external network
//! access is required.
//!
//! The mock server is a hand-rolled `TcpListener` rather than a dependency: it
//! needs to do very little, and a test that depends on a mock framework is a
//! test that breaks when the mock framework changes.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use eclyzer_bridge_lib::profiles::AuthMode;
use eclyzer_bridge_lib::testing::probes::{
    probe_authentication, probe_claude_protocol, probe_codex_protocol, probe_model,
    probe_reachability, probe_streaming, probe_tool_calling, ProbeOutcome, ProbeTarget,
};
use eclyzer_bridge_lib::testing::Secret;

/// A canned HTTP response.
#[derive(Debug, Clone)]
struct Response {
    status: u16,
    body: String,
    content_type: &'static str,
}

impl Response {
    fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_string(),
            content_type: "application/json",
        }
    }

    fn sse(body: &str) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            content_type: "text/event-stream",
        }
    }
}

/// A single recorded request, for asserting what the client actually sent.
#[derive(Debug, Clone, Default)]
struct Recorded {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

/// The routing rule for one path.
type Route = (String, Response);

/// A local HTTP server that answers a fixed set of routes.
struct MockServer {
    base_url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    // Held so the receiver outlives the server; the channel closes on drop.
    _rx: Receiver<()>,
}

impl MockServer {
    /// Start a server that answers according to `routes`.
    ///
    /// The first route whose key appears in the request path wins, so a test can
    /// register a general fallback alongside specific paths.
    fn start(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a free port");
        let addr = listener.local_addr().expect("local addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = channel::<()>();

        let routes = Arc::new(routes);
        let recorded = Arc::clone(&requests);

        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let routes = Arc::clone(&routes);
                let recorded = Arc::clone(&recorded);
                // One thread per connection keeps a slow request from blocking
                // others; these tests are short-lived.
                thread::spawn(move || {
                    handle(stream, &routes, &recorded);
                });
            }
            // The listener is closed when this thread ends, which happens when
            // the server is dropped.
            let _ = tx.send(());
        });

        Self {
            base_url: format!("http://{addr}"),
            requests,
            _rx: rx,
        }
    }

    /// Every request the server has seen, oldest first.
    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("requests lock").clone()
    }
}

/// Read one request, match a route, and write a response.
fn handle(stream: TcpStream, routes: &[Route], recorded: &Arc<Mutex<Vec<Recorded>>>) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    });

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    // Headers, until the blank line.
    let mut headers = HashMap::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.insert(name, value);
        }
    }

    // Body, if the client declared one.
    let mut body = String::new();
    if content_length > 0 {
        let mut buffer = vec![0u8; content_length];
        use std::io::Read;
        if reader.read_exact(&mut buffer).is_ok() {
            body = String::from_utf8_lossy(&buffer).to_string();
        }
    }

    recorded.lock().expect("requests lock").push(Recorded {
        method,
        path: path.clone(),
        headers,
        body: body.clone(),
    });

    // First matching route wins. Three match kinds are supported, because a
    // single path is requested by several different probes with different bodies:
    //
    //   * `path`          - any request to exactly this path
    //   * `path?stream`   - a request whose JSON body has `"stream": true`
    //   * `path?tools`    - a request whose JSON body carries a `tools` array
    //
    // Without this, one path could only ever return one body, so a mock could not
    // both answer the protocol probe with JSON and the streaming probe with SSE.
    let response = routes
        .iter()
        .find(|(matcher, _)| route_matches(matcher, &path, &body))
        .map(|(_, response)| response.clone())
        .unwrap_or_else(|| Response::json(404, r#"{"error":{"message":"no route"}}"#));

    write_response(stream, &response);
}

/// Whether a route matcher applies to this request.
///
/// Matchers are ordered, so a test registers the most specific one first.
fn route_matches(matcher: &str, path: &str, body: &str) -> bool {
    let (route, qualifier) = match matcher.split_once('?') {
        Some((route, qualifier)) => (route, Some(qualifier)),
        None => (matcher, None),
    };
    if !path.contains(route) {
        return false;
    }
    match qualifier {
        None => true,
        Some("stream") => serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|value| value.get("stream").and_then(|v| v.as_bool()))
            .unwrap_or(false),
        Some("tools") => serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .map(|value| value.get("tools").is_some())
            .unwrap_or(false),
        // An unknown qualifier never matches, so a typo fails loudly instead of
        // silently behaving like an unqualified route.
        Some(_) => false,
    }
}

/// Write a minimal, correct HTTP/1.1 response and close the connection.
fn write_response(mut stream: TcpStream, response: &Response) {
    let body = response.body.as_bytes();
    let head = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        response.status,
        reason(response.status),
        response.content_type,
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

/// A target pointing at the mock server.
fn target(server: &MockServer, auth: AuthMode) -> ProbeTarget {
    target_with_timeout(server, auth, 5_000)
}

/// A target with an explicit timeout, for tests about timing behaviour.
fn target_with_timeout(server: &MockServer, auth: AuthMode, timeout_ms: u64) -> ProbeTarget {
    ProbeTarget::new(
        &server.base_url,
        "test-model",
        Secret::new("sk-test-key-0123456789"),
        auth,
        Some(timeout_ms),
    )
    .expect("target")
}

/// A successful OpenAI-shaped chat completion.
const OK_CHAT: &str = r#"{"choices":[{"message":{"role":"assistant","content":"ready"}}],
"usage":{"prompt_tokens":5,"completion_tokens":1}}"#;

/// A successful Responses-shaped reply, which Codex uses.
const OK_RESPONSES: &str = r#"{"id":"resp_1","object":"response","status":"completed",
"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ready"}]}]}"#;

/// A successful Anthropic-shaped message.
const OK_MESSAGES: &str = r#"{"id":"msg_1","type":"message","role":"assistant",
"content":[{"type":"text","text":"ready"}],"usage":{"input_tokens":5,"output_tokens":1}}"#;

/// A model list naming the model the pipeline asks about.
///
/// The model step looks the model up by id, so a list that omits it would fail
/// that step and mask what the test is actually checking.
const MODELS_OK: &str = r#"{"data":[{"id":"test-model","object":"model"},{"id":"m","object":"model"},{"id":"example-model","object":"model"},{"id":"vendor_a/example-model","object":"model"},{"id":"dual-model","object":"model"}]}"#;

/// An Anthropic-shaped stream carrying a tool call.
///
/// The streaming step requires SSE, and the tool step requires a tool invocation
/// to appear. Serving both from one body lets the full 7-step pipeline reach
/// Compatible, which is what the real working provider did.
const SSE_MESSAGES_TOOL: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\n",
    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"get_weather\",\"input\":{}}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\\\"Jakarta\\\"}\"}}\n\n",
    "event: message_delta\n",
    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":12}}\n\n",
    "event: message_stop\n",
    "data: {\"type\":\"message_stop\"}\n\n",
);

/// A Responses-shaped stream carrying a function call.
const SSE_RESPONSES_TOOL: &str = concat!(
    "event: response.created\n",
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"status\":\"in_progress\"}}\n\n",
    "event: response.output_item.added\n",
    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"name\":\"get_weather\",\"arguments\":\"\"}}\n\n",
    "event: response.function_call_arguments.delta\n",
    "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"city\\\":\\\"Jakarta\\\"}\"}\n\n",
    "event: response.completed\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\n",
);

/// An Anthropic-shaped non-streaming reply that calls the tool.
const MESSAGES_TOOL_CALL: &str = r#"{"id":"msg_1","type":"message","role":"assistant",
"content":[{"type":"tool_use","id":"toolu_1","name":"get_weather","input":{"city":"Jakarta"}}],
"stop_reason":"tool_use","usage":{"input_tokens":5,"output_tokens":12}}"#;

/// A Responses-shaped non-streaming reply that calls the tool.
const RESPONSES_TOOL_CALL: &str = r#"{"id":"resp_1","object":"response","status":"completed",
"output":[{"type":"function_call","id":"fc_1","name":"get_weather","arguments":"{\"city\":\"Jakarta\"}"}]}"#;

// ---------------------------------------------------------------------------
// Reachability
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_reachable_endpoint_is_reported_as_supported() {
    let server = MockServer::start(vec![(
        "/v1/models".into(),
        Response::json(200, r#"{"data":[]}"#),
    )]);
    let result = probe_reachability(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::Supported,
        "detail: {:?}",
        result.detail
    );
}

#[tokio::test]
async fn an_unauthorized_response_still_proves_reachability() {
    // The endpoint answered, which is all reachability claims.
    let server = MockServer::start(vec![(
        "/v1/models".into(),
        Response::json(401, r#"{"error":{"message":"bad key"}}"#),
    )]);
    let result = probe_reachability(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(result.outcome, ProbeOutcome::Supported);
}

#[tokio::test]
async fn a_closed_port_is_unreachable() {
    // Bind then drop, so the port is almost certainly free.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let target = ProbeTarget::new(
        &format!("http://{addr}"),
        "test-model",
        Secret::new("sk-test"),
        AuthMode::Bearer,
        // Generous, because the first connection in a test binary can be slow to
        // resolve on Windows; a refused connection returns immediately and must
        // not be mistaken for a timeout.
        Some(10_000),
    )
    .unwrap();

    let result = probe_reachability(&target).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::Unreachable,
        "a refused connection must be Unreachable, not {:?}",
        result.outcome
    );
    // The detail must not leak the URL, which could carry a key.
    assert!(
        !result.detail.as_deref().unwrap_or("").contains("sk-"),
        "detail leaked a secret: {:?}",
        result.detail
    );
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_bearer_token_is_sent_as_an_authorization_header() {
    // The authentication probe posts to the Anthropic path, which is the
    // endpoint that returns 401 rather than 404 for a bad key.
    let server = MockServer::start(vec![(
        "/v1/messages".into(),
        Response::json(200, OK_MESSAGES),
    )]);
    let result = probe_authentication(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::Supported,
        "{:?}",
        result.detail
    );

    let sent = server.requests();
    let request = sent.first().expect("a request was made");
    let auth = request
        .headers
        .get("authorization")
        .expect("authorization header");
    assert_eq!(auth, "Bearer sk-test-key-0123456789");
}

#[tokio::test]
async fn x_api_key_mode_sends_the_key_in_that_header_instead() {
    let server = MockServer::start(vec![(
        "/v1/messages".into(),
        Response::json(200, OK_MESSAGES),
    )]);
    let result = probe_authentication(&target(&server, AuthMode::XApiKey)).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::Supported,
        "{:?}",
        result.detail
    );

    let sent = server.requests();
    let request = sent.first().expect("a request was made");
    assert_eq!(
        request.headers.get("x-api-key").map(String::as_str),
        Some("sk-test-key-0123456789")
    );
    // The key must not be sent twice, and not as a bearer token.
    assert!(
        !request.headers.contains_key("authorization"),
        "x_api_key mode must not also send an Authorization header"
    );
}

#[tokio::test]
async fn a_rejected_key_is_reported_as_unauthorized() {
    let server = MockServer::start(vec![(
        "/v1/messages".into(),
        Response::json(401, r#"{"error":{"message":"Incorrect API key provided"}}"#),
    )]);
    let result = probe_authentication(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(result.outcome, ProbeOutcome::Unauthorized);
}

#[tokio::test]
async fn a_forbidden_response_is_also_unauthorized() {
    let server = MockServer::start(vec![(
        "/v1/messages".into(),
        Response::json(403, r#"{"error":{"message":"forbidden"}}"#),
    )]);
    let result = probe_authentication(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(result.outcome, ProbeOutcome::Unauthorized);
}

#[tokio::test]
async fn a_two_hundred_from_the_auth_probe_counts_as_authenticated() {
    // Regression: the shared status classifier must treat a success status as
    // success. When it did not, every working provider was reported as
    // Unsupported, which blocked activation entirely.
    let server = MockServer::start(vec![(
        "/v1/messages".into(),
        Response::json(200, OK_MESSAGES),
    )]);
    let result = probe_authentication(&target(&server, AuthMode::XApiKey)).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::Supported,
        "a 200 must classify as Supported, got {:?} ({:?})",
        result.outcome,
        result.detail
    );
}

#[tokio::test]
async fn a_two_hundred_with_a_chat_shaped_body_is_also_authenticated() {
    // An OpenAI-compatible gateway answering on the Anthropic path is unusual
    // but not an authentication failure.
    let server = MockServer::start(vec![("/v1/messages".into(), Response::json(200, OK_CHAT))]);
    let result = probe_authentication(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::Supported,
        "{:?}",
        result.detail
    );
}

#[tokio::test]
async fn an_error_body_that_echoes_the_key_is_redacted() {
    // Some gateways echo the request back in the error, including the header.
    let server = MockServer::start(vec![(
        "/v1/messages".into(),
        Response::json(
            401,
            r#"{"error":{"message":"rejected Authorization: Bearer sk-test-key-0123456789"}}"#,
        ),
    )]);
    let result = probe_authentication(&target(&server, AuthMode::Bearer)).await;
    let detail = result.detail.unwrap_or_default();
    assert!(
        !detail.contains("sk-test-key-0123456789"),
        "an upstream error body leaked the key: {detail}"
    );
}

// ---------------------------------------------------------------------------
// Model availability
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_model_missing_from_the_list_is_reported_as_unknown() {
    // A models endpoint that answers 200 with a list that does not include the
    // requested model is the clearest possible "no such model".
    let server = MockServer::start(vec![(
        "/v1/models".into(),
        Response::json(
            200,
            r#"{"data":[{"id":"other-model"},{"id":"another-model"}]}"#,
        ),
    )]);
    let result = probe_model(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::UnknownModel,
        "detail: {:?}",
        result.detail
    );
}

#[tokio::test]
async fn a_model_present_in_the_list_is_supported() {
    let server = MockServer::start(vec![(
        "/v1/models".into(),
        Response::json(200, r#"{"data":[{"id":"test-model"}]}"#),
    )]);
    let result = probe_model(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::Supported,
        "{:?}",
        result.detail
    );
}

#[tokio::test]
async fn an_endpoint_without_a_models_list_is_not_a_failure() {
    // Many gateways do not implement /v1/models at all. That must not be
    // recorded as incompatible; the protocol probe is the real test.
    let server = MockServer::start(vec![(
        "/v1/models".into(),
        Response::json(404, r#"{"error":{"message":"not found"}}"#),
    )]);
    let result = probe_model(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::Supported,
        "{:?}",
        result.detail
    );
}

#[tokio::test]
async fn a_rate_limit_is_reported_as_such() {
    let server = MockServer::start(vec![(
        "/v1/models".into(),
        Response::json(429, r#"{"error":{"message":"rate limit exceeded"}}"#),
    )]);
    let result = probe_model(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(result.outcome, ProbeOutcome::RateLimited);
}

#[tokio::test]
async fn a_server_error_is_reported_as_a_provider_fault() {
    let server = MockServer::start(vec![(
        "/v1/models".into(),
        Response::json(500, r#"{"error":{"message":"internal"}}"#),
    )]);
    let result = probe_model(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(result.outcome, ProbeOutcome::ServerError);
}

#[tokio::test]
async fn an_unauthorized_models_request_is_reported_as_unauthorized() {
    let server = MockServer::start(vec![(
        "/v1/models".into(),
        Response::json(401, r#"{"error":{"message":"bad key"}}"#),
    )]);
    let result = probe_model(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(result.outcome, ProbeOutcome::Unauthorized);
}

#[tokio::test]
async fn malformed_json_from_the_models_endpoint_does_not_claim_the_model_was_listed() {
    // A 200 that is not JSON cannot be parsed, so Bridge must not report that the
    // model appeared in a model list — that would be inventing evidence. It may
    // still treat the endpoint as reachable, which is a real thing the 200
    // proves, so the assertion pins exactly that distinction.
    //
    // The earlier version of this test was named
    // `malformed_json_..._is_not_treated_as_success` but asserted
    // `detail.is_none() || !detail.contains("lists")` — a disjunction that the
    // production `Supported` fallback satisfies through its first branch. The
    // name promised a stronger property than the body checked, and a change that
    // started trusting any 200 body would not have failed it.
    let server = MockServer::start(vec![(
        "/v1/models".into(),
        Response::json(200, "this is not json at all"),
    )]);
    let result = probe_model(&target(&server, AuthMode::Bearer)).await;

    // Whatever the outcome, Bridge must never assert the model was listed.
    let detail = result.detail.clone().unwrap_or_default();
    assert!(
        !detail.to_lowercase().contains("listed"),
        "a non-JSON body cannot prove the model was listed: {detail:?}"
    );
    assert!(
        !detail.to_lowercase().contains("found"),
        "a non-JSON body cannot prove the model was found: {detail:?}"
    );
    // And the outcome must be one of the defined variants.
    assert!(
        matches!(
            result.outcome,
            ProbeOutcome::Supported
                | ProbeOutcome::Unsupported
                | ProbeOutcome::UnknownModel
                | ProbeOutcome::Unreachable
                | ProbeOutcome::TimedOut
        ),
        "unexpected outcome for a non-JSON 200: {:?}",
        result.outcome
    );
}

// ---------------------------------------------------------------------------
// Protocol probes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn codex_protocol_requires_the_responses_endpoint() {
    let server = MockServer::start(vec![(
        "/responses".into(),
        Response::json(200, OK_RESPONSES),
    )]);
    let result = probe_codex_protocol(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::Supported,
        "{:?}",
        result.detail
    );

    let sent = server.requests();
    assert!(
        sent.iter()
            .any(|request| request.path.contains("/responses")),
        "the Codex probe must call /responses, got {:?}",
        sent.iter().map(|r| &r.path).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn codex_protocol_reports_unsupported_when_responses_is_absent() {
    // A chat-only gateway returns 404 for /responses, which means the endpoint
    // exists but does not speak Codex's protocol.
    let server = MockServer::start(vec![(
        "/responses".into(),
        Response::json(404, r#"{"error":{"message":"not found"}}"#),
    )]);
    let result = probe_codex_protocol(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(result.outcome, ProbeOutcome::Unsupported);
}

#[tokio::test]
async fn claude_protocol_requires_the_messages_endpoint() {
    let server = MockServer::start(vec![
        // The Anthropic probe sends a deliberately tiny request; a 400 about the
        // request shape still proves the endpoint exists.
        ("/v1/messages".into(), Response::json(200, OK_MESSAGES)),
    ]);
    let result = probe_claude_protocol(&target(&server, AuthMode::XApiKey)).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::Supported,
        "{:?}",
        result.detail
    );

    let sent = server.requests();
    assert!(
        sent.iter()
            .any(|request| request.path.contains("/v1/messages")),
        "the Claude probe must call /v1/messages"
    );
    // Anthropic-shaped requests require the version header.
    let request = sent
        .iter()
        .find(|request| request.path.contains("/v1/messages"))
        .unwrap();
    assert_eq!(
        request.headers.get("anthropic-version").map(String::as_str),
        Some("2023-06-01")
    );
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_streaming_reply_is_recognized() {
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"re\"}}]}\n\n\
               data: {\"choices\":[{\"delta\":{\"content\":\"ady\"}}]}\n\n\
               data: [DONE]\n\n";
    // The Anthropic streaming path is /v1/messages.
    let server = MockServer::start(vec![("/v1/messages".into(), Response::sse(sse))]);
    let result = probe_streaming(&target(&server, AuthMode::XApiKey), false).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::Supported,
        "{:?}",
        result.detail
    );
    assert!(result.streamed, "the SSE stream was not detected");
}

#[tokio::test]
async fn a_non_streaming_reply_is_not_claimed_as_streaming() {
    let server = MockServer::start(vec![(
        "/v1/messages".into(),
        Response::json(200, OK_MESSAGES),
    )]);
    let result = probe_streaming(&target(&server, AuthMode::XApiKey), false).await;
    // A plain JSON body means streaming is unsupported, and the pipeline records
    // that as a warning rather than a failure.
    assert!(
        !result.streamed,
        "a buffered JSON reply must not be reported as streamed"
    );
}

// ---------------------------------------------------------------------------
// Tool calling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_tool_call_in_the_reply_is_detected() {
    let body = r#"{"content":[{"type":"tool_use","id":"toolu_1","name":"get_weather",
"input":{"city":"Jakarta"}}]}"#;
    let server = MockServer::start(vec![("/v1/messages".into(), Response::json(200, body))]);
    let result = probe_tool_calling(&target(&server, AuthMode::XApiKey), false).await;
    assert_eq!(
        result.outcome,
        ProbeOutcome::Supported,
        "{:?}",
        result.detail
    );
    assert!(result.tool_called, "the tool call was not detected");
}

#[tokio::test]
async fn a_plain_reply_is_not_claimed_as_tool_calling() {
    let server = MockServer::start(vec![(
        "/v1/messages".into(),
        Response::json(200, OK_MESSAGES),
    )]);
    let result = probe_tool_calling(&target(&server, AuthMode::XApiKey), false).await;
    assert!(
        !result.tool_called,
        "a text-only reply must not be reported as a tool call"
    );
}

#[tokio::test]
async fn an_endpoint_that_rejects_tools_is_unsupported() {
    let server = MockServer::start(vec![(
        "/v1/messages".into(),
        Response::json(400, r#"{"error":{"message":"tools are not supported"}}"#),
    )]);
    let result = probe_tool_calling(&target(&server, AuthMode::XApiKey), false).await;
    // Pin the exact outcome. `assert_ne!(.., Supported)` was satisfied by all
    // seven other variants, so a 400 misclassified as `Unauthorized` — telling a
    // user their working key was rejected and sending them to re-enter it — would
    // have passed.
    assert_eq!(result.outcome, ProbeOutcome::Unsupported);
}

// ---------------------------------------------------------------------------
// Security properties of the transport itself
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_redirect_is_not_followed_so_the_key_is_never_forwarded() {
    // A malicious or misconfigured gateway could redirect to an attacker's host
    // and receive the Authorization header. Redirects are disabled outright, so
    // the 302 itself is the response and no second request is made.
    let server = MockServer::start(vec![(
        "/v1/messages".into(),
        Response {
            status: 302,
            body: String::new(),
            content_type: "text/plain",
        },
    )]);
    let result = probe_authentication(&target(&server, AuthMode::Bearer)).await;

    // Whatever the classification, exactly one request must have been sent.
    assert_eq!(
        server.requests().len(),
        1,
        "a redirect was followed; an Authorization header could have leaked"
    );
    assert_ne!(result.outcome, ProbeOutcome::Supported);
}

#[tokio::test]
async fn the_probe_sends_a_json_body_naming_the_model() {
    let server = MockServer::start(vec![(
        "/v1/messages".into(),
        Response::json(200, OK_MESSAGES),
    )]);
    let _ = probe_authentication(&target(&server, AuthMode::Bearer)).await;

    let sent = server.requests();
    let request = sent.first().expect("a request was made");
    assert_eq!(request.method, "POST");
    let parsed: serde_json::Value =
        serde_json::from_str(&request.body).expect("the body must be JSON");
    assert_eq!(parsed["model"], "test-model");
}

#[tokio::test]
async fn a_very_large_error_body_is_truncated_rather_than_held_in_memory() {
    // The pipeline caps how much of an upstream body it keeps.
    let huge = format!(r#"{{"error":{{"message":"{}"}}}}"#, "x".repeat(400_000));
    let server = MockServer::start(vec![("/v1/models".into(), Response::json(500, &huge))]);
    let result = probe_model(&target(&server, AuthMode::Bearer)).await;
    assert_eq!(result.outcome, ProbeOutcome::ServerError);

    let detail = result.detail.unwrap_or_default();
    assert!(
        detail.len() <= 500,
        "a huge upstream body was passed through: {} bytes",
        detail.len()
    );
}

// ---------------------------------------------------------------------------
// Protocol capabilities, probed independently.
//
// These are the tests that distinguish a provider which speaks one API shape
// from one that speaks another. The user's two real providers are the models
// here: one answers Anthropic Messages, the other only the OpenAI shape.
// ---------------------------------------------------------------------------

use eclyzer_bridge_lib::profiles::AgentId;
use eclyzer_bridge_lib::testing::probes::{probe_protocols, Protocol};

/// A server that answers one route and 404s the rest.
///
/// The paths here are the **resolved endpoints** the probes actually request,
/// which was verified against Codex's own source rather than assumed:
///
/// * Codex builds its request from `url_for_path("/responses")` and its default
///   base URL is `https://api.openai.com/v1`, so the effective route is
///   `/v1/responses`. The probe goes through the same resolver activation uses,
///   so it requests exactly that.
/// * `wire_api = "chat"` is no longer supported by Codex at all; its source
///   rejects the value with an explicit error. Chat Completions is therefore
///   probed only to tell a Chat-only gateway apart from a broken one, never as a
///   way to serve Codex.
fn single_protocol_server(route: &str, body: &str) -> MockServer {
    let routes = [
        ("/v1/responses", OK_RESPONSES),
        ("/v1/chat/completions", OK_CHAT),
        ("/v1/messages", OK_MESSAGES),
    ]
    .into_iter()
    .map(|(path, payload)| {
        let response = if path == route {
            Response::json(200, body)
        } else {
            let _ = payload;
            // A 404 with a realistic body, matching what the user saw from their
            // gateway: "Route POST /v1/messages not found".
            Response::json(
                404,
                &format!(r#"{{"error":"Route POST {path} not found"}}"#),
            )
        };
        (path.to_string(), response)
    })
    .collect();
    MockServer::start(routes)
}

#[tokio::test]
async fn an_anthropic_only_gateway_reports_only_anthropic_messages() {
    // The `provider_a` shape: works for Claude Code, and the OpenAI routes 404.
    let server = single_protocol_server("/v1/messages", OK_MESSAGES);
    let caps = probe_protocols(&target(&server, AuthMode::XApiKey)).await;

    assert!(caps.supports(Protocol::AnthropicMessages));
    assert!(!caps.supports(Protocol::OpenAiResponses));
    assert!(!caps.supports(Protocol::OpenAiChatCompletions));
    assert!(caps.serves(AgentId::Claude));
    assert!(
        !caps.serves(AgentId::Codex),
        "an Anthropic-only endpoint must not be claimed for Codex"
    );
}

#[tokio::test]
async fn a_responses_only_gateway_reports_only_openai_responses() {
    // Note the path: Codex builds its request from `url_for_path("/responses")`
    // and its default base URL already ends in `/v1`, so the route here is the
    // bare `/responses` that the probe actually sends to a host without a path.
    let server = MockServer::start(vec![
        ("/responses".into(), Response::json(200, OK_RESPONSES)),
        (
            "/v1/chat/completions".into(),
            Response::json(404, r#"{"error":"not found"}"#),
        ),
        (
            "/v1/messages".into(),
            Response::json(404, r#"{"error":"not found"}"#),
        ),
    ]);
    let caps = probe_protocols(&target(&server, AuthMode::Bearer)).await;

    assert!(caps.supports(Protocol::OpenAiResponses));
    assert!(!caps.supports(Protocol::AnthropicMessages));
    assert!(caps.serves(AgentId::Codex));
    assert!(!caps.serves(AgentId::Claude));
}

#[tokio::test]
async fn a_chat_completions_only_gateway_serves_neither_agent() {
    // The case the task is explicit about: a Chat-Completions proxy must not be
    // reported as compatible with Codex or Claude Code, because neither agent
    // speaks that protocol and Bridge has no translation layer.
    let server = single_protocol_server("/v1/chat/completions", OK_CHAT);
    let caps = probe_protocols(&target(&server, AuthMode::Bearer)).await;

    assert!(caps.supports(Protocol::OpenAiChatCompletions));
    assert!(!caps.serves(AgentId::Codex));
    assert!(!caps.serves(AgentId::Claude));
    assert_eq!(caps.supported(), vec![Protocol::OpenAiChatCompletions]);
}

#[tokio::test]
async fn a_gateway_serving_two_protocols_reports_both() {
    let server = MockServer::start(vec![
        ("/responses".into(), Response::json(200, OK_RESPONSES)),
        ("/v1/messages".into(), Response::json(200, OK_MESSAGES)),
        (
            "/v1/chat/completions".into(),
            Response::json(404, r#"{"error":"not found"}"#),
        ),
    ]);
    let caps = probe_protocols(&target(&server, AuthMode::Bearer)).await;

    assert!(caps.supports(Protocol::OpenAiResponses));
    assert!(caps.supports(Protocol::AnthropicMessages));
    assert!(!caps.supports(Protocol::OpenAiChatCompletions));
    assert!(caps.multiple, "two protocols must be reported as multiple");
    assert!(caps.serves(AgentId::Codex));
    assert!(caps.serves(AgentId::Claude));
}

#[tokio::test]
async fn every_protocol_is_probed_even_when_the_first_one_fails() {
    // A capability matrix requires trying all three. Stopping at the first
    // failure would report a gateway as unsupported when it serves a different
    // API perfectly well.
    let server = single_protocol_server("/v1/messages", OK_MESSAGES);
    let _ = probe_protocols(&target(&server, AuthMode::XApiKey)).await;

    let routes: Vec<String> = server.requests().iter().map(|r| r.path.clone()).collect();
    // The resolved endpoints, not the raw protocol paths: the probe goes through
    // the shared resolver, so a bare host gets `/v1/responses` and the request
    // actually reaches the documented route.
    for route in ["/v1/responses", "/v1/chat/completions", "/v1/messages"] {
        assert!(
            routes.iter().any(|r| r == route),
            "{route} was never probed; saw {routes:?}"
        );
    }
}

#[tokio::test]
async fn the_probe_requests_the_resolved_endpoint_not_a_repeated_version() {
    // The defect that reached a user: a base URL ending in /v1 produced
    // /v1/v1/messages and their provider answered 404. The probe must send the
    // endpoint the resolver computes, which is also the one activation uses.
    let server = single_protocol_server("/v1/messages", OK_MESSAGES);
    let server_url = server.base_url.clone();
    let target = ProbeTarget::new(
        &format!("{server_url}/v1"),
        "test-model",
        Secret::new("sk-test"),
        AuthMode::XApiKey,
        Some(10_000),
    )
    .expect("target");

    let caps = probe_protocols(&target).await;
    assert!(caps.supports(Protocol::AnthropicMessages));

    let routes: Vec<String> = server.requests().iter().map(|r| r.path.clone()).collect();
    assert!(
        !routes.iter().any(|p| p.contains("/v1/v1/")),
        "a duplicated version was requested: {routes:?}"
    );
    for route in ["/v1/responses", "/v1/chat/completions", "/v1/messages"] {
        assert!(
            routes.iter().any(|r| r == route),
            "{route} missing; saw {routes:?}"
        );
    }
}

#[tokio::test]
async fn a_route_not_found_is_recorded_as_unsupported_not_as_an_auth_failure() {
    // A provider that does not serve a route has not rejected the credential.
    let server = single_protocol_server("/v1/messages", OK_MESSAGES);
    let caps = probe_protocols(&target(&server, AuthMode::XApiKey)).await;

    let responses = caps
        .results
        .iter()
        .find(|r| r.protocol == Protocol::OpenAiResponses)
        .expect("the Responses protocol is always probed");

    assert_eq!(responses.outcome, ProbeOutcome::Unsupported);
    assert_ne!(responses.outcome, ProbeOutcome::Unauthorized);
    assert_eq!(responses.route, "/v1/responses");
}

#[tokio::test]
async fn a_one_hundred_percent_unreachable_endpoint_reports_nothing_supported() {
    // Nothing listens on this port, so every capability must be Unreachable
    // rather than being optimistically reported.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let target = ProbeTarget::new(
        &format!("http://127.0.0.1:{port}"),
        "test-model",
        Secret::new("sk-test"),
        AuthMode::Bearer,
        Some(10_000),
    )
    .expect("target");

    let caps = probe_protocols(&target).await;
    assert!(caps.supported().is_empty());
    assert!(!caps.serves(AgentId::Codex));
    assert!(!caps.serves(AgentId::Claude));
    assert!(caps.summary().contains("did not answer"));
}

#[tokio::test]
async fn the_authentication_step_uses_the_route_the_agent_requires() {
    // The root cause of the reported EIAI failure: the authentication step always
    // sent an Anthropic-shaped request, so testing Codex asked the wrong route,
    // and testing Claude Code against an OpenAI-shaped provider produced a 404
    // that was reported as an authentication failure.
    use eclyzer_bridge_lib::testing::probes::{probe_authentication_for, Protocol};

    // A server that answers only the Responses route.
    let server = MockServer::start(vec![
        ("/v1/responses".into(), Response::json(200, OK_RESPONSES)),
        (
            "/v1/messages".into(),
            Response::json(404, r#"{"error":"Route POST /v1/messages not found"}"#),
        ),
    ]);

    // Codex: the credential is accepted, because the route it needs works.
    let codex = probe_authentication_for(
        &target(&server, AuthMode::Bearer),
        Protocol::OpenAiResponses,
    )
    .await;
    assert_eq!(codex.outcome, ProbeOutcome::Supported);

    // Claude Code against the same provider: the route is missing, which must be
    // Unsupported rather than Unauthorized.
    let claude = probe_authentication_for(
        &target(&server, AuthMode::Bearer),
        Protocol::AnthropicMessages,
    )
    .await;
    assert_eq!(claude.outcome, ProbeOutcome::Unsupported);
    assert_ne!(claude.outcome, ProbeOutcome::Unauthorized);
    let detail = claude.detail.unwrap_or_default();
    assert!(
        detail.to_lowercase().contains("protocol mismatch"),
        "the detail must not read like a credential problem: {detail}"
    );
    assert!(!detail.to_lowercase().contains("api key"), "{detail}");
}

#[tokio::test]
async fn a_rejected_key_is_still_reported_as_rejected_on_the_right_route() {
    // The fix must not swallow real authentication failures.
    use eclyzer_bridge_lib::testing::probes::{probe_authentication_for, Protocol};

    let server = MockServer::start(vec![(
        "/v1/responses".into(),
        Response::json(401, r#"{"error":{"message":"bad key"}}"#),
    )]);
    let result = probe_authentication_for(
        &target(&server, AuthMode::Bearer),
        Protocol::OpenAiResponses,
    )
    .await;
    assert_eq!(result.outcome, ProbeOutcome::Unauthorized);
}

#[tokio::test]
async fn the_playground_posts_to_the_route_each_agent_uses() {
    // The Playground had the same defect the test pipeline did: it treated "not
    // Claude" as Chat Completions, so a Codex profile was validated against a route
    // and body shape Codex cannot use. Codex removed `wire_api = "chat"` entirely
    // and builds its request from `url_for_path("/responses")`.
    use eclyzer_bridge_lib::profiles::ApiProtocol;
    use eclyzer_bridge_lib::testing::playground::send;

    let server = MockServer::start(vec![
        ("/v1/responses".into(), Response::json(200, OK_RESPONSES)),
        ("/v1/messages".into(), Response::json(200, OK_MESSAGES)),
        ("/v1/chat/completions".into(), Response::json(200, OK_CHAT)),
    ]);

    let target = ProbeTarget::new(
        &server.base_url,
        "test-model",
        Secret::new("sk-test"),
        AuthMode::Bearer,
        Some(10_000),
    )
    .expect("target");

    // Codex: the Responses route, and the reply is read with the Responses parser,
    // which finds the text inside `output` items rather than `choices`. The Chat
    // parser returned an empty string for this same body, which the caller reports
    // as "the provider returned an empty reply" for a perfectly good response.
    let codex = send(&target, "hello", ApiProtocol::OpenAiResponses)
        .await
        .expect("the playground call succeeds");
    assert_eq!(codex.text, "ready");

    // Claude Code: the Messages route.
    let claude = send(&target, "hello", ApiProtocol::AnthropicMessages)
        .await
        .expect("the playground call succeeds");
    assert_eq!(claude.text, "ready");

    let paths: Vec<String> = server.requests().iter().map(|r| r.path.clone()).collect();
    assert!(
        paths.iter().any(|p| p == "/v1/responses"),
        "Codex did not use the Responses route: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p == "/v1/messages"),
        "Claude Code did not use the Messages route: {paths:?}"
    );
    // Chat Completions serves neither agent, so it must never be requested.
    assert!(
        !paths.iter().any(|p| p.contains("chat/completions")),
        "the Playground used Chat Completions, which Codex removed support for and \
         Claude Code never used: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.contains("/v1/v1/")),
        "a duplicated version was requested: {paths:?}"
    );
}

#[tokio::test]
async fn the_playground_reports_a_provider_error_rather_than_an_empty_reply() {
    // A route the provider does not serve must produce a message naming the
    // endpoint's own response, not "the provider returned an empty reply", which
    // sends the user looking at the model instead of at the API shape.
    use eclyzer_bridge_lib::profiles::ApiProtocol;
    use eclyzer_bridge_lib::testing::playground::send;

    let server = MockServer::start(vec![(
        "/v1/responses".into(),
        Response::json(404, r#"{"error":"Route POST /v1/responses not found"}"#),
    )]);
    let target = ProbeTarget::new(
        &server.base_url,
        "test-model",
        Secret::new("sk-test"),
        AuthMode::Bearer,
        Some(10_000),
    )
    .expect("target");

    let err = send(&target, "hello", ApiProtocol::OpenAiResponses)
        .await
        .expect_err("a 404 must be an error");
    let text = format!("{} {}", err.message, err.hint.clone().unwrap_or_default());
    assert!(
        !text.to_lowercase().contains("empty reply"),
        "a missing route was reported as an empty reply: {text}"
    );
    // The endpoint's own words must survive into the error somewhere the user can
    // read them.
    let details = err.details.clone().unwrap_or_default();
    assert!(
        text.contains("not found") || details.contains("not found"),
        "the endpoint's own explanation was dropped: {text} / {details}"
    );
}

#[tokio::test]
async fn protocol_probing_does_not_leak_the_key_into_the_capability_report() {
    // The report is shown in the UI and can be copied into a diagnostics bundle.
    let server = single_protocol_server("/v1/messages", OK_MESSAGES);
    let caps = probe_protocols(&target(&server, AuthMode::XApiKey)).await;

    let serialised = serde_json::to_string(&caps).expect("the report serialises");
    assert!(!serialised.contains("sk-test-key"), "{serialised}");
    assert!(!serialised.contains("Authorization"), "{serialised}");
}

// ---------------------------------------------------------------------------
// The full compatibility pipeline, for the two real providers.
//
// These are the cases from the report. They exercise the whole 7-step pipeline
// against a local server, so the verdict, the step list, and the message the user
// reads are all covered — not just the individual probe.
// ---------------------------------------------------------------------------

use eclyzer_bridge_lib::profiles::{AdvancedOptions, CompatibilityStatus, Profile};
use eclyzer_bridge_lib::testing::pipeline::{test_profile, TestOutcome};

/// A profile pointed at a local mock server.
fn profile_for(base_url: &str, model: &str, agent: AgentId) -> Profile {
    let mut p = Profile::new(
        "test".to_string(),
        "test".to_string(),
        base_url.to_string(),
        model.to_string(),
        vec![agent],
    );
    p.advanced = AdvancedOptions {
        auth_mode: AuthMode::XApiKey,
        ..Default::default()
    };
    p
}

/// Every step's label and state, so a failure names exactly which step broke.
fn step_states(outcome: &TestOutcome) -> Vec<(String, String)> {
    outcome
        .steps
        .iter()
        .map(|step| (step.label.clone(), format!("{:?}", step.state)))
        .collect()
}

/// The detail attached to one step, or an empty string.
fn step_detail(outcome: &TestOutcome, id: &str) -> String {
    outcome
        .steps
        .iter()
        .find(|s| s.id == id)
        .and_then(|s| s.detail.clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn provider_a_shaped_provider_passes_every_claude_check() {
    // The provider that already worked: Anthropic Messages plus streaming and a
    // tool call. This is the regression guard for item 4 of the task, and it
    // requires the full seven steps to pass so the verdict is Compatible.
    //
    // A route table that satisfies every step of the pipeline for Claude Code:
    // a JSON reply for the protocol probe, an SSE stream for the streaming probe,
    // and a tool call for the tool probe. The qualified matchers are what let one
    // path answer three different probes.
    let server = MockServer::start(vec![
        (
            "/v1/messages?stream".into(),
            Response::sse(SSE_MESSAGES_TOOL),
        ),
        (
            "/v1/messages?tools".into(),
            Response::json(200, MESSAGES_TOOL_CALL),
        ),
        ("/v1/messages".into(), Response::json(200, OK_MESSAGES)),
        ("/v1/models".into(), Response::json(200, MODELS_OK)),
    ]);
    let profile = profile_for(&server.base_url, "example-model", AgentId::Claude);

    let outcome = test_profile(&profile, AgentId::Claude, Secret::new("sk-test"))
        .await
        .expect("the pipeline runs");

    let states = step_states(&outcome);
    assert_eq!(
        outcome.status,
        CompatibilityStatus::Compatible,
        "steps were {states:?}"
    );
    assert_eq!(outcome.steps.len(), 7, "steps were {states:?}");
    for (label, state) in &states {
        assert_eq!(state, "Passed", "{label} was {state}");
    }
    assert!(outcome.streaming, "steps were {states:?}");
    assert!(outcome.tool_calling, "steps were {states:?}");
}

#[tokio::test]
async fn provider_b_shaped_provider_is_reported_as_the_wrong_protocol_for_claude() {
    // The reported failure: a provider that serves the OpenAI shape and returns
    // "Route POST /v1/messages not found" for the Anthropic route. The pipeline
    // must not call this an authentication problem, and must say which protocol
    // the provider does serve.
    let server = MockServer::start(vec![
        (
            "/v1/messages".into(),
            Response::json(404, r#"{"error":"Route POST /v1/messages not found"}"#),
        ),
        ("/v1/chat/completions".into(), Response::json(200, OK_CHAT)),
        ("/v1/models".into(), Response::json(200, MODELS_OK)),
    ]);
    let profile = profile_for(
        &server.base_url,
        "vendor_a/example-model",
        AgentId::Claude,
    );

    let outcome = test_profile(&profile, AgentId::Claude, Secret::new("sk-test"))
        .await
        .expect("the pipeline runs");

    assert_ne!(
        outcome.status,
        CompatibilityStatus::Compatible,
        "an OpenAI-only provider must never be Compatible for Claude Code"
    );
    // A missing route is a protocol problem, so it must not read like a rejected
    // credential. `CompatibilityStatus` has no auth-specific variant, which is
    // itself the point: the advice must come from the message, not a status name.
    let reason_text = format!(
        "{} {}",
        outcome.reason_code.to_lowercase(),
        outcome.reason.to_lowercase()
    );
    assert!(
        !reason_text.contains("auth"),
        "a missing route was reported as an authentication failure: {reason_text}"
    );

    // The protocol step may not exist if an earlier step already failed, so the
    // detail is read from whichever step carried the explanation.
    let detail = step_detail(&outcome, "protocol");
    let all_steps: Vec<(String, String, Option<String>)> = outcome
        .steps
        .iter()
        .map(|s| (s.id.clone(), format!("{:?}", s.state), s.detail.clone()))
        .collect();
    assert!(
        detail.contains("Anthropic Messages"),
        "protocol detail was {detail:?}; all steps: {all_steps:#?}"
    );
    assert!(detail.contains("/v1/messages"), "{detail}");
    assert!(
        detail.contains("OpenAI Chat Completions"),
        "the note must name what the provider does serve: {detail}"
    );
    assert!(
        detail.to_lowercase().contains("does not translate"),
        "the note must say Bridge will not pretend: {detail}"
    );
    // And it must never read like a credential problem.
    assert!(!detail.to_lowercase().contains("api key"), "{detail}");
}

#[tokio::test]
async fn a_provider_serving_only_anthropic_is_not_claimed_for_codex() {
    // The mirror case: Codex requires Responses, and an Anthropic-only provider
    // must not be reported as usable by it.
    let server = MockServer::start(vec![
        ("/v1/messages".into(), Response::json(200, OK_MESSAGES)),
        (
            "/v1/responses".into(),
            Response::json(404, r#"{"error":"not found"}"#),
        ),
        ("/v1/models".into(), Response::json(200, MODELS_OK)),
    ]);
    let profile = profile_for(&server.base_url, "m", AgentId::Codex);

    let outcome = test_profile(&profile, AgentId::Codex, Secret::new("sk-test"))
        .await
        .expect("the pipeline runs");

    assert_ne!(
        outcome.status,
        CompatibilityStatus::Compatible,
        "an Anthropic-only provider must not be Compatible for Codex"
    );
    let protocol_step = outcome
        .steps
        .iter()
        .find(|s| s.id == "protocol")
        .expect("the protocol step ran");
    let detail = protocol_step.detail.clone().unwrap_or_default();
    assert!(detail.contains("OpenAI Responses"), "{detail}");
    assert!(
        detail.contains("Anthropic Messages"),
        "the note must name what the provider serves: {detail}"
    );
}

#[tokio::test]
async fn a_provider_serving_both_protocols_is_compatible_for_both() {
    let server = MockServer::start(vec![
        (
            "/v1/responses?stream".into(),
            Response::sse(SSE_RESPONSES_TOOL),
        ),
        (
            "/v1/responses?tools".into(),
            Response::json(200, RESPONSES_TOOL_CALL),
        ),
        ("/v1/responses".into(), Response::json(200, OK_RESPONSES)),
        (
            "/v1/messages?stream".into(),
            Response::sse(SSE_MESSAGES_TOOL),
        ),
        (
            "/v1/messages?tools".into(),
            Response::json(200, MESSAGES_TOOL_CALL),
        ),
        ("/v1/messages".into(), Response::json(200, OK_MESSAGES)),
        ("/v1/models".into(), Response::json(200, MODELS_OK)),
    ]);

    for agent in [AgentId::Codex, AgentId::Claude] {
        let profile = profile_for(&server.base_url, "dual-model", agent);
        let outcome = test_profile(&profile, agent, Secret::new("sk-test"))
            .await
            .expect("the pipeline runs");
        assert_eq!(
            outcome.status,
            CompatibilityStatus::Compatible,
            "{agent:?} failed: {:?}",
            step_states(&outcome)
        );
    }
}

#[tokio::test]
async fn the_pipeline_requests_the_resolved_route_for_each_agent() {
    // The endpoint the test uses must be the endpoint activation writes. A base
    // URL carrying /v1 is the case that broke, so it is the case tested.
    let server = MockServer::start(vec![
        ("/v1/responses".into(), Response::sse(SSE_RESPONSES_TOOL)),
        ("/v1/messages".into(), Response::sse(SSE_MESSAGES_TOOL)),
        ("/v1/models".into(), Response::json(200, MODELS_OK)),
    ]);
    let base = format!("{}/v1", server.base_url);

    for agent in [AgentId::Codex, AgentId::Claude] {
        let profile = profile_for(&base, "m", agent);
        let _ = test_profile(&profile, agent, Secret::new("sk-test")).await;
    }

    let paths: Vec<String> = server.requests().iter().map(|r| r.path.clone()).collect();
    assert!(
        !paths.iter().any(|p| p.contains("/v1/v1/")),
        "a duplicated version was requested: {paths:?}"
    );
    assert!(paths.iter().any(|p| p == "/v1/messages"), "{paths:?}");
    assert!(paths.iter().any(|p| p == "/v1/responses"), "{paths:?}");
    // The model list is not one of the three API routes, so it exercises the
    // auxiliary resolver. It doubled the version in an earlier revision and was
    // caught by this assertion rather than by inspection.
    assert!(
        paths.iter().any(|p| p == "/v1/models"),
        "the model list was requested at the wrong path: {paths:?}"
    );
}

#[tokio::test]
async fn a_gateway_serving_v1_in_its_base_url_is_probed_at_the_right_route() {
    // The /v1/v1 defect, exercised end to end. The mock server matches on the
    // path it receives, so a duplicated segment would 404 every route and this
    // test would report no capabilities at all.
    let server = single_protocol_server("/v1/messages", OK_MESSAGES);
    let base = format!("{}/v1", server.base_url);
    let target = ProbeTarget::new(
        &base,
        "test-model",
        Secret::new("sk-test"),
        AuthMode::XApiKey,
        Some(10_000),
    )
    .expect("target");

    let caps = probe_protocols(&target).await;
    assert!(
        caps.supports(Protocol::AnthropicMessages),
        "a base URL ending in /v1 must not produce /v1/v1"
    );

    let paths: Vec<String> = server.requests().iter().map(|r| r.path.clone()).collect();
    assert!(
        !paths.iter().any(|p| p.contains("/v1/v1/")),
        "a duplicated version segment was sent: {paths:?}"
    );
}
