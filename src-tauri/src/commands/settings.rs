//! Settings commands (PRD §44.2, §54).

use serde::{Deserialize, Serialize};

use crate::app::{AppSettings, AppState};
use crate::commands::{blocking, CommandResult};

/// Everything the Settings screen needs in one round trip.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    #[serde(flatten)]
    pub settings: AppSettings,
    pub app_version: String,
    pub data_directory: String,
    pub log_directory: String,
    pub repository_url: String,
    /// Which secret backend is in use, e.g. "Windows DPAPI (Current User)".
    pub secret_backend: String,
    pub secret_count: usize,
    pub profile_count: usize,
    /// Whether the bundled credential helper was found.
    pub credential_helper_available: bool,
}

/// A partial settings update from the frontend.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    #[serde(default)]
    pub update_check: Option<bool>,
    #[serde(default)]
    pub remember_project_directory: Option<bool>,
    #[serde(default)]
    pub log_level: Option<crate::diagnostics::LogLevel>,
    #[serde(default)]
    pub first_run_complete: Option<bool>,
    #[serde(default)]
    pub cost_notice_seen: Option<bool>,
    #[serde(default)]
    pub insecure_endpoint_notice_seen: Option<bool>,
    /// Written only when the user launches with "remember this folder" on.
    #[serde(default)]
    pub last_project_directory: Option<String>,
    /// Interface language tag, e.g. `en` or `id`.
    #[serde(default)]
    pub language: Option<String>,
}

/// Read the current settings and environment facts.
#[tauri::command]
pub async fn get_settings(state: tauri::State<'_, AppState>) -> CommandResult<SettingsView> {
    let state = state.inner().clone();
    blocking(move || {
        let profile_count = state.profiles().load()?.profiles.len();
        Ok(SettingsView {
            settings: state.settings(),
            app_version: crate::APP_VERSION.to_string(),
            data_directory: state.paths().root().display().to_string(),
            log_directory: state.paths().logs_dir().display().to_string(),
            repository_url: crate::REPO_URL.to_string(),
            secret_backend: crate::secrets::backend_name().to_string(),
            secret_count: state.secrets().count()?,
            profile_count,
            credential_helper_available: state.helper_path().is_some(),
        })
    })
    .await
}

/// Apply a partial settings update.
#[tauri::command]
pub async fn update_settings(
    state: tauri::State<'_, AppState>,
    patch: SettingsPatch,
) -> CommandResult<SettingsView> {
    let state = state.inner().clone();
    blocking(move || {
        state.update_settings(|settings| apply_patch(settings, &patch))?;

        let profile_count = state.profiles().load()?.profiles.len();
        Ok(SettingsView {
            settings: state.settings(),
            app_version: crate::APP_VERSION.to_string(),
            data_directory: state.paths().root().display().to_string(),
            log_directory: state.paths().logs_dir().display().to_string(),
            repository_url: crate::REPO_URL.to_string(),
            secret_backend: crate::secrets::backend_name().to_string(),
            secret_count: state.secrets().count()?,
            profile_count,
            credential_helper_available: state.helper_path().is_some(),
        })
    })
    .await
}

/// Apply `patch` onto `settings` in place.
///
/// Split out of the command so the merge rules — which decide whether a user's
/// preference survives a partial update — can be tested directly.
fn apply_patch(settings: &mut AppSettings, patch: &SettingsPatch) {
    // Only explicitly-provided fields change, so a partial patch from an older
    // UI never resets an unrelated preference.
    if let Some(value) = patch.update_check {
        settings.update_check = value;
    }
    if let Some(value) = patch.remember_project_directory {
        settings.remember_project_directory = value;
        if !value {
            // Stop remembering, and forget what was remembered.
            settings.last_project_directory = None;
        }
    }
    if let Some(value) = patch.log_level {
        settings.log_level = value;
    }
    if let Some(value) = patch.first_run_complete {
        settings.first_run_complete = value;
    }
    if let Some(value) = patch.cost_notice_seen {
        settings.cost_notice_seen = value;
    }
    if let Some(value) = patch.insecure_endpoint_notice_seen {
        settings.insecure_endpoint_notice_seen = value;
    }
    // A remembered folder is only meaningful while remembering is on, and it
    // must be an absolute path so a later launch cannot resolve relative to an
    // unexpected working directory.
    if let Some(value) = patch.last_project_directory.as_ref() {
        let trimmed = value.trim();
        let accept = settings.remember_project_directory
            && !trimmed.is_empty()
            && std::path::Path::new(trimmed).is_absolute();
        settings.last_project_directory = if accept {
            Some(trimmed.to_string())
        } else {
            None
        };
    }
    // The language is validated against the known set rather than stored as
    // typed, so a typo cannot leave the UI with no dictionary and a blank screen.
    if let Some(value) = patch.language.as_ref() {
        let normalised = value.trim().to_ascii_lowercase();
        if SUPPORTED_LANGUAGES.contains(&normalised.as_str()) {
            settings.language = normalised;
        }
    }
}

