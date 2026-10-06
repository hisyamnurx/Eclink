//! The ordered test pipeline and its verdict (PRD §20, §21).
//!
//! The pipeline stops as soon as a step makes the remaining ones meaningless:
//! there is no point testing streaming on an endpoint that rejected the key. Each
//! step is reported individually so the UI can show exactly where things broke.

use serde::{Deserialize, Serialize};

use crate::adapters::AgentId;
use crate::error::{BridgeError, BridgeResult};
use crate::profiles::{CompatibilityRecord, CompatibilityStatus, Profile};
use crate::testing::probes::{
    self, ProbeOutcome, ProbeResult, ProbeTarget, DEFAULT_TIMEOUT, STREAM_TIMEOUT,
};
use crate::testing::secret::Secret;

/// State of one pipeline step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestStepState {
    /// Ran and behaved as required.
    Passed,
    /// Ran and revealed something the user should know but which is not fatal.
    Warning,
    /// Ran and failed.
    Failed,
    /// Not run because an earlier step made it meaningless.
    Skipped,
}

/// One step's public result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestStep {
    /// Stable identifier, e.g. `reachability`.
    pub id: String,
    /// Human label, e.g. "Reachability".
    pub label: String,
    pub state: TestStepState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

impl TestStep {
    fn new(id: &str, label: &str, state: TestStepState, result: &ProbeResult) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            state,
            detail: result.detail.clone(),
            latency_ms: Some(result.latency_ms),
        }
    }

    fn skipped(id: &str, label: &str, because: &str) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            state: TestStepState::Skipped,
            detail: Some(format!("Not run: {because}")),
            latency_ms: None,
        }
    }
}

/// The full result of testing one profile against one agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestOutcome {
    pub profile_id: String,
    pub agent: AgentId,
    pub status: CompatibilityStatus,
    /// Stable machine reason, e.g. `responses_unsupported`.
    pub reason_code: String,
    /// Human explanation, already redacted.
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    pub steps: Vec<TestStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    pub streaming: bool,
    pub tool_calling: bool,
}

impl TestOutcome {
    /// Convert to the persisted compatibility record (PRD §21.1).
    pub fn to_record(&self, stale_key: &str) -> CompatibilityRecord {
        CompatibilityRecord {
            status: self.status,
            tested_at: Some(chrono::Local::now().to_rfc3339()),
            latency_ms: self.latency_ms,
            streaming: self.streaming,
            tool_calling: self.tool_calling,
            reason_code: Some(self.reason_code.clone()),
            reason: Some(self.reason.clone()),
            warning: self.warning.clone(),
            stale_key: Some(stale_key.to_string()),
        }
    }
}

/// Resolve the credential for `profile` from the secret store.
///
/// Returns an empty [`Secret`] when the profile has no stored key, which the
/// pipeline reports as a validation failure rather than a network problem.
pub fn resolve_secret(
    store: &crate::secrets::SecretStore,
    profile: &Profile,
) -> BridgeResult<Secret> {
    let Some(reference) = profile.secret_ref.as_deref() else {
        return Ok(Secret::default());
    };
    match store.get(reference)? {
        Some(value) => Ok(Secret::new(value)),
        None => Err(
            BridgeError::secret("The API key for this profile is no longer available.")
                .with_hint("Open the profile and enter its API key again."),
        ),
    }
}

