//! Provider profile model and persistence (PRD §18, §44.1).
//!
//! **Invariant:** `profiles.json` never contains an API key. Profiles reference
//! a secret by opaque ID (`secretRef`); the value lives in the DPAPI-sealed
//! `secrets.dat`.

use serde::{Deserialize, Serialize};

/// Agent a profile can be activated for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AgentId {
    Codex,
    Claude,
}

impl AgentId {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentId::Codex => "codex",
            AgentId::Claude => "claude",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            AgentId::Codex => "Codex",
            AgentId::Claude => "Claude Code",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "codex" => Some(AgentId::Codex),
            "claude" | "claude_code" | "claudecode" => Some(AgentId::Claude),
            _ => None,
        }
    }

    pub fn all() -> [AgentId; 2] {
        [AgentId::Codex, AgentId::Claude]
    }
}

/// Compatibility verdict for one (profile, agent) pair (PRD §21.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityStatus {
    Untested,
    Testing,
    Compatible,
    PartiallyCompatible,
    Incompatible,
    Unknown,
    TestFailed,
}

impl CompatibilityStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CompatibilityStatus::Untested => "untested",
            CompatibilityStatus::Testing => "testing",
            CompatibilityStatus::Compatible => "compatible",
            CompatibilityStatus::PartiallyCompatible => "partially_compatible",
            CompatibilityStatus::Incompatible => "incompatible",
            CompatibilityStatus::Unknown => "unknown",
            CompatibilityStatus::TestFailed => "test_failed",
        }
    }

    /// Whether a profile in this state may be activated for the agent.
    pub fn allows_activation(self) -> bool {
        matches!(
            self,
            CompatibilityStatus::Compatible | CompatibilityStatus::PartiallyCompatible
        )
    }
}

/// Cached compatibility result for one agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityRecord {
    pub status: CompatibilityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tested_at: Option<String>,
    /// Last measured round-trip latency in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Whether the endpoint streamed SSE successfully.
    #[serde(default)]
    pub streaming: bool,
    /// Whether a tool/function call round-tripped.
    #[serde(default)]
    pub tool_calling: bool,
    /// Short machine reason, e.g. `responses_unsupported`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    /// Human-readable reason, already redacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Informational warning, e.g. non-Claude upstream behind a gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// Values the result was computed against, used to detect staleness
    /// (PRD §45).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_key: Option<String>,
}

impl Default for CompatibilityRecord {
    fn default() -> Self {
        Self {
            status: CompatibilityStatus::Untested,
            tested_at: None,
            latency_ms: None,
            streaming: false,
            tool_calling: false,
            reason_code: None,
            reason: None,
            warning: None,
            stale_key: None,
        }
    }
}

/// Advanced, collapsed settings (PRD §18.3).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedOptions {
    /// Extra request headers sent during tests and playground calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HeaderPair>,
    /// Extra query parameters appended to requests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query_params: Vec<HeaderPair>,
    /// Per-request timeout override in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// How the credential is presented on the wire.
    #[serde(default)]
    pub auth_mode: AuthMode,
    /// Accept a plaintext HTTP endpoint after explicit user confirmation
    /// (PRD §50.1). Persisted so the confirmation is not re-asked silently.
    #[serde(default)]
    pub allow_insecure_endpoint: bool,
    /// Model alias override, when a gateway needs a different wire name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_alias: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// `Authorization: Bearer <key>` — the default for both agents.
    #[default]
    Bearer,
    /// `x-api-key: <key>` — Anthropic-style header.
    XApiKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderPair {
    pub name: String,
    pub value: String,
}

/// Headers a user may never override from the profile editor.
///
/// These carry the credential. Allowing a profile to set them would let a
/// profile silently replace the authenticated key — or point it at a host the
/// user did not choose. The check is applied wherever user headers are consumed,
/// not only in one code path, because a filter that lives in one caller is a
/// filter the next caller forgets.
pub const RESERVED_HEADERS: [&str; 3] = ["authorization", "x-api-key", "proxy-authorization"];

/// Whether `name` is a credential-bearing header the user may not override.
pub fn is_reserved_header(name: &str) -> bool {
    let lowered = name.trim().to_ascii_lowercase();
    RESERVED_HEADERS.contains(&lowered.as_str())
}

/// The user's extra headers, with blank names and reserved names removed.
///
/// Returns owned `(name, value)` pairs with the name trimmed, which is the shape
/// both the request pipeline and the Codex provider spec need.
pub fn user_headers(options: &AdvancedOptions) -> Vec<(String, String)> {
    options
        .headers
        .iter()
        .filter(|pair| !pair.name.trim().is_empty())
        .filter(|pair| !is_reserved_header(&pair.name))
        .map(|pair| (pair.name.trim().to_string(), pair.value.clone()))
        .collect()
}

