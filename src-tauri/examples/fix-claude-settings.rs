//! One-shot maintenance tool: apply the corrected Claude settings to a real
//! `~/.claude/settings.json`.
//!
//! This is not a test and not part of the shipped application. It exists because
//! the fix needed to be applied to a config that already contained the stale
//! values, and doing that by hand would have meant editing JSON with a text
//! editor — exactly the kind of blind replacement this project exists to avoid.
//!
//! It uses the real adapter and the real safety sequence, so the change goes
//! through the same read → validate → snapshot → patch → verify path the UI uses.
//!
//! It lives in `examples/` rather than `src/bin/` deliberately. Tauri bundles
//! every binary cargo produces, so a tool placed in `src/bin/` is shipped inside
//! the installer — which happened, and was caught by checking the installed
//! directory rather than by reading the config, since `bundle.resources` does not
//! describe it. `examples/` is built only when asked for and is never bundled.
//!
//! Run with the target directory as the only argument:
//!
//! ```text
//! cargo run --example fix-claude-settings -- "$env:USERPROFILE\.claude"
//! ```

use std::path::PathBuf;

use eclyzer_bridge_lib::adapters::claude::{stock_reset_preview, ClaudeAdapter};
use eclyzer_bridge_lib::adapters::{AgentAdapter, ApplyOptions};
use eclyzer_bridge_lib::backup::BackupManager;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(target) = args.next() else {
        eprintln!("usage: fix-claude-settings <path-to-.claude-directory>");
        std::process::exit(2);
    };
    let dir = PathBuf::from(target);
    let settings = dir.join("settings.json");
    if !settings.exists() {
        eprintln!("no settings.json at {}", settings.display());
        std::process::exit(2);
    }

    // The adapter resolves its directory from this variable, so this tool
    // operates on exactly the file the user passes and nothing else.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &dir) };

    let before = std::fs::read_to_string(&settings).expect("read settings");
    println!("=== BEFORE ===\n{before}");

    let preview = stock_reset_preview();
    println!("=== WOULD REMOVE ===");
    for (key, value) in preview
        .env_keys
        .iter()
        .chain(preview.alias_keys.iter())
        .chain(preview.root_keys.iter())
    {
        println!("  env/root {key} = {value}");
    }
    println!("  total: {}", preview.total());

    // Backups live outside the Claude directory so a mistake cannot take the
    // snapshots with it.
    let backup_root = std::env::temp_dir().join("eclyzer-fix-backups");
    let adapter = ClaudeAdapter::new(BackupManager::new(backup_root.clone()));
    println!("backups: {}", backup_root.display());

    let outcome = adapter
        .reset_to_stock_defaults(&ApplyOptions {
            secret: None,
            helper_path: None,
            expected_sha256: None,
            agent_version: None,
        })
        .expect("the reset must succeed through the normal safe sequence");

    println!("=== REMOVED ===\n{:#?}", outcome.managed_keys);
    println!("snapshot: {}", outcome.snapshot_id);

    let after = std::fs::read_to_string(&settings).expect("read settings");
    println!("=== AFTER ===\n{after}");
}
