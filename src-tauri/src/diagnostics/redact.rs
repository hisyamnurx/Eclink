//! Mandatory secret redaction (PRD §37.4, §22.4, §38).
//!
//! Every string that can reach a log file, a diagnostics bundle, or the UI
//! passes through this module. Redaction is:
//!
//! * **layered** — applied when a line is written, again when logs are read
//!   back, and again when a diagnostics report is assembled, so a single missed
//!   call site does not leak;
//! * **idempotent** — redacting twice equals redacting once, which is what makes
//!   those layers safe to stack;
//! * **context-aware** — it recognises header lines, query parameters, common
//!   key prefixes, JSON Web Tokens, and long opaque tokens.
//!
//! It deliberately never logs the full value: callers get `****abcd` style
//! fingerprints via [`fingerprint`].
//!
//! ## What this module does *not* guarantee
//!
//! This is pattern matching, not a guarantee. A secret whose shape matches none
//! of the rules below survives redaction. Known gaps, stated plainly so nobody
//! relies on more than is delivered:
//!
//! * a short, unprefixed secret assigned to a field whose name does not look
//!   sensitive (for example `{"session":"abc123"}`) is not detected;
//! * a secret split across lines, or pasted with no separator into surrounding
//!   text, may fall outside a rule's boundaries.
//!
//! In practice the secrets this application handles are provider API keys, which
//! the [`known_key_prefix`](Patterns) rule and the header rules do cover. The
//! defence that matters more is structural: [`Secret`](crate::testing::Secret)
//! cannot be logged or serialized at all, so a key never reaches this module
//! through the normal path in the first place.

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
    /// A JSON Web Token: three base64url segments separated by dots.
    ///
    /// Without this rule a JWT escapes both the prefix rule (its first segment
    /// begins `eyJ`, not `sk-`) and the long-token rule (that rule's character
    /// class excludes `.`, so a dotted string never occupies a single token).
    json_web_token: Regex,
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
            // A JWT. Each segment is base64url; the header segment of a real JWT
            // always starts `eyJ`, so requiring that keeps false positives down
            // while still matching tokens with an unusually short payload.
            json_web_token: Regex::new(
                r"\beyJ[A-Za-z0-9_\-]{4,}\.[A-Za-z0-9_\-]{4,}\.[A-Za-z0-9_\-]{4,}",
            )
            .expect("json_web_token regex"),
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

    // JWTs carry dots, so the long-token rule below cannot see them. Run this
    // before that rule so a JWT is replaced whole rather than in fragments.
    out = p.json_web_token.replace_all(&out, REDACTED).into_owned();

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

    #[test]
    fn redacts_a_json_web_token() {
        // A JWT previously escaped both rules: its header segment starts `eyJ`
        // rather than a provider prefix, and the long-token rule's character
        // class excludes `.`, so a dotted token never occupies a whole token.
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9\
                   .eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIn0\
                   .SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let out = redact_text(&format!("auth failed for token {jwt}"));
        assert!(
            !out.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"),
            "{out}"
        );
        assert!(!out.contains("SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV"), "{out}");
        assert!(out.contains("[REDACTED]"), "{out}");
    }

    #[test]
    fn redacts_a_jwt_in_a_header_value() {
        // The overlapping header rule and JWT rule must not leave a fragment.
        let out = redact_text(
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhYmMifQ.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        );
        assert!(!out.contains("dBjftJeZ4CVPmB92K27uhbUJU1p1r"), "{out}");
    }

    #[test]
    fn a_jwt_shaped_token_is_redacted_repeatedly_without_growing() {
        let jwt =
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhYmMifQ.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let once = redact_text(&format!("token={jwt}"));
        let twice = redact_text(&once);
        assert_eq!(once, twice, "redaction must stay idempotent for a JWT");
    }

    #[test]
    fn the_documented_gap_is_really_a_gap() {
        // This test exists to keep the module documentation honest. If a future
        // change makes short unprefixed values detectable, this test fails and
        // whoever made the change should update the "what this module does not
        // guarantee" section rather than delete the test.
        let out = redact_text(r#"{"session":"abc123"}"#);
        assert!(
            out.contains("abc123"),
            "the documented limitation no longer holds; update the module docs"
        );
    }
}