/// Build a probe target from a profile, applying its advanced options.
///
/// Public so the Playground sends requests with identical validation, header
/// handling, and insecure-endpoint rules as Test Connection.
pub fn build_target(profile: &Profile, secret: Secret) -> BridgeResult<ProbeTarget> {
    let mut target = ProbeTarget::new(
        &profile.base_url,
        profile.wire_model(),
        secret,
        profile.advanced.auth_mode,
        profile.advanced.timeout_ms,
    )?;

    // Insecure HTTP is only allowed after explicit confirmation (PRD §50.1).
    if !crate::profiles::is_local_endpoint(&target.base_url)
        && target.base_url.starts_with("http://")
        && !profile.advanced.allow_insecure_endpoint
    {
        return Err(BridgeError::validation(
            "This endpoint uses plain HTTP, so your API key would be sent without encryption.",
        )
        .with_hint(
            "Use an https:// endpoint, or enable the insecure-endpoint option for this profile.",
        )
        .with_details("insecure_endpoint"));
    }

    // Reserved headers are dropped by `profiles::user_headers`, which the Codex
    // adapter also uses. Sharing one function matters: a user-supplied header
    // must never override the credential, and a filter that exists in only one
    // caller is a filter the other caller forgets.
    target.extra_headers = crate::profiles::user_headers(&profile.advanced);
    target.extra_query = profile
        .advanced
        .query_params
        .iter()
        .filter(|pair| !pair.name.trim().is_empty())
        .map(|pair| (pair.name.trim().to_string(), pair.value.clone()))
        .collect();

    Ok(target)
}

/// Static checks that must pass before any network traffic is sent.
///
/// Order matters for the quality of the message: the most specific problem is
/// reported first, so a user with a malformed URL is told about the URL rather
/// than about a generic profile problem.
fn validate_inputs(
    profile: &Profile,
    agent: AgentId,
    secret: &Secret,
) -> Result<(), (String, String)> {
    // 1. The URL, because it is the most common and most concrete mistake.
    if let Err(message) = crate::profiles::validate_base_url(&profile.base_url) {
        return Err(("invalid_base_url".to_string(), message));
    }
    // 2. The credential.
    if secret.is_empty() {
        return Err((
            "missing_secret".to_string(),
            "No API key is stored for this profile.".to_string(),
        ));
    }
    // 3. Whether this profile is meant for this agent at all.
    if !profile.targets_agent(agent) {
        return Err((
            "agent_not_targeted".to_string(),
            format!(
                "This profile is not configured for {}. Add it as a target before testing.",
                agent.display_name()
            ),
        ));
    }
    // 4. Everything else about the profile.
    if let Err(message) = profile.validate() {
        return Err(("invalid_profile".to_string(), message));
    }
    Ok(())
}