/// Interface languages this build ships translations for.
///
/// Kept in the backend so a value written by any client is validated in one
/// place; the frontend dictionary list must match it.
pub const SUPPORTED_LANGUAGES: [&str; 2] = ["en", "id"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_patch_changes_nothing() {
        // Start from a non-default state so the assertions below can prove that
        // an absent field is left alone rather than merely left at its default.
        let mut settings = AppSettings {
            update_check: false,
            cost_notice_seen: true,
            log_level: crate::diagnostics::LogLevel::Warn,
            ..AppSettings::default()
        };

        apply_patch(&mut settings, &SettingsPatch::default());

        assert!(!settings.update_check, "an absent field must not be reset");
        assert!(
            settings.cost_notice_seen,
            "an absent field must not be reset"
        );
        assert_eq!(
            settings.log_level,
            crate::diagnostics::LogLevel::Warn,
            "an absent field must not be reset"
        );
        assert!(settings.remember_project_directory, "left at its default");
        assert_eq!(settings.last_project_directory, None);
    }

    #[test]
    fn a_present_field_is_applied() {
        let mut settings = AppSettings::default();
        let patch = SettingsPatch {
            update_check: Some(false),
            ..SettingsPatch::default()
        };
        apply_patch(&mut settings, &patch);
        assert!(!settings.update_check);
    }

    #[test]
    fn turning_off_remembering_forgets_the_stored_folder() {
        let mut settings = AppSettings {
            remember_project_directory: true,
            last_project_directory: Some(r"C:\Projects\app".into()),
            ..AppSettings::default()
        };
        let patch = SettingsPatch {
            remember_project_directory: Some(false),
            ..SettingsPatch::default()
        };

        apply_patch(&mut settings, &patch);

        assert!(!settings.remember_project_directory);
        // Keeping it would leave a path on disk that the user asked to forget.
        assert_eq!(settings.last_project_directory, None);
    }

    #[test]
    fn an_absolute_folder_is_remembered_while_remembering_is_on() {
        let mut settings = AppSettings {
            remember_project_directory: true,
            ..AppSettings::default()
        };
        let patch = SettingsPatch {
            last_project_directory: Some(r"C:\Projects\app".into()),
            ..SettingsPatch::default()
        };

        apply_patch(&mut settings, &patch);

        assert_eq!(
            settings.last_project_directory.as_deref(),
            Some(r"C:\Projects\app")
        );
    }

    #[test]
    fn a_relative_folder_is_refused() {
        // A relative path would resolve against whatever working directory the
        // launch happens to inherit, which is not what the user chose.
        let mut settings = AppSettings {
            remember_project_directory: true,
            last_project_directory: Some(r"C:\Projects\old".into()),
            ..AppSettings::default()
        };
        let patch = SettingsPatch {
            last_project_directory: Some(r"..\somewhere".into()),
            ..SettingsPatch::default()
        };

        apply_patch(&mut settings, &patch);

        assert_eq!(settings.last_project_directory, None);
    }

    #[test]
    fn a_folder_is_refused_while_remembering_is_off() {
        let mut settings = AppSettings {
            remember_project_directory: false,
            ..AppSettings::default()
        };
        let patch = SettingsPatch {
            last_project_directory: Some(r"C:\Projects\app".into()),
            ..SettingsPatch::default()
        };

        apply_patch(&mut settings, &patch);

        assert_eq!(settings.last_project_directory, None);
    }

    #[test]
    fn a_folder_with_surrounding_whitespace_is_trimmed() {
        let mut settings = AppSettings {
            remember_project_directory: true,
            ..AppSettings::default()
        };
        let patch = SettingsPatch {
            last_project_directory: Some("  C:\\Projects\\app  ".into()),
            ..SettingsPatch::default()
        };

        apply_patch(&mut settings, &patch);

        assert_eq!(
            settings.last_project_directory.as_deref(),
            Some(r"C:\Projects\app")
        );
    }

    #[test]
    fn an_empty_folder_clears_the_stored_one() {
        let mut settings = AppSettings {
            remember_project_directory: true,
            last_project_directory: Some(r"C:\Projects\old".into()),
            ..AppSettings::default()
        };
        let patch = SettingsPatch {
            last_project_directory: Some("   ".into()),
            ..SettingsPatch::default()
        };

        apply_patch(&mut settings, &patch);

        assert_eq!(settings.last_project_directory, None);
    }

    #[test]
    fn a_patch_touching_one_field_leaves_the_others_alone() {
        let mut settings = AppSettings {
            update_check: true,
            remember_project_directory: true,
            log_level: crate::diagnostics::LogLevel::Info,
            first_run_complete: true,
            cost_notice_seen: true,
            insecure_endpoint_notice_seen: true,
            ..AppSettings::default()
        };
        let patch = SettingsPatch {
            update_check: Some(false),
            ..SettingsPatch::default()
        };

        apply_patch(&mut settings, &patch);

        assert!(!settings.update_check);
        assert!(settings.remember_project_directory);
        assert_eq!(settings.log_level, crate::diagnostics::LogLevel::Info);
        assert!(settings.first_run_complete);
        assert!(settings.cost_notice_seen);
        assert!(settings.insecure_endpoint_notice_seen);
    }

    #[test]
    fn the_default_language_is_english() {
        // The product decision: an installation starts in English regardless of the
        // machine's locale, and the user opts into Indonesian.
        assert_eq!(AppSettings::default().language, "en");
    }

    #[test]
    fn a_supported_language_is_stored() {
        for language in SUPPORTED_LANGUAGES {
            let mut settings = AppSettings::default();
            apply_patch(
                &mut settings,
                &SettingsPatch {
                    language: Some(language.to_string()),
                    ..SettingsPatch::default()
                },
            );
            assert_eq!(settings.language, language);
        }
    }

    #[test]
    fn a_language_tag_is_normalised_before_it_is_stored() {
        let mut settings = AppSettings::default();
        apply_patch(
            &mut settings,
            &SettingsPatch {
                language: Some("  ID  ".to_string()),
                ..SettingsPatch::default()
            },
        );
        assert_eq!(settings.language, "id");
    }

    #[test]
    fn an_unsupported_language_is_refused_rather_than_stored() {
        // Storing it would leave the interface with no dictionary to read and render
        // a blank screen. The previous choice must survive a bad value.
        let mut settings = AppSettings {
            language: "id".to_string(),
            ..AppSettings::default()
        };
        for bad in ["fr", "", "  ", "english", "en-US", "<script>"] {
            apply_patch(
                &mut settings,
                &SettingsPatch {
                    language: Some(bad.to_string()),
                    ..SettingsPatch::default()
                },
            );
            assert_eq!(
                settings.language, "id",
                "\"{bad}\" was accepted as a language"
            );
        }
    }

    #[test]
    fn a_patch_without_a_language_leaves_the_choice_alone() {
        // Every other screen sends partial patches, so a missing field must not
        // reset the language.
        let mut settings = AppSettings {
            language: "id".to_string(),
            ..AppSettings::default()
        };
        apply_patch(
            &mut settings,
            &SettingsPatch {
                update_check: Some(false),
                ..SettingsPatch::default()
            },
        );
        assert_eq!(settings.language, "id");
    }

    #[test]
    fn the_supported_languages_match_the_frontend_dictionary_list() {
        // The two lists have to agree or the picker would offer a language the
        // backend refuses, or the backend would accept one the frontend cannot
        // render. The frontend list is read from its source, because nothing else
        // connects the two sides.
        let i18n = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("the crate is inside the repository")
                .join("src")
                .join("lib")
                .join("i18n.ts"),
        )
        .expect("the frontend dictionary is readable");

        let declaration = i18n
            .split("export const LANGUAGES")
            .nth(1)
            .expect("LANGUAGES is declared");
        let list = &declaration[..declaration.find(']').unwrap_or(declaration.len())];

        for language in SUPPORTED_LANGUAGES {
            assert!(
                list.contains(&format!("\"{language}\"")),
                "the backend supports \"{language}\" but the frontend does not offer it"
            );
        }
        // And nothing the frontend offers is missing here.
        let offered = list.matches('"').count() / 2;
        assert_eq!(
            offered,
            SUPPORTED_LANGUAGES.len(),
            "the frontend offers {offered} languages but the backend accepts {}",
            SUPPORTED_LANGUAGES.len()
        );
    }

    #[test]
    fn a_settings_file_without_a_language_reads_as_english() {
        // The migration path: an `app.json` written before the field existed must
        // still load, and must not guess at the machine's locale.
        let parsed: AppSettings = serde_json::from_str(
            r#"{
              "schemaVersion": 1,
              "updateCheck": true,
              "rememberProjectDirectory": true,
              "logLevel": "info",
              "firstRunComplete": false,
              "costNoticeSeen": true,
              "insecureEndpointNoticeSeen": false
            }"#,
        )
        .expect("an older settings file must load");

        assert_eq!(parsed.language, "en");
        assert!(parsed.cost_notice_seen, "the other fields still load");
    }

    #[test]
    fn the_language_survives_a_real_save_and_load() {
        // The persistence claim, exercised through the filesystem rather than by
        // inspecting the in-memory value.
        let dir = tempfile::tempdir().expect("a temp dir");
        let paths = crate::paths::AppPaths::with_root(dir.path());
        paths.ensure_dirs().expect("create dirs");

        let mut settings = crate::state::load_settings(&paths).expect("load defaults");
        assert_eq!(settings.language, "en", "a fresh install starts in English");

        apply_patch(
            &mut settings,
            &SettingsPatch {
                language: Some("id".to_string()),
                ..SettingsPatch::default()
            },
        );
        crate::state::save_settings(&paths, &settings).expect("save");

        let reloaded = crate::state::load_settings(&paths).expect("reload");
        assert_eq!(
            reloaded.language, "id",
            "the chosen language did not survive a restart"
        );
    }
}
