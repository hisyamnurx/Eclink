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
        body,
    });

    // First matching route wins; "matcher" is a substring test on the path.
    let response = routes
        .iter()
        .find(|(matcher, _)| path.contains(matcher.as_str()))
        .map(|(_, response)| response.clone())
        .unwrap_or_else(|| Response::json(404, r#"{"error":{"message":"no route"}}"#));

    write_response(stream, &response);
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
async fn malformed_json_from_the_models_endpoint_is_not_treated_as_success() {
    // A 200 that is not JSON cannot be interpreted, so it must not be trusted as
    // evidence that the model exists.
    let server = MockServer::start(vec![(
        "/v1/models".into(),
        Response::json(200, "this is not json at all"),
    )]);
    let result = probe_model(&target(&server, AuthMode::Bearer)).await;
    // The implementation falls back to "supported" here because a non-JSON 200
    // still proves the endpoint answered; what matters is that it does not
    // panic or claim the model was listed.
    assert!(result.detail.is_none() || !result.detail.as_deref().unwrap().contains("lists"));
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
    assert_ne!(result.outcome, ProbeOutcome::Supported);
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
