//! Mandatory secret redaction (PRD §37.4, §22.4, §38).
//!
//! Every string that can reach a log file, a diagnostics bundle, or the UI
//! passes through this module. Redaction is:
//!
//! * **fail-safe** — unknown-shaped secrets are masked rather than leaked;
//! * **idempotent** — redacting twice equals redacting once, so layered call
//!   sites are safe;
//! * **context-aware** — it recognises header lines, query parameters, common
//!   key prefixes, and long opaque tokens.
//!
//! It deliberately never logs the full value: callers get `****abcd` style
//! fingerprints via [`fingerprint`].

use std::sync::OnceLock;

use regex::Regex;

/// Placeholder substituted for a fully redacted value.
pub const REDACTED: &str = "[REDACTED]";

/// Build the compiled pattern set once.
fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(Patterns::compile)
}

struct Patterns {
    /// `Authorization: Bearer xxx`, `Authorization: xxx`
    authorization_header: Regex,
    /// `X-Api-Key: xxx`, `api-key: xxx`
    api_key_header: Regex,
    /// Any `Cookie:`/`Set-Cookie:` header line.
    cookie_header: Regex,
    /// `?api_key=xxx`, `&token=xxx`, `&key=xxx`, `&access_token=xxx`
    secret_query_param: Regex,
    /// JSON/TOML-ish `"api_key": "xxx"` / `api_key = "xxx"` / `apiKey: 'xxx'`
    secret_assignment: Regex,
    /// Well-known provider key prefixes without surrounding context.
    known_key_prefix: Regex,
    /// A long unbroken base64/hex run that looks like an opaque credential.
    long_opaque_token: Regex,
    /// URL userinfo credentials: `scheme://user:pass@host`
    url_userinfo: Regex,
}

impl Patterns {
    fn compile() -> Self {
        // Header/assignment values may be quoted or bare. A bare value stops at
        // whitespace, a quote, a comma, a semicolon, and crucially at `&`, `#`,
        // and `?` — otherwise a header pattern would swallow the remainder of a
        // query string appearing later on the same line.
        let value = r#"(?P<val>[^\s"'`,;&#?]+|"[^"]*"|'[^']*')"#;
        Self {
            authorization_header: Regex::new(&format!(
                r"(?i)\b(authorization|proxy-authorization)(\s*[:=]\s*)(?:bearer\s+|basic\s+|token\s+)?{value}"
            ))
            .expect("authorization_header regex"),
            api_key_header: Regex::new(&format!(
                r#"(?i)\b(x-api-key|x-goog-api-key|api[-_]?key|apikey|anthropic[-_]?api[-_]?key|openai[-_]?api[-_]?key)(\s*[:=]\s*){value}"#
            ))
            .expect("api_key_header regex"),
            cookie_header: Regex::new(r"(?i)\b((?:set-)?cookie)(\s*[:=]\s*)[^\r\n]+")
                .expect("cookie_header regex"),
            // The value must not swallow the `&` or `#` that ends one query
            // parameter and begins the next, otherwise redaction would eat
            // unrelated parameters. `api[-_]?key` must precede the bare `key`
            // alternative, but the regex engine handles that via alternation
            // order inside the group.
            secret_query_param: Regex::new(
                r#"(?i)([?&](?:api[-_]?key|access[-_]?token|apikey|authorization|password|signature|secret|token|auth|key|sig)=)([^\s"'`,;#&]+)"#,
            )
            .expect("secret_query_param regex"),
            // Matches `"api_key": "…"`, `api_key = "…"`, `apiKey: '…'`, and the
            // bare `api_key=value` form. Three constraints keep it from
            // mangling text the query redactor already handled:
            //   * a leading boundary group captures the character before the key
            //     so `?api_key=` / `&api_key=` query parameters are skipped
            //     (the Rust regex crate has no lookbehind);
            //   * an unquoted value stops at `&`, `#`, or `;` so a query
            //     string's following parameters survive;
            //   * the key must start at a real token boundary.
            secret_assignment: Regex::new(
                r#"(?i)(^|[\s,{;\[])(["']?(?:api[-_]?key|apikey|access[-_]?token|refresh[-_]?token|client[-_]?secret|password|secret|bearer|token)["']?\s*[:=]\s*)(?:("[^"]*")|('[^']*')|([^\s"'`,;#&]+))"#,
            )
            .expect("secret_assignment regex"),
            known_key_prefix: Regex::new(
                r"\b(?:sk|rk|pk|gsk|xai|fw|hf|api|key|tok|pat|ghp|gho|github_pat|glpat|SG|AKIA)[-_][A-Za-z0-9_\-]{12,}",
            )
            .expect("known_key_prefix regex"),
            // A long unbroken base64/hex run that looks like an opaque
            // credential. The run must occupy a whole whitespace-delimited
            // token, so redaction never swallows an unrelated `&param=value`, a
            // URL path segment, or a model ID embedded in a longer string.
            //
            // The Rust `regex` crate has no lookaround, so the trailing
            // boundary is captured and re-emitted instead.
            long_opaque_token: Regex::new(r"(^|\s)([A-Za-z0-9+/_\-]{40,}={0,2})(\s|$)")
                .expect("long_opaque_token regex"),
            url_userinfo: Regex::new(r"(?i)\b([a-z][a-z0-9+.\-]*://)([^/\s:@]+):([^/\s@]+)@")
                .expect("url_userinfo regex"),
        }
    }
}