/// A stored provider profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub provider_name: String,
    pub base_url: String,
    pub model: String,
    /// Opaque reference into the secret store. Never the key itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
    /// Agents this profile is intended for.
    pub targets: Vec<AgentId>,
    #[serde(default)]
    pub advanced: AdvancedOptions,
    /// Per-agent cached compatibility results.
    #[serde(default)]
    pub compatibility: std::collections::BTreeMap<AgentId, CompatibilityRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Profile {
    /// Build a new profile with a fresh ID and timestamps.
    pub fn new(
        name: String,
        provider_name: String,
        base_url: String,
        model: String,
        targets: Vec<AgentId>,
    ) -> Self {
        let now = chrono::Local::now().to_rfc3339();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            provider_name,
            base_url,
            model,
            secret_ref: None,
            targets,
            advanced: AdvancedOptions::default(),
            compatibility: Default::default(),
            notes: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Whether the profile declares support for `agent`.
    pub fn targets_agent(&self, agent: AgentId) -> bool {
        self.targets.contains(&agent)
    }

    /// Cached compatibility for `agent`.
    pub fn compatibility_for(&self, agent: AgentId) -> CompatibilityStatus {
        self.compatibility
            .get(&agent)
            .map(|record| record.status)
            .unwrap_or(CompatibilityStatus::Untested)
    }

    /// Effective model name on the wire, honouring an alias override.
    pub fn wire_model(&self) -> &str {
        self.advanced
            .model_alias
            .as_deref()
            .filter(|alias| !alias.trim().is_empty())
            .unwrap_or(&self.model)
    }

    /// Whether every declared target is independently ready (PRD §18.2).
    ///
    /// `Both` requires **both** tests to pass separately — a single success is
    /// never promoted to a combined verdict.
    pub fn is_ready(&self) -> bool {
        !self.targets.is_empty()
            && self
                .targets
                .iter()
                .all(|agent| self.compatibility_for(*agent).allows_activation())
    }

    /// Stable key describing what a compatibility result was measured against.
    ///
    /// Changing any input invalidates the cache (PRD §45).
    pub fn compatibility_stale_key(&self, agent: AgentId, agent_version: Option<&str>) -> String {
        let secret_marker = self.secret_ref.as_deref().unwrap_or("none");
        format!(
            "v1|{}|{}|{}|{}|{}|{}",
            self.base_url.trim_end_matches('/'),
            self.wire_model(),
            secret_marker,
            agent.as_str(),
            agent_version.unwrap_or("unknown"),
            crate::APP_VERSION
        )
    }

    /// Whether the cached result still describes the current inputs.
    pub fn compatibility_is_stale(&self, agent: AgentId, agent_version: Option<&str>) -> bool {
        match self.compatibility.get(&agent) {
            None => true,
            Some(record) => match &record.stale_key {
                None => true,
                Some(key) => key != &self.compatibility_stale_key(agent, agent_version),
            },
        }
    }

    /// Validate before persisting. The backend is the security boundary, so
    /// every field is checked here rather than in the UI (PRD §36).
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Profile name is required.".into());
        }
        if self.name.chars().count() > 120 {
            return Err("Profile name must be 120 characters or fewer.".into());
        }
        if self.provider_name.trim().is_empty() {
            return Err("Provider name is required.".into());
        }
        if self.model.trim().is_empty() {
            return Err("Model ID is required.".into());
        }
        // Reject control characters in fields that reach config files.
        for (label, value) in [
            ("Profile name", &self.name),
            ("Provider name", &self.provider_name),
            ("Model ID", &self.model),
            ("Base URL", &self.base_url),
        ] {
            if value.chars().any(|c| c.is_control()) {
                return Err(format!("{label} contains characters that are not allowed."));
            }
        }
        if self.targets.is_empty() {
            return Err("Select at least one target agent.".into());
        }
        validate_base_url(&self.base_url)?;
        Ok(())
    }
}

/// Validate and normalise a base URL.
///
/// Returns an actionable message rather than an HTTP status code (PRD §41).
pub fn validate_base_url(input: &str) -> Result<(), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Base URL is required.".into());
    }
    let parsed = url::Url::parse(trimmed)
        .map_err(|_| "Base URL is not a valid URL. Include http:// or https://.".to_string())?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "Base URL must use http:// or https:// (found \"{other}://\")."
            ))
        }
    }
    if parsed.host_str().unwrap_or_default().is_empty() {
        return Err("Base URL must include a host name.".into());
    }
    Ok(())
}

/// Whether a base URL points at a loopback/local address.
///
/// Local endpoints are exempt from the insecure-HTTP warning because the key
/// never leaves the machine.
pub fn is_local_endpoint(input: &str) -> bool {
    let Ok(parsed) = url::Url::parse(input.trim()) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback() || is_private_ip(ip),
        Err(_) => false,
    }
}

fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

/// Normalise a base URL for consistent storage and hashing.
///
/// Trailing slashes are trimmed (never the path itself) so
/// `https://api.x.com/v1/` and `https://api.x.com/v1` compare equal.
pub fn normalize_base_url(input: &str) -> String {
    let trimmed = input.trim();
    let without_trailing = trimmed.trim_end_matches('/');
    if without_trailing.is_empty() {
        trimmed.to_string()
    } else {
        without_trailing.to_string()
    }
}

/// Slugify a profile name into the agent-facing provider id suffix (PRD §16.3).
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_underscore = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "provider".to_string()
    } else {
        trimmed
    }
}

/// Deterministic provider id owned by Eclyzer Bridge for a profile.
///
/// Always namespaced so restore-default can identify managed entries without
/// guessing (PRD §16.3). Reserved built-in IDs are never produced because of
/// the mandatory prefix.
pub fn managed_provider_id(profile: &Profile) -> String {
    format!(
        "{}{}_{}",
        crate::MANAGED_PROVIDER_PREFIX,
        slugify(&profile.provider_name),
        profile.id.chars().take(8).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Profile {
        Profile::new(
            "DeepSeek Flash".into(),
            "DeepSeek".into(),
            "https://api.deepseek.com/v1".into(),
            "deepseek-chat".into(),
            vec![AgentId::Codex],
        )
    }

    #[test]
    fn rejects_empty_name() {
        let mut p = profile();
        p.name = "  ".into();
        assert!(p.validate().is_err());
    }

    #[test]
    fn rejects_missing_model() {
        let mut p = profile();
        p.model = "".into();
        assert!(p.validate().is_err());
    }

    #[test]
    fn rejects_invalid_scheme() {
        let mut p = profile();
        p.base_url = "ftp://example.com".into();
        let err = p.validate().unwrap_err();
        assert!(err.contains("http"), "{err}");
    }

    #[test]
    fn rejects_empty_targets() {
        let mut p = profile();
        p.targets = vec![];
        assert!(p.validate().is_err());
    }

    #[test]
    fn rejects_control_characters_in_config_bound_fields() {
        let mut p = profile();
        p.model = "good\nmodel".into();
        assert!(p.validate().is_err());

        let mut p2 = profile();
        p2.provider_name = "bad\u{0}name".into();
        assert!(p2.validate().is_err());
    }

    #[test]
    fn accepts_pathological_but_legal_windows_input() {
        let mut p = profile();
        p.name = "C:\\Users\\Test User\\My Project\\".into();
        // Control chars only are rejected; spaces/backslashes/Unicode are fine.
        assert!(p.validate().is_ok());
    }

    #[test]
    fn both_requires_both_tests() {
        let mut p = profile();
        p.targets = vec![AgentId::Codex, AgentId::Claude];
        p.compatibility.insert(
            AgentId::Codex,
            CompatibilityRecord {
                status: CompatibilityStatus::Compatible,
                ..Default::default()
            },
        );
        assert!(
            !p.is_ready(),
            "one passing target must not mark Both as ready"
        );

        p.compatibility.insert(
            AgentId::Claude,
            CompatibilityRecord {
                status: CompatibilityStatus::Compatible,
                ..Default::default()
            },
        );
        assert!(p.is_ready());
    }

    #[test]
    fn compatibility_cache_invalidates_on_model_change() {
        let mut p = profile();
        p.compatibility.insert(
            AgentId::Codex,
            CompatibilityRecord {
                status: CompatibilityStatus::Compatible,
                stale_key: Some(p.compatibility_stale_key(AgentId::Codex, Some("1.0.0"))),
                ..Default::default()
            },
        );
        assert!(!p.compatibility_is_stale(AgentId::Codex, Some("1.0.0")));

        p.model = "different-model".into();
        assert!(
            p.compatibility_is_stale(AgentId::Codex, Some("1.0.0")),
            "changing the model must invalidate the cache"
        );
    }

    #[test]
    fn compatibility_cache_invalidates_on_base_url_change() {
        let mut p = profile();
        p.compatibility.insert(
            AgentId::Codex,
            CompatibilityRecord {
                status: CompatibilityStatus::Compatible,
                stale_key: Some(p.compatibility_stale_key(AgentId::Codex, None)),
                ..Default::default()
            },
        );
        p.base_url = "https://other.example.com/v1".into();
        assert!(p.compatibility_is_stale(AgentId::Codex, None));
    }

    #[test]
    fn compatibility_cache_invalidates_on_agent_version_change() {
        let mut p = profile();
        p.compatibility.insert(
            AgentId::Codex,
            CompatibilityRecord {
                status: CompatibilityStatus::Compatible,
                stale_key: Some(p.compatibility_stale_key(AgentId::Codex, Some("1.0.0"))),
                ..Default::default()
            },
        );
        assert!(p.compatibility_is_stale(AgentId::Codex, Some("2.0.0")));
    }

    #[test]
    fn a_freshly_recorded_verdict_is_not_stale() {
        // The list screen drops stale records before showing them, and it
        // compares with `None` because that is what the test command records
        // with. If those two ever disagreed, every newly tested profile would
        // immediately appear untested - a confusing regression rather than a
        // safety one, but one that would be blamed on the provider.
        let mut p = profile();
        let key = p.compatibility_stale_key(AgentId::Codex, None);
        p.compatibility.insert(
            AgentId::Codex,
            CompatibilityRecord {
                status: CompatibilityStatus::Compatible,
                stale_key: Some(key),
                ..Default::default()
            },
        );
        assert!(
            !p.compatibility_is_stale(AgentId::Codex, None),
            "a verdict recorded against the current inputs must survive"
        );
        assert!(p.is_ready(), "and it must still allow activation");
    }

    #[test]
    fn editing_the_model_invalidates_the_cached_verdict() {
        // The concrete case the list screen must not paper over: test against
        // one model, then change it.
        let mut p = profile();
        p.compatibility.insert(
            AgentId::Codex,
            CompatibilityRecord {
                status: CompatibilityStatus::Compatible,
                stale_key: Some(p.compatibility_stale_key(AgentId::Codex, None)),
                ..Default::default()
            },
        );
        assert!(!p.compatibility_is_stale(AgentId::Codex, None));
        p.model = "a-different-model".into();
        assert!(
            p.compatibility_is_stale(AgentId::Codex, None),
            "changing the model must invalidate a passing verdict"
        );
    }

    #[test]
    fn swapping_the_stored_credential_invalidates_the_cached_verdict() {
        let mut p = profile();
        p.secret_ref = Some("ref-one".into());
        p.compatibility.insert(
            AgentId::Codex,
            CompatibilityRecord {
                status: CompatibilityStatus::Compatible,
                stale_key: Some(p.compatibility_stale_key(AgentId::Codex, None)),
                ..Default::default()
            },
        );
        assert!(!p.compatibility_is_stale(AgentId::Codex, None));

        // The user replaces the key: the old verdict was for a different credential.
        p.secret_ref = Some("ref-two".into());
        assert!(
            p.compatibility_is_stale(AgentId::Codex, None),
            "a different key must invalidate a passing verdict"
        );
    }

    #[test]
    fn base_url_normalization() {
        assert_eq!(
            normalize_base_url("https://api.x.com/v1/"),
            "https://api.x.com/v1"
        );
        assert_eq!(normalize_base_url("https://api.x.com"), "https://api.x.com");
    }

    #[test]
    fn detects_local_endpoints() {
        assert!(is_local_endpoint("http://localhost:8080/v1"));
        assert!(is_local_endpoint("http://127.0.0.1:1234/v1"));
        assert!(is_local_endpoint("http://192.168.1.10:8000"));
        assert!(!is_local_endpoint("https://api.deepseek.com"));
    }

    #[test]
    fn slugify_is_filesystem_and_config_safe() {
        assert_eq!(slugify("DeepSeek Flash"), "deepseek_flash");
        assert_eq!(slugify("My  Gateway!!"), "my_gateway");
        assert_eq!(slugify("***"), "provider");
        assert_eq!(slugify("Ünïcødé"), "n_c_d");
    }

    #[test]
    fn managed_provider_id_is_namespaced_and_unique() {
        let a = profile();
        let mut b = profile();
        b.id = uuid::Uuid::new_v4().to_string();
        let id_a = managed_provider_id(&a);
        let id_b = managed_provider_id(&b);
        assert!(id_a.starts_with(crate::MANAGED_PROVIDER_PREFIX));
        assert_ne!(id_a, id_b, "distinct profiles need distinct provider ids");
        // Never collide with Codex reserved built-ins.
        for reserved in ["openai", "ollama", "lmstudio"] {
            assert_ne!(id_a, reserved);
        }
    }

    #[test]
    fn wire_model_honours_alias() {
        let mut p = profile();
        assert_eq!(p.wire_model(), "deepseek-chat");
        p.advanced.model_alias = Some("alias-model".into());
        assert_eq!(p.wire_model(), "alias-model");
        // A blank alias falls back to the real model.
        p.advanced.model_alias = Some("   ".into());
        assert_eq!(p.wire_model(), "deepseek-chat");
    }

    #[test]
    fn agent_id_round_trips() {
        for agent in AgentId::all() {
            assert_eq!(AgentId::parse(agent.as_str()), Some(agent));
        }
    }
}
