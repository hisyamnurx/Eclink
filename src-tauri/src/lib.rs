//! Eclink — a local configuration bridge for AI coding CLIs.
//!
//! Eclink lets a user point Codex or Claude Code at an OpenAI-compatible
//! or Anthropic-compatible provider, without hand-editing `config.toml` or
//! `settings.json` and without pasting an API key into a file that is easy to
//! commit by accident.
//!
//! ## Design commitments enforced by this crate
//!
//! * **Nothing is replaced wholesale.** Agent configuration is always read,
//!   parsed, validated, snapshotted, patched key-by-key, written atomically, and
//!   re-read to verify. An unparsable config is a hard stop.
//! * **Keys never land in a config file, a log, or a backup** where that can be
//!   avoided. Codex is configured with a credential-helper command, so the token
//!   is fetched on demand from DPAPI-sealed storage.
//! * **Every change is reversible.** An immutable original backup is taken once
//!   per agent, a snapshot before every mutation, and a ledger records what
//!   happened so Undo has a target.
//! * **No telemetry.** The only outbound traffic is a provider test or
//!   playground call the user starts, a GitHub Releases update check, and links
//!   the user opens themselves.

pub mod adapters;
pub mod app;
pub mod backup;
pub mod commands;
pub mod config;
pub mod diagnostics;
pub mod error;
pub mod paths;
pub mod process;
pub mod profiles;
pub mod secrets;
pub mod state;
pub mod testing;
pub mod updater;

pub use error::{BridgeError, BridgeResult};

/// Version of the running build.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Project home, opened only when the user asks.
pub const REPO_URL: &str = "https://github.com/hisyamnurx/Eclink";

/// Prefix that makes a provider entry recognizably Eclink-owned.
///
/// This prefix is what lets `restore_default` remove exactly Eclink's own
/// entries and never a provider the user added (PRD §16.3).
///
/// It keeps the pre-rename spelling on purpose. Every provider Eclink has ever
/// written into a user's `config.toml` carries this exact prefix, and the
/// ownership check is a string comparison against it. Renaming the prefix would
/// make Eclink treat its own entries as the user's, so `restore_default` would
/// stop removing them and `get_active_profile` would stop recognizing them.
/// See `docs/legacy-compatibility.md`.
pub const MANAGED_PROVIDER_PREFIX: &str = "eclyzer_bridge_";

/// Start the desktop application.
///
/// A bootstrap failure is reported in a native dialog rather than a silent exit,
/// because the most likely causes — an unwritable data directory, a corrupt
/// `app.json` — are things the user can act on.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = match app::builder() {
        Ok(builder) => builder,
        Err(err) => {
            report_startup_failure(&err);
            return;
        }
    };

    builder
        .setup(|app| {
            // Best-effort housekeeping: prune old logs so the folder does not
            // grow forever. `prune_logs` handles its own errors internally and
            // never blocks or fails startup.
            use tauri::Manager;
            let dir = app.state::<app::AppState>().paths().logs_dir();
            tauri::async_runtime::spawn_blocking(move || {
                diagnostics::prune_logs(&dir);
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|err| {
            report_startup_failure(
                &BridgeError::internal("Eclink could not open its window.")
                    .with_details(err.to_string()),
            );
        });
}

/// Show a startup failure to the user, falling back to stderr.
///
/// A native message box is used rather than a Tauri dialog because this runs
/// before the Tauri application exists. If even that is unavailable the message
/// still reaches stderr, so a support request can recover it.
fn report_startup_failure(err: &BridgeError) {
    let message = app::bootstrap_error_message(err);
    eprintln!("{message}");
    show_error_dialog(&message);
}

/// Best-effort native error dialog.
///
/// Uses the Win32 `MessageBoxW` through the `windows` crate already in the
/// dependency tree. Any failure is ignored: the message has already been written
/// to stderr.
#[cfg(target_os = "windows")]
fn show_error_dialog(message: &str) {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let text = HSTRING::from(message);
    let caption = HSTRING::from("Eclink");
    // SAFETY: both HSTRINGs outlive the call, and a null owner window is
    // explicitly permitted for a modal message box. The return value is ignored
    // deliberately, since there is nothing useful to do if it fails.
    unsafe {
        let _ = MessageBoxW(None, &text, &caption, MB_OK | MB_ICONERROR);
    }
}

#[cfg(not(target_os = "windows"))]
fn show_error_dialog(message: &str) {
    // Non-Windows builds are unsupported; stderr is all that is available.
    let _ = message;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_managed_prefix_cannot_collide_with_a_builtin_provider() {
        // Codex reserves `openai`, `ollama`, and `lmstudio`.
        for builtin in ["openai", "ollama", "lmstudio"] {
            assert!(
                !builtin.starts_with(MANAGED_PROVIDER_PREFIX),
                "{builtin} must never look Bridge-owned"
            );
        }
    }

    #[test]
    fn the_repository_url_has_no_trailing_slash() {
        assert!(!REPO_URL.ends_with('/'), "{REPO_URL}");
        assert!(REPO_URL.starts_with("https://"), "{REPO_URL}");
    }

    #[test]
    fn the_version_is_populated_from_the_manifest() {
        assert!(!APP_VERSION.is_empty());
        // A semver shape, so the UI can render it and the updater can compare.
        assert!(APP_VERSION.split('.').count() >= 2, "{APP_VERSION}");
    }

    #[test]
    fn bootstrap_error_copy_includes_the_hint_and_details() {
        let err = BridgeError::io("The data folder could not be created.")
            .with_hint("Check that you can write to your local app data folder.")
            .with_details("access denied");
        let message = app::bootstrap_error_message(&err);
        assert!(message.contains("could not be created"), "{message}");
        assert!(message.contains("local app data"), "{message}");
        assert!(message.contains("access denied"), "{message}");
    }
}