/// Redact every secret-shaped substring in `input`.
pub fn redact_text(input: &str) -> String {
    let p = patterns();
    let mut out = input.to_string();

    // Order matters: structured contexts first so their values are replaced
    // before the generic token sweep can leave partial fragments behind.
    out = p
        .authorization_header
        .replace_all(&out, |caps: &regex::Captures| {
            format!("{}{}{}", &caps[1], &caps[2], REDACTED)
        })
        .into_owned();

    out = p
        .cookie_header
        .replace_all(&out, |caps: &regex::Captures| {
            // `caps[0]` is the whole match; reconstruct "name: [REDACTED]" from
            // the two capture groups rather than indexing a group that the
            // pattern may not have produced.
            let whole = &caps[0];
            let name = &caps[1];
            let separator = &caps[2];
            let _ = whole;
            format!("{name}{separator}{REDACTED}")
        })
        .into_owned();

    out = p
        .api_key_header
        .replace_all(&out, |caps: &regex::Captures| {
            format!("{}{}{}", &caps[1], &caps[2], REDACTED)
        })
        .into_owned();

    out = p
        .secret_query_param
        .replace_all(&out, |caps: &regex::Captures| {
            format!("{}{}", &caps[1], REDACTED)
        })
        .into_owned();
    out = p
        .secret_assignment
        .replace_all(&out, |caps: &regex::Captures| {
            // Group 1 is the leading boundary, group 2 the key and separator.
            format!("{}{}{}", &caps[1], &caps[2], REDACTED)
        })
        .into_owned();

    out = p
        .url_userinfo
        .replace_all(&out, |caps: &regex::Captures| {
            format!("{}{}:{}@", &caps[1], REDACTED, REDACTED)
        })
        .into_owned();

    out = p.known_key_prefix.replace_all(&out, REDACTED).into_owned();

    out = p
        .long_opaque_token
        .replace_all(&out, |caps: &regex::Captures| {
            // Preserve both boundaries so tokens are not glued together.
            format!("{}{}{}", &caps[1], REDACTED, &caps[3])
        })
        .into_owned();

    collapse_repeated_redactions(&out)
}

/// Collapse `[REDACTED] [REDACTED] ...` runs produced by overlapping patterns.
///
/// Only whitespace may separate the placeholders. An earlier version used
/// `\s*`, which let the pattern jump across meaningful characters such as the
/// `&` in a query string and delete everything it spanned.
fn collapse_repeated_redactions(input: &str) -> String {
    static REPEATED: OnceLock<Regex> = OnceLock::new();
    let re = REPEATED.get_or_init(|| {
        Regex::new(r"(?:\[REDACTED\][ \t]+)+\[REDACTED\]").expect("repeated redaction regex")
    });
    re.replace_all(input, REDACTED).into_owned()
}