/// Run the full pipeline for one (profile, agent) pair.
///
/// This performs real network requests and may incur a small cost at the
/// provider. The caller is responsible for showing the cost notice first.
pub async fn test_profile(
    profile: &Profile,
    agent: AgentId,
    secret: Secret,
) -> BridgeResult<TestOutcome> {
    let mut steps: Vec<TestStep> = Vec::new();

    // ---- 1. Input validation -------------------------------------------
    if let Err((code, message)) = validate_inputs(profile, agent, &secret) {
        steps.push(TestStep {
            id: "validation".to_string(),
            label: "Input Validation".to_string(),
            state: TestStepState::Failed,
            detail: Some(message.clone()),
            latency_ms: None,
        });
        return Ok(TestOutcome {
            profile_id: profile.id.clone(),
            agent,
            status: CompatibilityStatus::Incompatible,
            reason_code: code,
            reason: message,
            warning: None,
            steps,
            latency_ms: None,
            streaming: false,
            tool_calling: false,
        });
    }
    steps.push(TestStep {
        id: "validation".to_string(),
        label: "Input Validation".to_string(),
        state: TestStepState::Passed,
        detail: None,
        latency_ms: None,
    });

    let mut target = build_target(profile, secret)?;
    let codex = agent == AgentId::Codex;

    // ---- 2. Reachability ------------------------------------------------
    let reach = probes::probe_reachability(&target).await;
    let reach_ok = reach.outcome == ProbeOutcome::Supported;
    steps.push(TestStep::new(
        "reachability",
        "Reachability",
        if reach_ok {
            TestStepState::Passed
        } else {
            TestStepState::Failed
        },
        &reach,
    ));
    if !reach_ok {
        return Ok(finish(
            profile,
            agent,
            steps,
            reach.outcome,
            None,
            false,
            false,
            None,
        ));
    }

    // ---- 3. Authentication ---------------------------------------------
    let auth = probes::probe_authentication(&target).await;
    let auth_ok = auth.outcome == ProbeOutcome::Supported;
    steps.push(TestStep::new(
        "authentication",
        "Authentication",
        if auth_ok {
            TestStepState::Passed
        } else {
            TestStepState::Failed
        },
        &auth,
    ));
    if !auth_ok {
        return Ok(finish(
            profile,
            agent,
            steps,
            auth.outcome,
            None,
            false,
            false,
            None,
        ));
    }

    // ---- 4. Model availability -----------------------------------------
    let model = probes::probe_model(&target).await;
    let model_state = match model.outcome {
        ProbeOutcome::Supported => TestStepState::Passed,
        ProbeOutcome::UnknownModel => TestStepState::Failed,
        // A rate limit here is not a configuration problem; carry on.
        ProbeOutcome::RateLimited => TestStepState::Warning,
        _ => TestStepState::Warning,
    };
    steps.push(TestStep::new(
        "model",
        "Model Availability",
        model_state,
        &model,
    ));
    if model.outcome == ProbeOutcome::UnknownModel {
        // The remaining steps would all fail for the same reason, so say so
        // explicitly rather than reporting three more confusing failures.
        steps.push(TestStep::skipped(
            "protocol",
            "Agent Protocol",
            "the endpoint does not recognize this model",
        ));
        steps.push(TestStep::skipped(
            "streaming",
            "Streaming",
            "the endpoint does not recognize this model",
        ));
        steps.push(TestStep::skipped(
            "tools",
            "Tool Calling",
            "the endpoint does not recognize this model",
        ));
        return Ok(finish(
            profile,
            agent,
            steps,
            ProbeOutcome::UnknownModel,
            None,
            false,
            false,
            None,
        ));
    }

    // ---- 5. Agent protocol ---------------------------------------------
    let protocol = if codex {
        probes::probe_codex_protocol(&target).await
    } else {
        probes::probe_claude_protocol(&target).await
    };
    let protocol_ok = protocol.outcome == ProbeOutcome::Supported;
    steps.push(TestStep::new(
        "protocol",
        "Agent Protocol",
        if protocol_ok {
            TestStepState::Passed
        } else {
            TestStepState::Failed
        },
        &protocol,
    ));
    if !protocol_ok {
        return Ok(finish(
            profile,
            agent,
            steps,
            protocol.outcome,
            None,
            false,
            false,
            Some(&protocol),
        ));
    }

    // From here on the endpoint works; the remaining steps only refine the
    // verdict, so they use the longer streaming timeout.
    target.timeout = STREAM_TIMEOUT;

    // ---- 6. Streaming ---------------------------------------------------
    let streaming = probes::probe_streaming(&target, codex).await;
    let stream_ok = streaming.outcome == ProbeOutcome::Supported && streaming.streamed;
    // Streaming is an optional capability: the required steps already passed, so
    // a failure here lowers the verdict to partially compatible rather than
    // failing the whole test.
    steps.push(TestStep::new(
        "streaming",
        "Streaming",
        if stream_ok {
            TestStepState::Passed
        } else {
            TestStepState::Warning
        },
        &streaming,
    ));

    // ---- 7. Tool calling -----------------------------------------------
    let tools = probes::probe_tool_calling(&target, codex).await;
    let tools_ok = tools.outcome == ProbeOutcome::Supported && tools.tool_called;
    let tools_state = if tools_ok {
        TestStepState::Passed
    } else if tools.outcome == ProbeOutcome::Supported {
        // The model answered but chose not to call the tool. Worth noting,
        // not worth failing the whole profile over.
        TestStepState::Warning
    } else {
        TestStepState::Warning
    };
    steps.push(TestStep::new("tools", "Tool Calling", tools_state, &tools));

    let latency = Some(protocol.latency_ms);

    // ---- 8. Verdict ------------------------------------------------------
    let (status, reason_code, reason, warning) = verdict(stream_ok, tools_ok, tools.outcome);

    Ok(TestOutcome {
        profile_id: profile.id.clone(),
        agent,
        status,
        reason_code,
        reason,
        warning,
        steps,
        latency_ms: latency,
        streaming: stream_ok,
        tool_calling: tools_ok,
    })
}

