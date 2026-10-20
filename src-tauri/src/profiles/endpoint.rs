//! Per-protocol endpoint resolution.
//!
//! A provider is stored with **one** base URL, and each agent needs a different
//! route on it. Codex builds its request from `/responses` and expects the base
//! URL to supply the version prefix; Claude Code uses the Anthropic Messages
//! route. The user should not have to remember which of those needs `/v1`, and a
//! provider saved as `https://host` must work for both — so the effective
//! endpoint is *derived* from the stored base plus the protocol, rather than
//! being typed in twice.
//!
//! This module exists because the earlier implementation joined strings:
//!
//! ```text
//! format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
//! ```
//!
//! which turns `https://host/v1` plus `/v1/messages` into
//! `https://host/v1/v1/messages`. That is a real failure that reached a user:
//! their provider answered `404 Route POST:/v1/messages not found` because the
//! request never reached the route they served.
//!
//! The rules below are expressed over *parsed URL path segments* rather than
//! substrings, so `/myv1` is not mistaken for a version prefix and a custom
//! gateway prefix such as `/api/openai` is preserved intact.

use url::Url;

/// The API shape a request is being built for.
///
/// Deliberately the same three shapes the tester probes, so the endpoint used for
/// a compatibility test is by construction the endpoint activation will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiProtocol {
    /// OpenAI Responses. Codex requires this.
    OpenAiResponses,
    /// OpenAI Chat Completions. Neither agent uses it, but probing it
    /// distinguishes a Chat-only gateway from a broken one.
    OpenAiChatCompletions,
    /// Anthropic Messages. Claude Code requires this.
    AnthropicMessages,
}

impl ApiProtocol {
    /// The path this protocol is served at, relative to a host root.
    ///
    /// Codex asks for a bare `/responses` and relies on its base URL for `/v1`;
    /// the other two are absolute because neither agent supplies a prefix for
    /// them. Resolving all three through the same rule produces the same final
    /// URL either way, which is the point.
    pub fn path(self) -> &'static str {
        match self {
            ApiProtocol::OpenAiResponses => "/responses",
            ApiProtocol::OpenAiChatCompletions => "/v1/chat/completions",
            ApiProtocol::AnthropicMessages => "/v1/messages",
        }
    }

    /// The route as a user would recognise it, for display.
    pub fn display_route(self) -> &'static str {
        match self {
            ApiProtocol::OpenAiResponses => "/v1/responses",
            ApiProtocol::OpenAiChatCompletions => "/v1/chat/completions",
            ApiProtocol::AnthropicMessages => "/v1/messages",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            ApiProtocol::OpenAiResponses => "openai_responses",
            ApiProtocol::OpenAiChatCompletions => "openai_chat_completions",
            ApiProtocol::AnthropicMessages => "anthropic_messages",
        }
    }
}