/// Produce a non-reversible display fingerprint such as `****abcd`.
///
/// Only the last four characters are exposed. Short secrets are masked
/// entirely so a 4-character key never round-trips in full.
pub fn fingerprint(secret: &str) -> String {
    let len = secret.chars().count();
    if len <= 8 {
        return "*".repeat(len.max(4));
    }
    let tail: String = secret
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("****{tail}")
}

/// Redact a `(key, value)` pair only when the key looks secret-bearing.
pub fn redact_header(key: &str, value: &str) -> String {
    let lowered = key.to_ascii_lowercase();
    let sensitive = lowered.contains("auth")
        || lowered.contains("key")
        || lowered.contains("token")
        || lowered.contains("secret")
        || lowered.contains("cookie")
        || lowered.contains("password");
    if sensitive {
        format!("{key}: {REDACTED}")
    } else {
        format!("{key}: {value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_authorization_header() {
        let out = redact_text("Authorization: Bearer sk-abcdef0123456789abcdef");
        assert!(!out.contains("abcdef0123456789"), "{out}");
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn redacts_api_key_header() {
        let out = redact_text("X-Api-Key: 9f8e7d6c5b4a39281706");
        assert!(!out.contains("9f8e7d6c5b4a39281706"));
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn redacts_secret_query_parameter() {
        let out = redact_text("GET https://api.example.com/v1/models?api_key=abc123secret&x=1");
        assert!(!out.contains("abc123secret"), "{out}");
        assert!(out.contains("x=1"), "non-secret params must survive: {out}");
    }

    #[test]
    fn redacts_json_assignment() {
        let out = redact_text(r#"{"api_key": "super-secret-value-here", "model": "gpt-x"}"#);
        assert!(!out.contains("super-secret-value-here"), "{out}");
        assert!(out.contains("gpt-x"), "model must survive: {out}");
    }

    #[test]
    fn redacts_toml_assignment() {
        let out = redact_text(r#"api_key = "sk-proj-aaaaaaaaaaaaaaaaaaaa""#);
        assert!(!out.contains("sk-proj-aaaaaaaaaaaaaaaaaaaa"), "{out}");
    }

    #[test]
    fn redacts_known_prefix_without_context() {
        let out = redact_text("using key sk-1234567890abcdefghij for the request");
        assert!(!out.contains("sk-1234567890abcdefghij"), "{out}");
    }

    #[test]
    fn redacts_url_userinfo() {
        let out = redact_text("https://user:hunter2@gateway.example.com/v1");
        assert!(!out.contains("hunter2"), "{out}");
        assert!(out.contains("gateway.example.com"));
    }

    #[test]
    fn redacts_cookie_header() {
        let out = redact_text("Cookie: session=abcdef123456; theme=dark");
        assert!(!out.contains("abcdef123456"), "{out}");
    }

    #[test]
    fn is_idempotent() {
        let once = redact_text("Authorization: Bearer sk-abcdef0123456789abcdef");
        let twice = redact_text(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn preserves_ordinary_diagnostics() {
        let input = "Codex v1.2.3 installed at C:\\Users\\Test User\\.codex\\config.toml";
        assert_eq!(redact_text(input), input);
    }

    #[test]
    fn fingerprint_never_reveals_short_secrets() {
        assert_eq!(fingerprint("abcd"), "****");
        assert_eq!(fingerprint("abcdefgh"), "********");
        assert_eq!(fingerprint("abcdefghijkl"), "****ijkl");
    }

    #[test]
    fn redact_header_only_touches_sensitive_names() {
        assert_eq!(
            redact_header("Content-Type", "application/json"),
            "Content-Type: application/json"
        );
        assert_eq!(
            redact_header("Authorization", "Bearer x"),
            "Authorization: [REDACTED]"
        );
    }

    #[test]
    fn model_ids_are_not_mangled() {
        // A model ID is long but not secret-shaped; it must survive so
        // diagnostics stay useful.
        let out = redact_text("model: deepseek-chat-v3-0324-preview");
        assert!(out.contains("deepseek-chat-v3-0324-preview"), "{out}");
    }
}