/// Combine the optional-step results into a verdict (PRD §21.1).
///
/// A profile is never promoted to `Compatible` on the strength of the required
/// steps alone when the optional ones failed outright.
fn verdict(
    stream_ok: bool,
    tools_ok: bool,
    tools_outcome: ProbeOutcome,
) -> (CompatibilityStatus, String, String, Option<String>) {
    match (stream_ok, tools_ok) {
        (true, true) => (
            CompatibilityStatus::Compatible,
            "compatible".to_string(),
            "This endpoint supports streaming and tool calling for this agent.".to_string(),
            None,
        ),
        (true, false) => {
            let warning = if tools_outcome == ProbeOutcome::Supported {
                "Streaming works. Tool calling could not be confirmed because the model answered without calling the tool."
                    .to_string()
            } else {
                format!(
                    "Streaming works, but tool calling failed: {}",
                    tools_outcome.message()
                )
            };
            (
                CompatibilityStatus::PartiallyCompatible,
                "tools_unsupported".to_string(),
                "This endpoint supports streaming but tool calling could not be confirmed."
                    .to_string(),
                Some(warning),
            )
        }
        (false, true) => (
            CompatibilityStatus::PartiallyCompatible,
            "streaming_unsupported".to_string(),
            "This endpoint supports tool calling but did not stream responses in a usable form."
                .to_string(),
            Some(
                "The agent will still work, but responses will arrive all at once instead of streaming."
                    .to_string(),
            ),
        ),
        (false, false) => (
            CompatibilityStatus::PartiallyCompatible,
            "streaming_and_tools_unsupported".to_string(),
            "This endpoint answered correctly but supports neither streaming nor tool calling."
                .to_string(),
            Some(
                "Basic conversations will work. Streaming and tool use will not."
                    .to_string(),
            ),
        ),
    }
}

/// Assemble an outcome for a pipeline that stopped early.
#[allow(clippy::too_many_arguments)]
fn finish(
    profile: &Profile,
    agent: AgentId,
    steps: Vec<TestStep>,
    outcome: ProbeOutcome,
    _latency: Option<u64>,
    streaming: bool,
    tool_calling: bool,
    protocol: Option<&ProbeResult>,
) -> TestOutcome {
    let (status, reason_code, reason) = match outcome {
        ProbeOutcome::Unauthorized => (
            CompatibilityStatus::Incompatible,
            "unauthorized",
            "The endpoint rejected the API key. Check that the key is correct and active.",
        ),
        ProbeOutcome::UnknownModel => (
            CompatibilityStatus::Incompatible,
            "unknown_model",
            "The endpoint does not recognize this model. Check the model ID exactly as the provider spells it.",
        ),
        ProbeOutcome::Unsupported => {
            if protocol.is_some() {
                (
                    CompatibilityStatus::Incompatible,
                    "protocol_unsupported",
                    "The endpoint does not implement the API format this agent requires.",
                )
            } else {
                (
                    CompatibilityStatus::Incompatible,
                    "endpoint_unsupported",
                    "The endpoint did not accept the request format.",
                )
            }
        }
        ProbeOutcome::Unreachable => (
            CompatibilityStatus::TestFailed,
            "unreachable",
            "The endpoint could not be reached. Check the base URL and your network connection.",
        ),
        ProbeOutcome::TimedOut => (
            CompatibilityStatus::TestFailed,
            "timed_out",
            "The endpoint did not respond in time. It may be slow or blocked by a firewall.",
        ),
        ProbeOutcome::RateLimited => (
            CompatibilityStatus::Unknown,
            "rate_limited",
            "The endpoint is rate limiting requests, so compatibility could not be determined. Try again shortly.",
        ),
        ProbeOutcome::ServerError => (
            CompatibilityStatus::TestFailed,
            "server_error",
            "The endpoint returned a server error. This is usually temporary.",
        ),
        ProbeOutcome::Supported => (
            CompatibilityStatus::Compatible,
            "compatible",
            "The endpoint answered correctly.",
        ),
    };

    TestOutcome {
        profile_id: profile.id.clone(),
        agent,
        status,
        reason_code: reason_code.to_string(),
        reason: reason.to_string(),
        warning: None,
        steps,
        latency_ms: None,
        streaming,
        tool_calling,
    }
}

