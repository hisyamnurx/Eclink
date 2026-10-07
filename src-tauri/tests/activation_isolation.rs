//! Activating one agent must not touch the other.
//!
//! The report asked for this specifically: Activate should affect only what each
//! agent needs, leave no stale configuration, alias, environment variable, or
//! provider behind, and a restart must not restore the wrong state.
//!
//! Each adapter already has tests proving it preserves the settings it does not own
//! *within its own file*. What was missing is the cross-file claim: that activating
//! for Claude Code leaves Codex's `config.toml` byte-identical, and the reverse.
//! That is the property a user actually relies on when they switch one agent to a
//! gateway and expect the other to keep working.
//!
//! # Why this is one test in its own file
//!
//! The adapters resolve their configuration directory from `CLAUDE_CONFIG_DIR` and
//! `CODEX_HOME`, which are process-global. Two tests in one binary would race on
//! them. A separate integration test file is a separate process, and a single
//! `#[test]` function is sequential inside it, so the sequence below is genuinely
//! deterministic rather than merely usually passing.

use std::path::{Path, PathBuf};

use eclyzer_bridge_lib::adapters::claude::ClaudeAdapter;
use eclyzer_bridge_lib::adapters::codex::CodexAdapter;
use eclyzer_bridge_lib::adapters::{AgentAdapter, ApplyOptions};
use eclyzer_bridge_lib::backup::BackupManager;
use eclyzer_bridge_lib::profiles::{AdvancedOptions, AgentId, AuthMode, Profile};

/// A Claude settings file containing settings Bridge does not own.
const CLAUDE_BEFORE: &str = r#"{
  "hasCompletedOnboarding": true,
  "theme": "dark",
  "autoUpdatesChannel": "stable",
  "modelSettings": { "opus": "high" }
}
"#;

/// A Codex config containing a provider and settings Bridge does not own.
const CODEX_BEFORE: &str = r#"# My Codex config - do not lose this comment
approval_policy = "on-request"
model = "my-own-model"

[sandbox_workspace_write]
network_access = true

[mcp_servers.mine]
command = "node"
"#;

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("create dir");
    std::fs::write(path, content).expect("write fixture");
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn profile(name: &str, model: &str, agent: AgentId) -> Profile {
    let mut p = Profile::new(
        name.to_string(),
        name.to_string(),
        "https://gateway.example.com".to_string(),
        model.to_string(),
        vec![agent],
    );
    p.advanced = AdvancedOptions {
        auth_mode: AuthMode::XApiKey,
        ..Default::default()
    };
    p
}

fn options<'a>() -> ApplyOptions<'a> {
    ApplyOptions {
        secret: Some("sk-isolation-key"),
        helper_path: None,
        expected_sha256: None,
        agent_version: None,
    }
}

