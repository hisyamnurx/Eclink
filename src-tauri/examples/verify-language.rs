//! Verify the installed build against the real thing, not against a mock.
//!
//! Two defects were reported from manual testing and both were invisible to the
//! unit tests that existed at the time: a false protocol verdict, and a language
//! setting that saved and changed nothing. This example exists so the checks can be
//! re-run against a live environment deliberately:
//!
//! ```text
//! # The language round trip, against the real settings file.
//! cargo run --example verify-language
//!
//! # The gateway verdict, against a running translating gateway.
//! $env:ECLYZER_GATEWAY_KEY = "<key>"
//! cargo run --example verify-gateway -- http://localhost:41000/v1 combo_a
//! ```
//!
//! It lives in `examples/` rather than `src/bin/` because Tauri bundles every binary
//! in the crate's `bin` directory into the installer, and a maintenance tool must
//! not ship to users.

use eclyzer_bridge_lib::adapters::AgentId;
use eclyzer_bridge_lib::commands::SUPPORTED_LANGUAGES;
use eclyzer_bridge_lib::profiles::{AdvancedOptions, AuthMode, Profile};
use eclyzer_bridge_lib::testing::pipeline::test_profile;
use eclyzer_bridge_lib::testing::Secret;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gateway") => gateway(&args[1..]),
        _ => language(),
    }
}

/// The language setting: does it persist through a real settings file?
///
/// Runs against a **scratch directory**, never the user's own. This tool is meant to
/// be run on a working machine and must not be able to disturb the settings someone
/// is actually using. The path exercises the same read, write, and parse code that
/// the application does; only the root differs.
fn language() -> Result<(), Box<dyn std::error::Error>> {
    let scratch = std::env::temp_dir().join("eclyzer-verify-language");
    let _ = std::fs::remove_dir_all(&scratch);
    let paths = eclyzer_bridge_lib::paths::AppPaths::with_root(&scratch);
    paths.ensure_dirs()?;
    println!("scratch root             : {}", paths.root().display());

    let before = eclyzer_bridge_lib::state::load_settings(&paths)?;
    println!("fresh install language   : {}", before.language);
    println!("supported languages      : {SUPPORTED_LANGUAGES:?}");
    assert_eq!(
        before.language, "en",
        "a fresh install must start in English"
    );

    // The same change the Settings screen makes, through the same layer.
    for target in SUPPORTED_LANGUAGES {
        let mut settings = eclyzer_bridge_lib::state::load_settings(&paths)?;
        settings.language = target.to_string();
        eclyzer_bridge_lib::state::save_settings(&paths, &settings)?;

        // Read it back from disk, which is what a restart does.
        let reloaded = eclyzer_bridge_lib::state::load_settings(&paths)?;
        let ok = reloaded.language == target;
        println!(
            "  set {target:<3} -> read back {:<3} {}",
            reloaded.language,
            if ok { "OK" } else { "MISMATCH" }
        );
        assert!(ok, "the language did not survive a save and reload");
    }

    // An older file with no language key must load as English rather than fail.
    let raw = std::fs::read_to_string(paths.root().join("app.json"))?;
    let without: String = {
        let mut value: serde_json::Value = serde_json::from_str(&raw)?;
        value.as_object_mut().unwrap().remove("language");
        serde_json::to_string_pretty(&value)?
    };
    std::fs::write(paths.root().join("app.json"), without)?;
    let migrated = eclyzer_bridge_lib::state::load_settings(&paths)?;
    println!(
        "  file with no language key -> {} (expected en)",
        migrated.language
    );
    assert_eq!(migrated.language, "en");

    // Clean up after ourselves.
    let _ = std::fs::remove_dir_all(&scratch);
    println!("scratch removed");
    Ok(())
}

/// The gateway verdict, against a real endpoint.
fn gateway(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let base_url = args.first().cloned().unwrap_or_else(|| {
        std::env::var("ECLYZER_GATEWAY_URL").unwrap_or_else(|_| "http://localhost:41000/v1".into())
    });
    let model = args.get(1).cloned().unwrap_or_else(|| {
        std::env::var("ECLYZER_GATEWAY_MODEL").unwrap_or_else(|_| "default".into())
    });
    let key = std::env::var("ECLYZER_GATEWAY_KEY")
        .map_err(|_| "set ECLYZER_GATEWAY_KEY to the gateway's API key")?;

    println!("gateway : {base_url}");
    println!("model   : {model}");
    // The key is never printed, only its presence.
    println!("key     : <{} chars>", key.len());

    for protocol in [
        eclyzer_bridge_lib::profiles::ApiProtocol::AnthropicMessages,
        eclyzer_bridge_lib::profiles::ApiProtocol::OpenAiResponses,
    ] {
        println!(
            "resolved {:?} -> {}",
            protocol,
            eclyzer_bridge_lib::profiles::resolve_endpoint(&base_url, protocol)?
        );
    }

    let runtime = tokio::runtime::Runtime::new()?;
    let mut profile = Profile::new(
        "gateway".into(),
        "gateway".into(),
        base_url.clone(),
        model.clone(),
        vec![AgentId::Claude, AgentId::Codex],
    );
    profile.advanced = AdvancedOptions {
        auth_mode: AuthMode::XApiKey,
        ..Default::default()
    };

    for agent in [AgentId::Claude, AgentId::Codex] {
        let outcome = runtime.block_on(test_profile(&profile, agent, Secret::new(key.clone())))?;
        println!(
            "\n{agent:?}: {:?} ({})",
            outcome.status, outcome.reason_code
        );
        println!("  {}", outcome.reason);
        for step in &outcome.steps {
            println!(
                "    {:<16} {:?} {}",
                step.id,
                step.state,
                step.detail.as_deref().unwrap_or("")
            );
        }
    }

    // The Playground, through the same endpoint and credential. This is the check
    // that matters most: a provider the compatibility test accepts must also work in
    // the Playground, or a user is told something works and then cannot use it. The
    // two used to hold separate parsers and disagreed about a streamed reply.
    for protocol in [
        eclyzer_bridge_lib::profiles::ApiProtocol::AnthropicMessages,
        eclyzer_bridge_lib::profiles::ApiProtocol::OpenAiResponses,
    ] {
        let target = eclyzer_bridge_lib::testing::probes::ProbeTarget::new(
            &base_url,
            &model,
            Secret::new(key.clone()),
            AuthMode::XApiKey,
            Some(30_000),
        )?;
        match runtime.block_on(eclyzer_bridge_lib::testing::playground::send(
            &target, "halo", protocol,
        )) {
            Ok(result) => println!(
                "\nPlayground {protocol:?}: OK  framing={} streaming={} latency={}ms tokens={:?}/{:?}\n  reply: {}",
                result.framing,
                result.streaming,
                result.latency_ms,
                result.prompt_tokens,
                result.completion_tokens,
                result.text.replace('\n', " ")
            ),
            Err(err) => println!(
                "\nPlayground {protocol:?}: FAILED  code={:?} message={} details={:?}",
                err.code, err.message, err.details
            ),
        }
    }

    Ok(())
}
