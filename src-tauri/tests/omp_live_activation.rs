//! Opt-in live OMP activation check.
//!
//! This test writes only to a temporary OMP agent directory. It uses the real
//! installed `omp` binary and the real Eclink credential helper, because the
//! point is to prove the generated `!command` form is accepted by OMP itself.
//!
//! Run it explicitly:
//!
//! ```text
//! $env:ECLINK_LIVE_OMP_PROFILE_ID = "<an Eclink profile id with a stored key>"
//! $env:ECLINK_LIVE_OMP_MODEL = "<a model that profile's gateway serves>"
//! cargo test -p eclink --test omp_live_activation -- --ignored --nocapture
//! ```
//!
//! It is ignored by default so CI never depends on a user's machine.

use std::path::PathBuf;

use eclink_lib::adapters::{AgentAdapter, ApplyOptions, OmpAdapter, OmpSelection};
use eclink_lib::backup::BackupManager;
use eclink_lib::profiles::{AgentId, OmpApiProtocol, OmpOptions, Profile};

#[test]
#[ignore = "requires a real OMP install and a stored Eclink profile"]
fn live_activation_writes_a_provider_omp_accepts() {
    let Ok(profile_id) = std::env::var("ECLINK_LIVE_OMP_PROFILE_ID") else {
        eprintln!("ECLINK_LIVE_OMP_PROFILE_ID is not set; skipping");
        return;
    };
    let model =
        std::env::var("ECLINK_LIVE_OMP_MODEL").unwrap_or_else(|_| "hisyam-6.1-sol".to_string());

    let root = std::env::temp_dir().join(format!("eclink-omp-live-{}", uuid::Uuid::new_v4()));
    let agent_dir = root.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("config.yml"),
        "modelRoles:\n  default: user/original\n  vision: user/vision\n",
    )
    .unwrap();
    // SAFETY: this test owns the variable for its whole duration and is run in
    // its own test process.
    unsafe { std::env::set_var("PI_CODING_AGENT_DIR", &agent_dir) };

    let helper = PathBuf::from(std::env::var("ECLINK_LIVE_HELPER").unwrap_or_else(|_| {
        let cwd = std::env::current_dir().unwrap();
        [
            cwd.join("target/release/eclink-helper.exe"),
            cwd.join("../target/release/eclink-helper.exe"),
        ]
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| cwd.join("../target/release/eclink-helper.exe"))
        .display()
        .to_string()
    }));
    assert!(helper.is_file(), "build the helper first: {helper:?}");

    let adapter = OmpAdapter::new(
        BackupManager::new(root.join("backups")),
        root.join("omp-ledger.json"),
        OmpSelection::new("default"),
    );

    let mut profile = Profile::new(
        "Live OMP check".to_string(),
        "Live Gateway".to_string(),
        "http://192.168.1.222:20128/v1".to_string(),
        model.clone(),
        vec![AgentId::Omp],
    );
    profile.id = profile_id.clone();
    profile.omp = OmpOptions {
        protocol: OmpApiProtocol::OpenAiCompletions,
        default_model: None,
        smol: None,
        slow: None,
        plan: None,
        model_name: Some("Live OMP check".to_string()),
        context_window: Some(128_000),
        max_tokens: Some(16_384),
    };

    let outcome = adapter
        .apply_profile(
            &profile,
            &ApplyOptions {
                secret: None,
                helper_path: Some(helper.clone()),
                expected_sha256: None,
                agent_version: None,
            },
        )
        .expect("activation should succeed");
    assert_eq!(
        outcome.active.kind,
        eclink_lib::adapters::ActiveConfigurationKind::Managed,
        "OMP must report the provider as active"
    );

    let models = std::fs::read_to_string(agent_dir.join("models.yml")).unwrap();
    assert!(models.contains("eclink-omp-"), "{models}");
    assert!(models.contains("secret resolve"), "{models}");
    assert!(
        !models.to_lowercase().contains("sk-"),
        "no literal credential may be written"
    );

    // Activating a second Eclink profile in the same OMP profile must retire the
    // first provider instead of leaving both registered.
    let first_provider = outcome
        .active
        .provider_id
        .clone()
        .expect("managed provider id");
    let mut second = profile.clone();
    second.id = "00000000-0000-0000-0000-000000000002".to_string();
    second.name = "Second OMP check".to_string();
    adapter
        .apply_profile(
            &second,
            &ApplyOptions {
                secret: None,
                helper_path: Some(helper),
                expected_sha256: None,
                agent_version: None,
            },
        )
        .expect("second activation should succeed");
    let models = std::fs::read_to_string(agent_dir.join("models.yml")).unwrap();
    assert!(
        !models.contains(&first_provider),
        "the previous Eclink provider must be removed: {models}"
    );
    assert!(models.contains("eclink-omp-"), "{models}");

    // The original and the snapshot are sealed, so neither may contain a key.
    for entry in walk(&root.join("backups")) {
        if entry.extension().and_then(|ext| ext.to_str()) == Some("content") {
            let text = std::fs::read_to_string(&entry).unwrap();
            assert!(
                !text.contains("sk-"),
                "backup {} contains a credential",
                entry.display()
            );
        }
    }

    let restored = adapter
        .restore_default(&ApplyOptions {
            secret: None,
            helper_path: None,
            expected_sha256: None,
            agent_version: None,
        })
        .expect("restore default should succeed");
    assert_ne!(
        restored.active.kind,
        eclink_lib::adapters::ActiveConfigurationKind::Managed
    );
    let models = std::fs::read_to_string(agent_dir.join("models.yml")).unwrap();
    assert!(!models.contains("eclink-omp-"), "{models}");
    let config = std::fs::read_to_string(agent_dir.join("config.yml")).unwrap();
    assert!(config.contains("default: user/original"), "{config}");
    assert!(config.contains("vision: user/vision"), "{config}");

    unsafe { std::env::remove_var("PI_CODING_AGENT_DIR") };
    let _ = std::fs::remove_dir_all(&root);
}

fn walk(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