#[test]
fn activating_one_agent_leaves_the_others_configuration_byte_identical() {
    let root = tempfile::tempdir().expect("a temp root");
    let claude_home: PathBuf = root.path().join("claude-home");
    let codex_home: PathBuf = root.path().join("codex-home");

    // Point both adapters at scratch directories. `tempfile::TempDir` is dropped at
    // the end of the test, and the env vars are only read while it is alive.
    unsafe {
        std::env::set_var("CLAUDE_CONFIG_DIR", &claude_home);
        std::env::set_var("CODEX_HOME", &codex_home);
    }

    let claude_settings = claude_home.join("settings.json");
    let codex_config = codex_home.join("config.toml");
    write(&claude_settings, CLAUDE_BEFORE);
    write(&codex_config, CODEX_BEFORE);

    // Sanity: the adapters really are pointed at the scratch directories, otherwise
    // this test would silently be operating on the user's real configuration.
    assert_eq!(
        ClaudeAdapter::config_dir().expect("resolves").0,
        claude_home,
        "the Claude adapter is not using the scratch directory"
    );
    assert_eq!(
        CodexAdapter::config_dir().expect("resolves").0,
        codex_home,
        "the Codex adapter is not using the scratch directory"
    );

    let backups = root.path().join("backups");
    let claude = ClaudeAdapter::new(BackupManager::new(backups.join("claude")));
    let codex = CodexAdapter::new(BackupManager::new(backups.join("codex")));

    // --- Activate for Claude Code -------------------------------------------
    let claude_profile = profile("claude-gw", "claude-wire-model", AgentId::Claude);
    claude
        .apply_profile(&claude_profile, &options())
        .expect("Claude activation succeeds");

    let claude_after_activate = read(&claude_settings);
    let codex_after_claude_activation = read(&codex_config);

    assert_ne!(
        claude_after_activate, CLAUDE_BEFORE,
        "Claude activation must actually change Claude's settings"
    );
    assert_eq!(
        codex_after_claude_activation, CODEX_BEFORE,
        "activating for Claude Code changed Codex's config.toml"
    );

    // The unrelated Claude settings must still be there, so activation is additive
    // rather than a rewrite.
    for kept in [
        "hasCompletedOnboarding",
        "\"theme\": \"dark\"",
        "modelSettings",
    ] {
        assert!(
            claude_after_activate.contains(kept),
            "Claude activation dropped {kept}"
        );
    }

    // --- Activate for Codex -------------------------------------------------
    let codex_profile = profile("codex-gw", "codex-wire-model", AgentId::Codex);
    codex
        .apply_profile(&codex_profile, &options())
        .expect("Codex activation succeeds");

    let codex_after_activate = read(&codex_config);
    let claude_after_codex_activation = read(&claude_settings);

    assert_ne!(
        codex_after_activate, CODEX_BEFORE,
        "Codex activation must actually change Codex's config"
    );
    assert_eq!(
        claude_after_codex_activation, claude_after_activate,
        "activating for Codex changed Claude's settings.json"
    );

    // Codex must keep the user's comment, their unrelated settings, and the model
    // they had chosen is replaced only because activation is meant to replace it.
    for kept in [
        "# My Codex config - do not lose this comment",
        "approval_policy",
        "network_access",
        "mcp_servers.mine",
    ] {
        assert!(
            codex_after_activate.contains(kept),
            "Codex activation dropped {kept}"
        );
    }

    // --- Reset for Claude must also leave Codex alone -----------------------
    let codex_before_claude_reset = read(&codex_config);
    claude
        .reset_to_stock_defaults(&options())
        .expect("Claude reset succeeds");

    assert_eq!(
        read(&codex_config),
        codex_before_claude_reset,
        "resetting Claude Code changed Codex's config.toml"
    );

    // And the reset must have removed the gateway from Claude's settings, which is
    // what makes it a reset rather than a no-op.
    let claude_after_reset = read(&claude_settings);
    assert!(
        !claude_after_reset.contains("gateway.example.com"),
        "the Claude reset left the gateway endpoint behind:\n{claude_after_reset}"
    );
    assert!(
        !claude_after_reset.contains("claude-wire-model"),
        "the Claude reset left the model selection behind:\n{claude_after_reset}"
    );
    // The user's own unrelated settings survive a reset.
    for kept in [
        "hasCompletedOnboarding",
        "\"theme\": \"dark\"",
        "modelSettings",
    ] {
        assert!(
            claude_after_reset.contains(kept),
            "the Claude reset dropped {kept}"
        );
    }

    // --- Reset for Codex must leave Claude alone ---------------------------
    let claude_before_codex_reset = read(&claude_settings);
    codex
        .reset_to_stock_defaults(&options())
        .expect("Codex reset succeeds");

    assert_eq!(
        read(&claude_settings),
        claude_before_codex_reset,
        "resetting Codex changed Claude's settings.json"
    );

    let codex_after_reset = read(&codex_config);
    assert!(
        !codex_after_reset.contains("gateway.example.com"),
        "the Codex reset left the gateway endpoint behind:\n{codex_after_reset}"
    );
    assert!(
        codex_after_reset.contains("# My Codex config - do not lose this comment"),
        "the Codex reset dropped the user's comment:\n{codex_after_reset}"
    );
    assert!(
        codex_after_reset.contains("approval_policy"),
        "the Codex reset dropped an unrelated setting:\n{codex_after_reset}"
    );
    // --- Each agent writes only its own file ---------------------------------
    //
    // A structural check to go with the behavioural ones above: the paths the two
    // adapters report are in different directories with different names, so no write
    // can land in both. This lives in the same test function because both would set
    // the same process-global variables, and two tests in one binary run in
    // parallel; splitting them made the first one observe the second one's
    // directory, which is how this was noticed.
    let claude_settings_path = ClaudeAdapter::settings_path().expect("resolves");
    let codex_config_path = CodexAdapter::config_dir()
        .expect("resolves")
        .0
        .join("config.toml");

    assert_ne!(claude_settings_path, codex_config_path);
    assert!(
        claude_settings_path.starts_with(&claude_home),
        "Claude writes outside its own directory: {}",
        claude_settings_path.display()
    );
    assert!(
        codex_config_path.starts_with(&codex_home),
        "Codex writes outside its own directory: {}",
        codex_config_path.display()
    );
    assert_eq!(
        claude_settings_path.file_name().and_then(|n| n.to_str()),
        Some("settings.json")
    );
    assert_eq!(
        codex_config_path.file_name().and_then(|n| n.to_str()),
        Some("config.toml")
    );
}
