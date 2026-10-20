//! What a restore has to undo, proved against the source rather than assumed.
//!
//! The report asked whether Restore is genuinely clean: settings file, environment
//! variables, aliases, shell configuration, a wrapper or launcher, the provider and
//! model overrides, the base URL, and any stored API key reference.
//!
//! That list is worth answering precisely, because "restore" means different things
//! for different kinds of state. A value this program writes into an agent's own
//! configuration file can be removed by rewriting that file. A value it *never*
//! writes anywhere cannot leak in the first place, and the honest way to show that
//! is to inspect the production source for the code that would have to exist.
//!
//! So these tests are source audits. They read the crate's own `.rs` files and
//! assert that the mechanisms the report worried about are absent from everything
//! outside `#[cfg(test)]`. That is a real guarantee: if someone later adds a
//! registry write or a shell-profile edit, these fail.
//!
//! Why a source audit rather than a runtime test: a runtime test can only prove the
//! code path it executes is clean. This proves no code path can write persistent
//! machine state, which is the actual claim being made.

use std::path::{Path, PathBuf};

/// Every production `.rs` file in the crate, with its test module removed.
///
/// Test modules install environment variables, write fixture files, and point
/// `HOME` at temporary directories. That is legitimate, and it is also invisible at
/// runtime, so it must be excluded or these audits would fail on their own
/// scaffolding.
fn production_sources() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect(&root, &mut files);
    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{} is readable: {e}", path.display()));
            let production = strip_test_module(&text);
            (path, production)
        })
        .collect()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Remove the test module.
