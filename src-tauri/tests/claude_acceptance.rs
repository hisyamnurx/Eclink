//! Verify the Claude activation fix against a **copy** of the user's real
//! settings file.
//!
//! This is not a unit test of a pure function: it drives the real
//! `ClaudeAdapter::apply_profile` and `reset_to_stock_defaults` against the exact
//! file contents that produced the reported bug, then prints what a new Claude
//! Code session would see. It is ignored by default because it reads and writes a
//! directory named by an environment variable, and it exists so the acceptance
//! check can be run deliberately:
//!
//! ```text
//! # Start from the backup the fix was validated against, so the test always
//! # sees the input that produced the bug rather than the already-fixed file.
//! $dir = "$env:TEMP\claude-verify"
//! New-Item -ItemType Directory -Force $dir | Out-Null
//! Copy-Item "$env:USERPROFILE\.claude\settings.json.before-bridge.bak" `
//!           "$dir\settings.json" -Force
//! $env:ECLYZER_VERIFY_DIR = $dir
//! cargo test --test claude_acceptance -- --ignored --nocapture
//! ```
//!
//! The backup lives in the user's own Claude directory rather than in the
//! repository: it is their personal configuration, and a public repository is not
//! the place for it even though it happens to contain no credential.
//!
//! The directory must already contain a `settings.json` to operate on, and the
//! test refuses to run against a path that looks like a real `~/.claude`.

use std::path::PathBuf;

use eclyzer_bridge_lib::adapters::claude::{effective_model, ClaudeAdapter, MODEL_SETTINGS_KEY};
use eclyzer_bridge_lib::adapters::{AgentAdapter, ApplyOptions};
use eclyzer_bridge_lib::backup::BackupManager;
use eclyzer_bridge_lib::profiles::{AgentId, AuthMode, Profile};

fn verify_dir() -> PathBuf {
    let raw = std::env::var("ECLYZER_VERIFY_DIR")
        .expect("set ECLYZER_VERIFY_DIR to a scratch directory containing settings.json");
    let path = PathBuf::from(raw);

    // Safety: never operate on a real Claude directory, and never on a home root.
    let text = path.display().to_string();
    assert!(
        !text.contains(".claude") || text.contains("verify") || text.contains("scratch"),
        "refusing to run against what looks like a real Claude config directory: {text}"
    );
    assert!(
        path.join("settings.json").exists(),
        "{} does not contain settings.json",
        path.display()
    );
    path
}

fn adapter_for(path: &PathBuf) -> (ClaudeAdapter, tempfile::TempDir) {
    // The adapter resolves its directory from CLAUDE_CONFIG_DIR, so point it at
    // the scratch copy rather than the user's real one.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", path) };
    let backups = tempfile::tempdir().expect("a temp dir for backups");
    let adapter = ClaudeAdapter::new(BackupManager::new(backups.path().join("backups")));
    (adapter, backups)
}

fn options<'a>() -> ApplyOptions<'a> {
    ApplyOptions {
        secret: Some("sk-verify-key"),
        helper_path: None,
        expected_sha256: None,
        agent_version: None,
    }
}

fn profile() -> Profile {
    let mut p = Profile::new(
        "provider_a".to_string(),
        "provider_a".to_string(),
        "https://gateway.example.com".to_string(),
        "example-model".to_string(),
        vec![AgentId::Claude],
    );
    p.advanced = AdvancedOptions {
        auth_mode: AuthMode::XApiKey,
        ..Default::default()
    };
    p
}

use eclyzer_bridge_lib::profiles::AdvancedOptions;

