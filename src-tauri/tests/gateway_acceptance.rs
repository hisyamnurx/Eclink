//! Verify the Claude compatibility pipeline against a **real** running gateway.
//!
//! This exists because the reported the gateway false negative could not be settled by
//! reading code: the gateway was running, the user had used it successfully, and
//! Bridge disagreed. The only way to know which side was wrong was to send the
//! requests and compare.
//!
//! It is ignored by default because it needs a live gateway and a credential, so it
//! is run deliberately:
//!
//! ```text
//! $env:ECLYZER_GATEWAY_URL  = "http://localhost:41000/v1"
//! $env:ECLYZER_GATEWAY_KEY  = "<gateway api key>"
//! $env:ECLYZER_GATEWAY_MODEL = "combo_a"
//! cargo test --test gateway_acceptance -- --ignored --nocapture
//! ```
//!
//! The key is read from the environment and never printed. What the test prints is
//! the verdict, the step list, and the resolved endpoint - the things needed to
//! compare Bridge's behaviour against a manual `curl`.

use eclink_lib::profiles::{AdvancedOptions, AgentId, AuthMode, CompatibilityStatus, Profile};
use eclink_lib::testing::Secret;
use eclink_lib::testing::{test_profile, TestStepState};

fn env_or_skip(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    }
}

#[tokio::test]
#[ignore = "needs a live gateway; set ECLYZER_GATEWAY_URL and ECLYZER_GATEWAY_KEY"]
async fn a_translating_gateway_is_recognised_as_claude_compatible() {
    let Some(base_url) = env_or_skip("ECLYZER_GATEWAY_URL") else {
        eprintln!("ECLYZER_GATEWAY_URL not set; skipping");
        return;
    };
    let Some(key) = env_or_skip("ECLYZER_GATEWAY_KEY") else {
        eprintln!("ECLYZER_GATEWAY_KEY not set; skipping");
        return;
    };
    let model = env_or_skip("ECLYZER_GATEWAY_MODEL").unwrap_or_else(|| "default".to_string());

    // Print the endpoint that will be requested, so it can be compared against the
    // URL a manual request used. The key is deliberately absent from this output.
    for protocol in [
        eclink_lib::profiles::ApiProtocol::AnthropicMessages,
        eclink_lib::profiles::ApiProtocol::OpenAiResponses,
    ] {
        match eclink_lib::profiles::resolve_endpoint(&base_url, protocol) {
            Ok(url) => println!("resolved {:?} -> {url}", protocol),
            Err(err) => println!("resolved {:?} -> ERROR {err}", protocol),
        }
    }

    let mut profile = Profile::new(
        "gateway".to_string(),
        "gateway".to_string(),
        base_url.clone(),
        model.clone(),
        vec![AgentId::Claude, AgentId::Codex],
    );
    profile.advanced = AdvancedOptions {
        auth_mode: AuthMode::XApiKey,
        ..Default::default()
    };

    for agent in [AgentId::Claude, AgentId::Codex] {
        let outcome = test_profile(&profile, agent, Secret::new(key.clone()))
            .await
            .expect("the pipeline runs");

        println!("\n=== {agent:?} ===");
        println!("status    : {:?}", outcome.status);
        println!("reasonCode: {}", outcome.reason_code);
        println!("reason    : {}", outcome.reason);
        for step in &outcome.steps {
            println!(
                "  {:<16} {:?}{}",
                step.id,
                step.state,
                step.detail
                    .as_deref()
                    .map(|d| format!("  {d}"))
                    .unwrap_or_default()
            );
        }

        if agent == AgentId::Claude {
            // The whole point of this test: a gateway that accepts Anthropic
            // Messages on its client-facing route is Claude-compatible, whatever it
            // does upstream.
            assert_ne!(
                outcome.status,
                CompatibilityStatus::Incompatible,
                "a gateway serving /v1/messages must not be Incompatible for Claude Code. \
                 reason={} detail={:?}",
                outcome.reason,
                outcome
                    .steps
                    .iter()
                    .find(|s| s.id == "protocol")
                    .and_then(|s| s.detail.clone())
            );
            assert_eq!(
                outcome
                    .steps
                    .iter()
                    .find(|s| s.id == "protocol")
                    .map(|s| s.state),
                Some(TestStepState::Passed),
                "the protocol step must pass when the gateway answers /v1/messages"
            );
        }
    }
}