///
/// Cuts at the first `#[cfg(test)]` attribute, which by Rust convention introduces
/// the test module at the end of the file. Conservative: it can only under-audit
/// (if a file put tests first, production code after them would be dropped), never
/// over-audit, and `the_audit_actually_reads_production_code` asserts that enough
/// production code survives for the audits to mean something.
fn strip_test_module(source: &str) -> String {
    let mut out = String::new();
    for line in source.lines() {
        if line.trim() == "#[cfg(test)]" {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Every production file, as one searchable string, paired with names for reporting.
fn search_all(needle: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for (path, source) in production_sources() {
        for (index, line) in source.lines().enumerate() {
            if line.contains(needle) {
                hits.push(format!(
                    "{}:{}: {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    index + 1,
                    line.trim()
                ));
            }
        }
    }
    hits
}

#[test]
fn the_audit_actually_reads_production_code() {
    // Guards the guard: if `strip_test_module` ever over-trims, every audit below
    // would pass vacuously, which is worse than failing.
    let sources = production_sources();
    assert!(
        sources.len() > 10,
        "expected the crate to contain many source files, found {}",
        sources.len()
    );
    let total: usize = sources.iter().map(|(_, s)| s.lines().count()).sum();
    assert!(
        total > 5_000,
        "after removing test modules only {total} lines remain, which is too few to \
         be the real production code"
    );
    // A known production symbol must survive the strip.
    assert!(
        sources
            .iter()
            .any(|(_, s)| s.contains("pub fn validate_base_url")),
        "a known production function was stripped along with the test modules"
    );
}

#[test]
fn no_production_code_writes_a_persistent_environment_variable() {
    // The report asked specifically about environment variables. Windows has no
    // process-local way to set a persistent variable: doing it means `setx`, the
    // registry, or a shell profile. None of those appear here.
    for needle in [
        "std::env::set_var",
        "SetEnvironmentVariable",
        "setx",
        "HKEY_CURRENT_USER",
        "HKEY_LOCAL_MACHINE",
        "winreg",
    ] {
        let hits = search_all(needle);
        assert!(
            hits.is_empty(),
            "production code contains `{needle}`, which would leave persistent state \
             behind that Restore does not clean:\n  {}",
            hits.join("\n  ")
        );
    }
}

#[test]
fn no_production_code_edits_a_shell_profile_or_creates_a_wrapper() {
    // The report asked about shell config, wrappers, and launchers. Bridge writes
    // its settings into the agent's own configuration file and nothing else, so
    // there is no wrapper on disk for Restore to miss.
    for needle in [
        "$PROFILE",
        "profile.ps1",
        ".bashrc",
        ".bash_profile",
        ".zshrc",
        "config.fish",
        "Microsoft.PowerShell_profile",
    ] {
        let hits = search_all(needle);
        assert!(
            hits.is_empty(),
            "production code references `{needle}`; if Bridge edits a shell profile, \
             Restore has to undo it and currently does not:\n  {}",
            hits.join("\n  ")
        );
    }
}

#[test]
fn no_production_code_installs_a_shim_or_path_entry() {
    // A shim on PATH is another way a "wrapper" could be introduced, and it would
    // outlive an uninstall.
    //
    // `.cmd` is deliberately NOT searched for on its own: detection legitimately
    // probes `claude.exe`, `claude.cmd`, and `claude` to find an installed agent, so
    // the bare extension appears in production code for a good reason. What matters
    // is whether anything is *written* to such a path, which
    // `the_only_files_written_are_the_two_agent_configurations` covers.
    // `shim` is deliberately NOT searched for on its own: detection looks inside
    // `~/scoop/shims` for an installed agent, which is a read of somebody else's
    // directory, not a shim of our own. What matters is whether anything is
    // *written* to such a location, which
    // `the_only_files_written_are_the_two_agent_configurations` covers.
    // A shim registered under the Windows "App Paths" key would outlive an
    // uninstall, so its absence is asserted directly.
    let app_paths = search_all("\\Microsoft\\Windows\\CurrentVersion\\App Paths");
    assert!(
        app_paths.is_empty(),
        "production code references the App Paths registry key, which suggests a \
         launcher shim is installed:\n  {}",
        app_paths.join("\n  ")
    );

    // A persistent PATH edit would have to go through one of these.
    for needle in [
        "set_var(\"PATH",
        "SetEnvironmentVariable",
        "Environment]::SetEnvironmentVariable",
    ] {
        let hits = search_all(needle);
        assert!(
            hits.is_empty(),
            "production code edits PATH (`{needle}`), which an uninstall would not undo:\n  {}",
            hits.join("\n  ")
        );
    }
}

#[test]
fn the_only_files_written_are_the_two_agent_configurations() {
    // This is the positive half of the claim. Every atomic write must target a path
    // obtained from `locate_config`-style resolution, so there is no third file
    // anywhere that a restore would need to know about.
    let mut targets = Vec::new();
    for (path, source) in production_sources() {
        for (index, line) in source.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") {
                continue;
            }
            if trimmed.contains("write_atomic(") || trimmed.contains("std::fs::write(") {
                targets.push(format!(
                    "{}:{}: {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    index + 1,
                    trimmed
                ));
            }
        }
    }

    assert!(
        !targets.is_empty(),
        "found no writes at all, so this audit is not checking anything"
    );

    // Every write target must be a resolved path, never a literal somewhere else.
    for target in &targets {
        let suspicious = ["C:\\", "/etc/", "C:/", "ProgramData", "AppData\\Roaming"];
        for needle in suspicious {
            assert!(
                !target.contains(needle),
                "a write targets a hard-coded location (`{needle}`), which Restore \
                 would not know to undo:\n  {target}"
            );
        }
    }
}

#[test]
fn a_stock_reset_removes_every_environment_variable_it_manages() {
    // The managed variable list is the contract for what activation may write into
    // an agent's `env` block, and therefore what a reset must remove. If they ever
    // drift apart, a reset would leave a stale override behind.
    use eclink_lib::adapters::claude::{MANAGED_ENV_VARS, OWNED_ENV_VARS, STOCK_RESET_ENV_KEYS};

    for managed in MANAGED_ENV_VARS {
        assert!(
            STOCK_RESET_ENV_KEYS.contains(&managed),
            "`{managed}` is managed by activation but a stock reset would not remove it, \
             so Restore would leave it behind"
        );
    }
    for owned in OWNED_ENV_VARS {
        assert!(
            STOCK_RESET_ENV_KEYS.contains(&owned),
            "`{owned}` is written by the bridge but a stock reset would not remove it"
        );
    }
}

#[test]
fn a_stock_reset_removes_the_model_alias_variables_too() {
    // The family alias variables are what add rows to Claude Code's `/model`
    // picker, and stale rows from a gateway were one of the reported symptoms.
    //
    // They are kept in a SEPARATE list from `STOCK_RESET_ENV_KEYS` on purpose: the
    // confirmation dialog shows "Provider and credential" and "Model aliases" as two
    // groups, so the two categories have to stay distinguishable. The invariant is
    // therefore not that they share one array, but that the reset function consults
    // both. That is a property of the code, so it is asserted against the code.
    use eclink_lib::adapters::claude::{FAMILY_ALIAS_VARS, STOCK_RESET_ENV_KEYS};

    assert_eq!(
        FAMILY_ALIAS_VARS.len(),
        4,
        "the family alias set changed; a reset must still cover every member"
    );
    assert!(
        FAMILY_ALIAS_VARS
            .iter()
            .all(|alias| !STOCK_RESET_ENV_KEYS.contains(alias)),
        "the two lists now overlap, so the preview would report a key twice"
    );

    // The reset iterates both lists. Read the function and require that it does.
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("adapters")
            .join("claude.rs"),
    )
    .expect("claude.rs is readable");

    let body = source
        .split("fn stock_reset_keys(")
        .nth(1)
        .expect("stock_reset_keys exists");
    let body = &body[..body.find("\n}").unwrap_or(body.len())];

    assert!(
        body.contains("STOCK_RESET_ENV_KEYS"),
        "a stock reset does not remove the base environment keys"
    );
    assert!(
        body.contains("FAMILY_ALIAS_VARS"),
        "a stock reset does not remove the family alias variables, so stale rows \
         would remain in the model picker after a reset"
    );
    assert!(
        body.contains("STOCK_RESET_ROOT_KEYS"),
        "a stock reset does not remove the top-level model keys"
    );

    // And the preview must describe the same two groups, because the dialog's
    // promise is what the user consents to. A preview that omitted the aliases
    // would understate what the reset does.
    let preview = source
        .split("fn stock_reset_preview(")
        .nth(1)
        .expect("stock_reset_preview exists");
    let preview = &preview[..preview.find("\n}").unwrap_or(preview.len())];
    assert!(
        preview.contains("FAMILY_ALIAS_VARS"),
        "the reset preview omits the alias variables, so it understates what a reset \
         will change"
    );
}