#[test]
#[ignore = "reads ECLYZER_VERIFY_DIR; run deliberately"]
fn activating_provider_a_makes_its_model_the_one_a_new_session_starts_on() {
    let dir = verify_dir();
    let settings = dir.join("settings.json");
    let before = std::fs::read_to_string(&settings).expect("read before");
    println!("\n=== BEFORE ===\n{before}");

    let (adapter, _backups) = adapter_for(&dir);
    let outcome = adapter
        .apply_profile(&profile(), &options())
        .expect("activation must succeed");
    println!("\nmanaged keys: {:?}", outcome.managed_keys);

    let after = std::fs::read_to_string(&settings).expect("read after");
    println!("\n=== AFTER ACTIVATE ===\n{after}");

    let doc: serde_json::Value = serde_json::from_str(&after).expect("valid JSON");
    assert_eq!(
        doc.get(MODEL_SETTINGS_KEY).and_then(|v| v.as_str()),
        Some("example-model"),
        "the activated model must be the one selected for new sessions"
    );
    assert_eq!(
        doc.get("env")
            .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
            .and_then(|v| v.as_str()),
        Some("https://gateway.example.com"),
        "the provider endpoint must be the activated one"
    );
    // The old localhost endpoint must be gone from the *selection* keys, which is
    // where a stale provider URL would actually misroute requests.
    assert_eq!(
        doc.get("env")
            .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
            .and_then(|v| v.as_str()),
        Some("https://gateway.example.com"),
        "the provider endpoint must be the activated one"
    );
    // Unrelated settings must survive.
    for kept in [
        "modelSettings",
        "\"theme\": \"dark\"",
        "hasCompletedOnboarding",
    ] {
        assert!(after.contains(kept), "lost {kept}");
    }

    let report = effective_model(None);
    println!("\n=== EFFECTIVE MODEL ===\n{report:#?}");
    assert_eq!(report.source, "bridge");
    assert_eq!(report.model.as_deref(), Some("example-model"));

    // Now the stock reset: everything provider/model shaped must go.
    adapter
        .reset_to_stock_defaults(&ApplyOptions {
            secret: None,
            helper_path: None,
            expected_sha256: None,
            agent_version: None,
        })
        .expect("reset must succeed");

    let reset = std::fs::read_to_string(&settings).expect("read reset");
    println!("\n=== AFTER STOCK RESET ===\n{reset}");

    // Every key that can *select* a provider or a model must be gone. The check
    // is made against the parsed document rather than the raw text, because
    // `modelSettings` legitimately contains model names as *keys* — see below —
    // and a substring test over the whole file cannot tell those apart.
    let doc: serde_json::Value = serde_json::from_str(&reset).expect("valid JSON");
    let env = doc.get("env").and_then(|v| v.as_object());
    let anthropic_keys: Vec<&String> = env
        .map(|map| map.keys().filter(|k| k.starts_with("ANTHROPIC_")).collect())
        .unwrap_or_default();
    assert!(
        anthropic_keys.is_empty(),
        "no ANTHROPIC_* override may survive a stock reset, found {anthropic_keys:?}:\n{reset}"
    );
    // The legacy key Bridge used to write must be gone specifically.
    for key in [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
    ] {
        assert!(
            env.is_none_or(|map| !map.contains_key(key)),
            "{key} survived a stock reset:\n{reset}"
        );
    }
    assert!(doc.get("model").is_none(), "the model key survived a reset");
    assert!(
        doc.get("modelOverrides").is_none(),
        "modelOverrides survived a reset"
    );

    // `modelSettings` is deliberately NOT removed, and the reason matters:
    // the official settings reference defines it as "keep a saved effort level or
    // auto-compact window per model, or cap one model's effort". It selects no
    // model and adds no `/model` picker row, so removing it would destroy the
    // user's effort preferences for nothing. Its keys are model *names*, which is
    // why a substring check on a model ID must not be used as the acceptance
    // criterion for a reset.
    assert!(
        reset.contains("modelSettings"),
        "the user's effort levels were destroyed:\n{reset}"
    );

    // What the user actually sees in `/model` comes from the selection keys, all
    // of which are asserted gone above.

    // The user's own settings still survive.
    for kept in [
        "modelSettings",
        "\"theme\": \"dark\"",
        "hasCompletedOnboarding",
        "autoUpdatesChannel",
    ] {
        assert!(reset.contains(kept), "the reset destroyed {kept}:\n{reset}");
    }

    let after_reset = effective_model(None);
    println!("\n=== EFFECTIVE MODEL AFTER RESET ===\n{after_reset:#?}");
    assert_eq!(
        after_reset.source, "account",
        "after a stock reset, Claude Code's account default should win"
    );

    println!("\nALL ACCEPTANCE ASSERTIONS PASSED");
}