/// The cost notice shown before the first test run (PRD §20.9).
pub const COST_NOTICE: &str = "Testing sends a small number of real requests to your provider. \
These requests may count toward your usage and cost a small amount.";

/// The concurrency guard: only one test may run per profile at a time.
pub fn lock_name(profile_id: &str) -> String {
    format!("test-{profile_id}.lock")
}

/// Default timeout used when a profile does not override one.
pub fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT.as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::AdvancedOptions;

    fn profile() -> Profile {
        let mut p = Profile::new(
            "DeepSeek".to_string(),
            "DeepSeek".to_string(),
            "https://api.deepseek.com".to_string(),
            "deepseek-chat".to_string(),
            vec![AgentId::Codex, AgentId::Claude],
        );
        p.secret_ref = Some(p.id.clone());
        p
    }

    #[test]
    fn missing_secret_is_caught_before_any_traffic() {
        let p = profile();
        let err = validate_inputs(&p, AgentId::Codex, &Secret::default()).unwrap_err();
        assert_eq!(err.0, "missing_secret");
    }

    #[test]
    fn an_untargeted_agent_is_rejected() {
        let mut p = profile();
        p.targets = vec![AgentId::Codex];
        let err = validate_inputs(&p, AgentId::Claude, &Secret::new("k")).unwrap_err();
        assert_eq!(err.0, "agent_not_targeted");
        assert!(err.1.contains("Claude Code"), "{}", err.1);
    }

    #[test]
    fn an_invalid_url_is_rejected_with_a_code() {
        let mut p = profile();
        p.base_url = "ftp://example.com".to_string();
        let err = validate_inputs(&p, AgentId::Codex, &Secret::new("k")).unwrap_err();
        assert_eq!(err.0, "invalid_base_url");
    }

    #[test]
    fn insecure_remote_http_requires_explicit_confirmation() {
        let mut p = profile();
        p.base_url = "http://api.example.com".to_string();
        let err = build_target(&p, Secret::new("k")).unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Validation);
        assert!(
            err.message.contains("without encryption"),
            "{}",
            err.message
        );

        // With confirmation it proceeds.
        p.advanced = AdvancedOptions {
            allow_insecure_endpoint: true,
            ..Default::default()
        };
        assert!(build_target(&p, Secret::new("k")).is_ok());
    }

    #[test]
    fn loopback_http_is_allowed_without_a_prompt() {
        // The key never leaves the machine, so there is nothing to warn about.
        let mut p = profile();
        p.base_url = "http://127.0.0.1:8080".to_string();
        assert!(build_target(&p, Secret::new("k")).is_ok());
    }

    #[test]
    fn a_user_header_cannot_override_the_credential() {
        let mut p = profile();
        p.advanced = AdvancedOptions {
            headers: vec![
                crate::profiles::HeaderPair {
                    name: "Authorization".to_string(),
                    value: "Bearer attacker".to_string(),
                },
                crate::profiles::HeaderPair {
                    name: "x-api-key".to_string(),
                    value: "attacker".to_string(),
                },
                crate::profiles::HeaderPair {
                    name: "X-Tenant".to_string(),
                    value: "acme".to_string(),
                },
            ],
            ..Default::default()
        };
        let target = build_target(&p, Secret::new("k")).unwrap();
        assert_eq!(target.extra_headers.len(), 1);
        assert_eq!(target.extra_headers[0].0, "X-Tenant");
    }

    #[test]
    fn verdict_is_compatible_only_when_both_optional_steps_pass() {
        let (status, _, _, warning) = verdict(true, true, ProbeOutcome::Supported);
        assert_eq!(status, CompatibilityStatus::Compatible);
        assert!(warning.is_none());
    }

    #[test]
    fn streaming_without_tools_is_partially_compatible_with_a_warning() {
        let (status, code, _, warning) = verdict(true, false, ProbeOutcome::Supported);
        assert_eq!(status, CompatibilityStatus::PartiallyCompatible);
        assert_eq!(code, "tools_unsupported");
        assert!(warning.is_some());
    }

    #[test]
    fn tools_without_streaming_is_partially_compatible() {
        let (status, code, _, warning) = verdict(false, true, ProbeOutcome::Supported);
        assert_eq!(status, CompatibilityStatus::PartiallyCompatible);
        assert_eq!(code, "streaming_unsupported");
        assert!(warning.is_some());
    }

    #[test]
    fn neither_optional_step_is_still_usable() {
        let (status, code, _, warning) = verdict(false, false, ProbeOutcome::Unsupported);
        assert_eq!(status, CompatibilityStatus::PartiallyCompatible);
        assert_eq!(code, "streaming_and_tools_unsupported");
        assert!(warning.is_some());
    }

    #[test]
    fn early_exit_reasons_map_to_actionable_copy() {
        let p = profile();
        let outcome = finish(
            &p,
            AgentId::Codex,
            Vec::new(),
            ProbeOutcome::Unauthorized,
            None,
            false,
            false,
            None,
        );
        assert_eq!(outcome.status, CompatibilityStatus::Incompatible);
        assert_eq!(outcome.reason_code, "unauthorized");
        assert!(outcome.reason.contains("API key"), "{}", outcome.reason);
    }

    #[test]
    fn a_protocol_mismatch_names_the_protocol() {
        let p = profile();
        let protocol = ProbeResult {
            outcome: ProbeOutcome::Unsupported,
            latency_ms: 10,
            detail: None,
            streamed: false,
            tool_called: false,
        };
        let outcome = finish(
            &p,
            AgentId::Codex,
            Vec::new(),
            ProbeOutcome::Unsupported,
            None,
            false,
            false,
            Some(&protocol),
        );
        assert_eq!(outcome.reason_code, "protocol_unsupported");
    }

    #[test]
    fn a_transport_failure_is_test_failed_not_incompatible() {
        // A network hiccup must never be recorded as "this provider doesn't work".
        for probe_outcome in [ProbeOutcome::Unreachable, ProbeOutcome::TimedOut] {
            let outcome = finish(
                &profile(),
                AgentId::Codex,
                Vec::new(),
                probe_outcome,
                None,
                false,
                false,
                None,
            );
            assert_eq!(
                outcome.status,
                CompatibilityStatus::TestFailed,
                "{probe_outcome:?} must not be a compatibility verdict"
            );
        }
    }

    #[test]
    fn a_rate_limit_is_unknown_rather_than_incompatible() {
        let outcome = finish(
            &profile(),
            AgentId::Codex,
            Vec::new(),
            ProbeOutcome::RateLimited,
            None,
            false,
            false,
            None,
        );
        assert_eq!(outcome.status, CompatibilityStatus::Unknown);
    }

    #[test]
    fn outcome_converts_to_a_persisted_record() {
        let outcome = finish(
            &profile(),
            AgentId::Codex,
            Vec::new(),
            ProbeOutcome::Supported,
            None,
            true,
            true,
            None,
        );
        let record = outcome.to_record("stale-key");
        assert_eq!(record.status, CompatibilityStatus::Compatible);
        assert_eq!(record.stale_key.as_deref(), Some("stale-key"));
        assert!(record.tested_at.is_some());
        assert!(record.streaming);
    }

    #[test]
    fn skipped_steps_say_why() {
        let step = TestStep::skipped("streaming", "Streaming", "authentication failed");
        assert_eq!(step.state, TestStepState::Skipped);
        assert!(step.detail.unwrap().contains("authentication failed"));
    }

    #[test]
    fn cost_notice_mentions_the_cost() {
        assert!(COST_NOTICE.contains("cost"), "{COST_NOTICE}");
        assert!(COST_NOTICE.contains("real requests"), "{COST_NOTICE}");
    }

    #[test]
    fn lock_names_are_per_profile() {
        assert_eq!(lock_name("abc"), "test-abc.lock");
        assert_ne!(lock_name("abc"), lock_name("def"));
    }
}