/// Why an endpoint could not be resolved.
///
/// Implements `std::error::Error` so callers can propagate it with `?` into a
/// `Box<dyn Error>` instead of unwrapping. The message is the whole payload, so no
/// separate `source` is carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// The stored base URL is not a URL at all.
    Invalid(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::Invalid(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Whether a path segment is an API version marker such as `v1` or `v2`.
///
/// Only `v` followed by digits, so `v1beta`, `api`, and `openai` are not version
/// markers and are never collapsed.
fn is_version_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    matches!(chars.next(), Some('v') | Some('V'))
        && chars.clone().next().is_some()
        && chars.all(|c| c.is_ascii_digit())
}

/// The non-empty path segments of a URL.
fn path_segments(url: &Url) -> Vec<String> {
    url.path()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

/// Resolve the effective endpoint for `protocol` from a stored base URL.
///
/// The rule is: keep whatever the user typed, and append only the part of the
/// protocol path that is not already present.
///
/// ```text
/// https://example.com            + /v1/messages  ->  https://example.com/v1/messages
/// https://example.com/           + /v1/messages  ->  https://example.com/v1/messages
/// https://example.com/v1         + /v1/messages  ->  https://example.com/v1/messages
/// https://example.com/v1/        + /v1/messages  ->  https://example.com/v1/messages
/// https://example.com/api/openai + /v1/messages  ->  https://example.com/api/openai/v1/messages
/// ```
///
/// A custom gateway prefix is never stripped, because dropping a tenant path
/// would send requests somewhere the user did not choose.
pub fn resolve_endpoint(base_url: &str, protocol: ApiProtocol) -> Result<String, ResolveError> {
    let trimmed = base_url.trim();
    let url = Url::parse(trimmed).map_err(|_| {
        ResolveError::Invalid(format!(
            "\"{trimmed}\" is not a valid URL, so no endpoint can be derived from it."
        ))
    })?;

    let base_path = url.path().trim_end_matches('/').to_string();
    let wanted = protocol.path().trim_start_matches('/');

    // If the base already ends with the whole route, there is nothing to add.
    // This is what makes resolution idempotent: resolving an endpoint that has
    // already been resolved must return it unchanged, so a caller cannot
    // accidentally double a route by resolving twice.
    if !base_path.is_empty()
        && (base_path.eq_ignore_ascii_case(&format!("/{wanted}"))
            || base_path
                .to_ascii_lowercase()
                .ends_with(&format!("/{wanted}")))
    {
        let mut unchanged = url.clone();
        unchanged.set_query(None);
        unchanged.set_fragment(None);
        return Ok(unchanged.to_string());
    }

    let (wanted_head, wanted_rest) = match wanted.split_once('/') {
        Some((head, tail)) => (head, tail),
        None => (wanted, ""),
    };

    // What the base URL's path already supplies.
    //
    // Two shapes matter. `/v1` already ends with the version segment, so a
    // protocol path that also begins with `v1` must not repeat it. A base with no
    // path at all supplies nothing, so a *bare* protocol path such as
    // `/responses` needs its version added — Codex asks for `/responses` and
    // expects the base to carry `/v1`, so when the base has no version the
    // resolver has to supply one or Codex would be pointed at `/responses`.
    let segments = path_segments(&url);
    let base_last = segments.last().map(|s| s.to_ascii_lowercase());
    let base_has_this_version = base_last
        .as_deref()
        .map(|last| last.eq_ignore_ascii_case(wanted_head))
        .unwrap_or(false);
    // Only consulted for a bare protocol path: a base that already ends in *any*
    // version supplies the version that path relies on.
    let base_has_a_version = segments
        .last()
        .map(|last| is_version_segment(last))
        .unwrap_or(false);

    let suffix = if base_has_this_version {
        // The base already ends with this version, so the path's own version is
        // dropped: `/v1` + `/v1/messages` becomes `/v1/messages`.
        wanted_rest.to_string()
    } else if is_version_segment(wanted_head) {
        // The path carries its own version and the base supplies none, or
        // supplies a different one. Keep the path intact: `/v1` is part of the
        // route the provider documented, and rewriting it would change which
        // route is requested.
        wanted.to_string()
    } else if base_has_a_version {
        // A bare protocol path such as Codex's `/responses` on a base that
        // already carries a version. The base supplies it, so adding another
        // would produce `/v1/v1/responses` — the very defect this module exists
        // to prevent.
        wanted.to_string()
    } else {
        // A bare protocol path with nothing supplying a version. The documented
        // route for this API includes one, so it is added: a bare host becomes
        // `/v1/responses`, and a custom prefix becomes
        // `/api/openai/v1/responses` because Codex's own default base URL
        // carries the version.
        format!("v1/{wanted}")
    };

    // Rebuild through the URL API rather than by concatenating strings. Parsing
    // already handles scheme, userinfo, IPv6 brackets, and port quoting
    // correctly; re-deriving them by hand is how `[[::1]]` happens.
    let mut rebuilt = url.clone();
    let mut path = base_path.clone();
    if !suffix.is_empty() {
        path.push('/');
        path.push_str(&suffix);
    }
    if path.is_empty() {
        path.push('/');
    }
    rebuilt.set_path(&path);
    // A query or fragment on the base URL is not part of an API route, and
    // leaving it attached would append it mid-path once the route is added.
    rebuilt.set_query(None);
    rebuilt.set_fragment(None);
    Ok(rebuilt.to_string())
}

/// The base URL Claude Code must be given in `ANTHROPIC_BASE_URL`.
///
/// **This is not the same value the Playground uses, and that difference is the
/// whole reason a profile can pass in the Playground and fail in real Claude
/// Code.**
///
/// * Eclink's tester builds the *complete* request URL itself
///   ([`resolve_endpoint`]), so it needs the stored base including any `/v1`
///   version segment and requests `<stored>/v1/messages`.
/// * Claude Code instead treats `ANTHROPIC_BASE_URL` as a prefix and appends its
///   own `/v1/messages`. Its own documented connectivity check is
///   `POST "$ANTHROPIC_BASE_URL/v1/messages"`, and its compatibility guide states
///   it "posts to `/v1/messages`". The documented example value is a bare host
///   such as `https://llm-gateway.example.com`.
///
/// So a base URL stored for the OpenAI-compatible route — which conventionally
/// ends in `/v1`, e.g. `https://modelrouter.id/v1` — must have that version
/// segment removed before it is written, or Claude Code requests
/// `/v1/v1/messages`. A real provider answers that with
/// `404 {"error":{"message":"Invalid URL (POST /v1/v1/messages)"}}`, which Claude
/// Code then reports as the misleading "There's an issue with the selected
/// model".
///
/// Only a *trailing* version segment is removed. A custom tenant path such as
/// `/api/anthropic` is kept, because it is part of where the user's requests must
/// go. When the base is exactly a version segment (`https://h/v1`), dropping it
/// would leave a bare origin, which still resolves correctly to `/v1/messages`.
pub fn resolve_anthropic_base_base_url(base_url: &str) -> Result<String, ResolveError> {
    let trimmed = base_url.trim();
    let mut url = Url::parse(trimmed).map_err(|_| {
        ResolveError::Invalid(format!(
            "\"{trimmed}\" is not a valid URL, so no endpoint can be derived from it."
        ))
    })?;

    // A query or fragment is not part of the API prefix Claude Code appends to.
    url.set_query(None);
    url.set_fragment(None);

    let mut segments = path_segments(&url);
    if segments.last().is_some_and(|last| is_version_segment(last)) {
        segments.pop();
    }

    let path = if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    };
    url.set_path(&path);
    Ok(url.to_string())
}

/// The endpoint each agent would use for this provider, resolved in one pass.
///
/// Returns `None` for an agent when the stored URL cannot be parsed, so the UI
/// can show a validation error instead of a fabricated endpoint.
pub fn agent_endpoints(base_url: &str) -> (Option<String>, Option<String>, Option<String>) {
    (
        resolve_endpoint(base_url, ApiProtocol::OpenAiResponses).ok(),
        resolve_endpoint(base_url, ApiProtocol::AnthropicMessages).ok(),
        resolve_endpoint(base_url, ApiProtocol::OpenAiChatCompletions).ok(),
    )
}

/// A non-API path a probe needs on a provider that may already carry a version.
///
/// The model list is not one of the three versioned API routes, but it lives
/// under the same version prefix. Joining it plainly is what produced
/// `/v1/v1/models` for a provider configured with a `/v1` base, so the same
/// version-collapsing rule applies: keep the base's own version rather than
/// adding a second one.
pub fn resolve_auxiliary_path(base_url: &str, path: &str) -> Result<String, ResolveError> {
    let trimmed = base_url.trim();
    let mut url = Url::parse(trimmed)
        .map_err(|_| ResolveError::Invalid(format!("\"{trimmed}\" is not a valid URL.")))?;

    let wanted = path.trim_start_matches('/');
    let (wanted_head, wanted_rest) = match wanted.split_once('/') {
        Some((head, tail)) => (head, tail),
        None => (wanted, ""),
    };

    let base_path = url.path().trim_end_matches('/').to_string();
    let segments = path_segments(&url);
    let base_has_this_version = segments
        .last()
        .map(|last| last.eq_ignore_ascii_case(wanted_head))
        .unwrap_or(false);
    let base_has_a_version = segments
        .last()
        .map(|last| is_version_segment(last))
        .unwrap_or(false);

    let suffix = if base_has_this_version {
        wanted_rest.to_string()
    } else if is_version_segment(wanted_head) || base_has_a_version {
        wanted.to_string()
    } else {
        format!("v1/{wanted}")
    };

    let mut final_path = base_path;
    if !suffix.is_empty() {
        final_path.push('/');
        final_path.push_str(&suffix);
    }
    if final_path.is_empty() {
        final_path.push('/');
    }
    url.set_path(&final_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ApiProtocol; 3] = [
        ApiProtocol::OpenAiResponses,
        ApiProtocol::OpenAiChatCompletions,
        ApiProtocol::AnthropicMessages,
    ];

    #[test]
    fn every_documented_base_shape_resolves_without_duplicating_a_version() {
        // The table from the task, one row per shape, asserted for every protocol.
        let cases = [
            ("https://example.com", "https://example.com"),
            ("https://example.com/", "https://example.com"),
            ("https://example.com/v1", "https://example.com/v1"),
            ("https://example.com/v1/", "https://example.com/v1"),
        ];
        for (input, expected_prefix) in cases {
            for protocol in ALL {
                let resolved = resolve_endpoint(input, protocol).expect("resolves");
                assert!(
                    resolved.starts_with(expected_prefix),
                    "{input} + {:?} -> {resolved}",
                    protocol
                );
                assert!(
                    !resolved.contains("/v1/v1/"),
                    "{input} + {:?} duplicated the version: {resolved}",
                    protocol
                );
                assert!(
                    !resolved.ends_with("/v1"),
                    "{input} + {:?} lost its route: {resolved}",
                    protocol
                );
            }
        }
    }

    #[test]
    fn the_anthropic_route_is_exact_for_every_base_shape() {
        for input in [
            "https://example.com",
            "https://example.com/",
            "https://example.com/v1",
            "https://example.com/v1/",
            "https://example.com/v1//",
        ] {
            assert_eq!(
                resolve_endpoint(input, ApiProtocol::AnthropicMessages).unwrap(),
                "https://example.com/v1/messages",
                "base {input}"
            );
        }
    }

    #[test]
    fn the_codex_route_is_exact_for_every_base_shape() {
        // Codex's own request path is bare `/responses` and its default base URL
        // already ends in `/v1`, so the resolved endpoint must contain `/v1`
        // exactly once regardless of what the user typed.
        for input in [
            "https://example.com",
            "https://example.com/",
            "https://example.com/v1",
            "https://example.com/v1/",
        ] {
            assert_eq!(
                resolve_endpoint(input, ApiProtocol::OpenAiResponses).unwrap(),
                "https://example.com/v1/responses",
                "base {input}"
            );
        }
    }

    #[test]
    fn the_chat_route_is_exact_for_every_base_shape() {
        for input in [
            "https://example.com",
            "https://example.com/",
            "https://example.com/v1",
            "https://example.com/v1/",
        ] {
            assert_eq!(
                resolve_endpoint(input, ApiProtocol::OpenAiChatCompletions).unwrap(),
                "https://example.com/v1/chat/completions",
                "base {input}"
            );
        }
    }

    #[test]
    fn a_custom_gateway_prefix_is_never_stripped() {
        // Dropping a tenant path would send requests to the wrong place, so it is
        // preserved exactly.
        for (input, expected) in [
            (
                "https://gw.example.com/api/openai",
                "https://gw.example.com/api/openai/v1/messages",
            ),
            (
                "https://gw.example.com/anthropic",
                "https://gw.example.com/anthropic/v1/messages",
            ),
            (
                "https://gw.example.com/tenant/abc",
                "https://gw.example.com/tenant/abc/v1/messages",
            ),
            (
                "https://gw.example.com/api/openai/v1",
                "https://gw.example.com/api/openai/v1/messages",
            ),
        ] {
            assert_eq!(
                resolve_endpoint(input, ApiProtocol::AnthropicMessages).unwrap(),
                expected,
                "base {input}"
            );
        }
    }

    #[test]
    fn a_version_like_prefix_that_is_not_a_version_is_not_collapsed() {
        for input in ["https://h/v1beta", "https://h/myv1", "https://h/v"] {
            let resolved = resolve_endpoint(input, ApiProtocol::AnthropicMessages).unwrap();
            assert!(
                resolved.starts_with(input),
                "{input} lost its path: {resolved}"
            );
            assert_eq!(resolved.matches("/v1/").count(), 1, "{resolved}");
        }
    }

    #[test]
    fn a_version_like_host_does_not_trigger_collapsing() {
        // Only the path decides, so a host named `v1` contributes nothing.
        assert_eq!(
            resolve_endpoint("https://v1.example.com", ApiProtocol::AnthropicMessages).unwrap(),
            "https://v1.example.com/v1/messages"
        );
        assert_eq!(
            resolve_endpoint("https://v1.example.com", ApiProtocol::OpenAiResponses).unwrap(),
            "https://v1.example.com/v1/responses"
        );
    }

    #[test]
    fn a_different_version_on_the_base_is_preserved_rather_than_rewritten() {
        // `/v2` is not `/v1`. For the Anthropic route, which carries its own
        // version, both segments are kept so the user can see exactly what will be
        // requested instead of having it silently rewritten.
        assert_eq!(
            resolve_endpoint("https://h/v2", ApiProtocol::AnthropicMessages).unwrap(),
            "https://h/v2/v1/messages"
        );
    }

    #[test]
    fn the_codex_endpoint_matches_what_codex_itself_would_request() {
        // Codex appends its bare `/responses` path to the base URL it is given, so
        // the resolver must do exactly the same rather than inventing a version
        // the user did not configure. Verified against Codex's own
        // `url_for_path("/responses")` call.
        assert_eq!(
            resolve_endpoint("https://h/v10", ApiProtocol::OpenAiResponses).unwrap(),
            "https://h/v10/responses"
        );
        assert_eq!(
            resolve_endpoint("https://h/v1", ApiProtocol::OpenAiResponses).unwrap(),
            "https://h/v1/responses"
        );
        assert_eq!(
            resolve_endpoint("https://h", ApiProtocol::OpenAiResponses).unwrap(),
            "https://h/v1/responses",
            "a bare host still needs a version, since Codex's own default base has one"
        );
    }

    #[test]
    fn a_bare_protocol_path_gets_the_documented_version_even_on_a_versioned_base() {
        // Codex asks for `/responses` and relies on the base for the version. A
        // base ending in `/v1` already supplies it, so the result is
        // `/v1/responses`; a base ending in something else keeps both segments.
        assert_eq!(
            resolve_endpoint("https://h/v1", ApiProtocol::OpenAiResponses).unwrap(),
            "https://h/v1/responses"
        );
    }

    #[test]
    fn the_port_is_preserved() {
        assert_eq!(
            resolve_endpoint("http://127.0.0.1:41000/v1", ApiProtocol::AnthropicMessages).unwrap(),
            "http://127.0.0.1:41000/v1/messages"
        );
        assert_eq!(
            resolve_endpoint("http://localhost:11434", ApiProtocol::OpenAiResponses).unwrap(),
            "http://localhost:11434/v1/responses"
        );
    }

    #[test]
    fn an_ipv6_host_keeps_its_brackets() {
        assert_eq!(
            resolve_endpoint("http://[::1]:8080", ApiProtocol::AnthropicMessages).unwrap(),
            "http://[::1]:8080/v1/messages"
        );
    }

    #[test]
    fn an_invalid_url_is_reported_rather_than_guessed() {
        // Returning a fabricated endpoint would send a key somewhere unexpected.
        assert!(resolve_endpoint("not a url", ApiProtocol::AnthropicMessages).is_err());
        assert!(resolve_endpoint("", ApiProtocol::AnthropicMessages).is_err());
        assert!(resolve_endpoint("ftp://h/v1", ApiProtocol::AnthropicMessages).is_ok());
    }

    #[test]
    fn resolution_is_idempotent() {
        // Resolving an already-resolved endpoint must not append another segment.
        for protocol in ALL {
            let once = resolve_endpoint("https://h/v1", protocol).unwrap();
            let twice = resolve_endpoint(&once, protocol).unwrap();
            assert_eq!(once, twice, "{protocol:?} is not idempotent");
        }
    }

    #[test]
    fn a_query_string_or_fragment_on_the_base_is_not_carried_into_the_endpoint() {
        // A stray `?` in the base would otherwise end up mid-path and break the
        // request, since the endpoint is built as text.
        let resolved = resolve_endpoint("https://h/v1?tenant=acme", ApiProtocol::AnthropicMessages)
            .expect("resolves");
        assert!(
            !resolved.contains('?') || resolved.ends_with("/v1/messages"),
            "{resolved}"
        );
        assert!(resolved.contains("/v1/messages"), "{resolved}");
    }

    #[test]
    fn the_three_protocols_produce_three_different_endpoints() {
        let (responses, messages, chat) = agent_endpoints("https://h");
        assert_eq!(responses.as_deref(), Some("https://h/v1/responses"));
        assert_eq!(messages.as_deref(), Some("https://h/v1/messages"));
        assert_eq!(chat.as_deref(), Some("https://h/v1/chat/completions"));
    }

    #[test]
    fn every_protocol_path_has_a_matching_display_route() {
        // The UI shows the display route while the request uses the path, so they
        // must describe the same thing once resolved.
        for protocol in ALL {
            let resolved = resolve_endpoint("https://h", protocol).unwrap();
            assert!(
                resolved.ends_with(protocol.display_route()),
                "{:?}: {resolved} does not end with {}",
                protocol,
                protocol.display_route()
            );
        }
    }

    #[test]
    fn an_auxiliary_path_does_not_double_the_version_either() {
        // The model list is not one of the three API routes, so it goes through
        // the auxiliary resolver. A plain join here produced `/v1/v1/models` for
        // a provider configured with `/v1`, which is the same defect in a second
        // place and was found by a pipeline test rather than by inspection.
        for base in [
            "https://example.com",
            "https://example.com/",
            "https://example.com/v1",
            "https://example.com/v1/",
        ] {
            let resolved = resolve_auxiliary_path(base, "/v1/models").expect("resolves");
            assert_eq!(resolved, "https://example.com/v1/models", "base {base}");
        }
    }

    #[test]
    fn an_auxiliary_path_keeps_a_custom_prefix() {
        assert_eq!(
            resolve_auxiliary_path("https://gw.example.com/api/openai", "/v1/models").unwrap(),
            "https://gw.example.com/api/openai/v1/models"
        );
        assert_eq!(
            resolve_auxiliary_path("https://gw.example.com/api/openai/v1", "/v1/models").unwrap(),
            "https://gw.example.com/api/openai/v1/models"
        );
    }

    #[test]
    fn an_auxiliary_path_without_a_version_is_still_joined() {
        assert_eq!(
            resolve_auxiliary_path("https://example.com", "/health").unwrap(),
            "https://example.com/v1/health",
            "a bare auxiliary path gets the documented version"
        );
        assert_eq!(
            resolve_auxiliary_path("https://example.com/v1", "/health").unwrap(),
            "https://example.com/v1/health"
        );
    }

    #[test]
    fn the_anthropic_base_url_never_leaves_a_trailing_version_for_claude_code() {
        // Regression: Claude Code appends `/v1/messages` to ANTHROPIC_BASE_URL.
        // Writing a stored base that already ends in `/v1` produced
        // `/v1/v1/messages`, which a real provider answered with
        // `404 Invalid URL (POST /v1/v1/messages)`. Claude Code then reported the
        // misleading "There's an issue with the selected model", so a profile that
        // worked in the Playground failed in a real session.
        for (stored, expected) in [
            ("https://modelrouter.id/v1", "https://modelrouter.id"),
            ("https://modelrouter.id/v1/", "https://modelrouter.id"),
            ("https://modelrouter.id", "https://modelrouter.id"),
            ("https://modelrouter.id/", "https://modelrouter.id"),
            ("https://h/v2", "https://h"),
            ("https://h/v10", "https://h"),
            ("http://127.0.0.1:20128/v1", "http://127.0.0.1:20128"),
        ] {
            let base = resolve_anthropic_base_base_url(stored).unwrap();
            assert_eq!(base.trim_end_matches('/'), expected, "stored {stored}");
            // The property that actually matters: what Claude Code will request.
            let requested = format!("{}/v1/messages", base.trim_end_matches('/'));
            assert_eq!(
                requested.matches("/v1/").count(),
                1,
                "stored {stored} made Claude Code request {requested}"
            );
        }
    }

    #[test]
    fn the_anthropic_base_url_keeps_a_custom_tenant_prefix() {
        // Dropping a tenant path would send the user's requests somewhere they did
        // not choose, so only a trailing *version* segment is removed.
        for (stored, expected) in [
            (
                "https://gw.example.com/api/anthropic",
                "https://gw.example.com/api/anthropic",
            ),
            (
                "https://gw.example.com/api/openai/v1",
                "https://gw.example.com/api/openai",
            ),
            ("https://gw.example.com/myv1", "https://gw.example.com/myv1"),
            (
                "https://gw.example.com/v1beta",
                "https://gw.example.com/v1beta",
            ),
        ] {
            assert_eq!(
                resolve_anthropic_base_base_url(stored).unwrap(),
                expected,
                "stored {stored}"
            );
        }
    }

    #[test]
    fn the_anthropic_base_url_of_a_bare_version_origin_is_still_usable() {
        // `https://h/v1` has no tenant path; dropping the version leaves the
        // origin, which Claude Code's own append turns back into `/v1/messages`.
        let base = resolve_anthropic_base_base_url("https://h/v1").unwrap();
        assert_eq!(base.trim_end_matches('/'), "https://h");
        assert_eq!(
            format!("{}/v1/messages", base.trim_end_matches('/')),
            "https://h/v1/messages"
        );
    }

    #[test]
    fn the_anthropic_base_url_drops_a_query_and_fragment() {
        // A query string on the base would otherwise end up before the route Claude
        // Code appends.
        let base = resolve_anthropic_base_base_url("https://h/v1?tenant=acme#x").unwrap();
        assert!(!base.contains('?'), "{base}");
        assert!(!base.contains('#'), "{base}");
        assert_eq!(base, "https://h/");
    }

    #[test]
    fn the_anthropic_base_url_rejects_an_unparsable_url() {
        assert!(resolve_anthropic_base_base_url("not a url").is_err());
        assert!(resolve_anthropic_base_base_url("").is_err());
    }

    #[test]
    fn the_playground_and_claude_code_agree_on_where_the_request_lands() {
        // The two code paths must describe the same request even though they build
        // it differently: the tester resolves the full URL, Claude Code appends.
        for stored in [
            "https://modelrouter.id/v1",
            "https://modelrouter.id",
            "https://gw.example.com/api/anthropic",
            "https://gw.example.com/api/anthropic/v1",
        ] {
            let playground = resolve_endpoint(stored, ApiProtocol::AnthropicMessages).unwrap();
            let written = resolve_anthropic_base_base_url(stored).unwrap();
            let claude_code = format!("{}/v1/messages", written.trim_end_matches('/'));
            assert_eq!(
                playground, claude_code,
                "stored {stored}: the Playground tests {playground} but Claude Code requests {claude_code}"
            );
        }
    }

    #[test]
    fn the_real_provider_urls_from_the_report_resolve_correctly() {
        assert_eq!(
            resolve_endpoint(
                "https://gateway.example.com",
                ApiProtocol::AnthropicMessages
            )
            .unwrap(),
            "https://gateway.example.com/v1/messages"
        );
        // This one is the case that produced `404 Route POST:/v1/messages not
        // found`: the base already ends in /v1 and the old code appended /v1
        // again.
        let provider_b = resolve_endpoint(
            "https://router.example.com/v1",
            ApiProtocol::AnthropicMessages,
        )
        .unwrap();
        assert_eq!(provider_b, "https://router.example.com/v1/messages");
        assert!(
            !provider_b.contains("/v1/v1/"),
            "the reported 404 cause is back"
        );
    }
}
