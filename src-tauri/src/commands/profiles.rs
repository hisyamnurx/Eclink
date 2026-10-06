//! Profile CRUD and connection testing commands.

use serde::{Deserialize, Serialize};

use crate::adapters::AgentId;
use crate::app::AppState;
use crate::commands::{blocking, parse_agent, require_id, CommandResult};
use crate::error::{BridgeError, BridgeResult};
use crate::profiles::{AdvancedOptions, Profile, ProviderTemplate};
use crate::testing::{test_profile, TestOutcome};

/// A profile plus the information the list needs to render it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    #[serde(flatten)]
    pub profile: Profile,
    /// Whether an API key is stored for this profile.
    pub has_secret: bool,
    /// A masked hint such as `****cdef`, never the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_fingerprint: Option<String>,
    /// Whether every declared target is independently verified (PRD §18.2).
    pub ready: bool,
}

/// The payload the frontend sends when saving a profile.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInput {
    /// Absent for a new profile.
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub provider_name: String,
    pub base_url: String,
    pub model: String,
    /// A new API key to store. Absent means "leave the stored key alone".
    #[serde(default)]
    pub api_key: Option<String>,
    /// Whether to delete the stored key.
    #[serde(default)]
    pub clear_api_key: bool,
    pub targets: Vec<String>,
    #[serde(default)]
    pub advanced: Option<AdvancedOptions>,
    #[serde(default)]
    pub notes: Option<String>,
}

impl ProfileInput {
    /// Validate and convert the untrusted payload into a profile.
    fn into_profile(self, existing: Option<&Profile>) -> BridgeResult<Profile> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(BridgeError::validation("A profile name is required."));
        }
        if name.chars().count() > 80 {
            return Err(BridgeError::validation(
                "The profile name is too long (maximum 80 characters).",
            ));
        }
        // Control characters would corrupt log lines and the UI.
        if name.chars().any(|ch| ch.is_control()) {
            return Err(BridgeError::validation(
                "The profile name contains characters that are not allowed.",
            ));
        }

        let provider_name = self.provider_name.trim();
        if provider_name.is_empty() {
            return Err(BridgeError::validation("A provider name is required."));
        }

        crate::profiles::validate_base_url(&self.base_url).map_err(BridgeError::validation)?;

        let model = self.model.trim();
        if model.is_empty() {
            return Err(BridgeError::validation("A model name is required.")
                .with_hint("Enter the model ID exactly as the provider spells it."));
        }
        if model.chars().any(char::is_control) {
            return Err(BridgeError::validation(
                "The model name contains characters that are not allowed.",
            ));
        }

        // Targets come from the frontend, so validate them rather than trusting.
        let mut targets = Vec::new();
        for target in &self.targets {
            let agent = parse_agent(target)?;
            if !targets.contains(&agent) {
                targets.push(agent);
            }
        }
        if targets.is_empty() {
            return Err(BridgeError::validation(
                "Select at least one agent for this profile.",
            ));
        }
        targets.sort();

        let now = chrono::Local::now().to_rfc3339();
        let mut profile = match existing {
            Some(existing) => {
                let mut updated = existing.clone();
                updated.name = name.to_string();
                updated.provider_name = provider_name.to_string();
                updated.base_url = crate::profiles::normalize_base_url(&self.base_url);
                updated.model = model.to_string();
                updated.targets = targets;
                updated.updated_at = now;
                updated
            }
            None => {
                let mut created = Profile::new(
                    name.to_string(),
                    provider_name.to_string(),
                    crate::profiles::normalize_base_url(&self.base_url),
                    model.to_string(),
                    targets,
                );
                created.created_at = now.clone();
                created.updated_at = now;
                created
            }
        };

        if let Some(advanced) = self.advanced {
            profile.advanced = advanced;
        }
        profile.notes = self.notes.filter(|text| !text.trim().is_empty());

        profile.validate().map_err(BridgeError::validation)?;
        Ok(profile)
    }
}

/// List every stored profile.
#[tauri::command]
pub async fn list_profiles(
    state: tauri::State<'_, AppState>,
) -> CommandResult<Vec<ProfileSummary>> {
    let state = state.inner().clone();
    blocking(move || {
        let file = state.profiles().load()?;
        let mut out = Vec::new();
        for profile in file.profiles {
            out.push(summarise(&state, profile));
        }
        Ok(out)
    })
    .await
}

/// Remove cached compatibility verdicts that no longer describe the profile's
/// current inputs.
///
/// Extracted so the rule is testable without an `AppState`.
///
/// Without this, editing the base URL, model, or stored credential after a
/// successful test left the "Compatible" badge and the enabled Activate button
/// in place, because the cached record was served verbatim.
/// `compatibility_is_stale` already knew how to detect this; nothing called it.
///
/// The agent version is passed as `None` to match how the record is written
/// (`test_profile_connection` records with `None`); using a detected version
/// here would make every freshly written record look stale immediately.
fn drop_stale_verdicts(profile: &mut Profile) {
    let stale: Vec<AgentId> = [AgentId::Codex, AgentId::Claude]
        .into_iter()
        .filter(|agent| {
            profile.compatibility.contains_key(agent)
                && profile.compatibility_is_stale(*agent, None)
        })
        .collect();
    for agent in stale {
        profile.compatibility.remove(&agent);
    }
}

fn summarise(state: &AppState, mut profile: Profile) -> ProfileSummary {
    let (has_secret, fingerprint) = match profile.secret_ref.as_deref() {
        Some(reference) => match state.secrets().fingerprint(reference) {
            Ok(Some(fingerprint)) => (true, Some(fingerprint)),
            _ => (false, None),
        },
        None => (false, None),
    };

    drop_stale_verdicts(&mut profile);

    let ready = profile.is_ready();
    ProfileSummary {
        profile,
        has_secret,
        secret_fingerprint: fingerprint,
        ready,
    }
}

/// Load one profile with its current summaries.
#[tauri::command]
pub async fn get_profile(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CommandResult<ProfileSummary> {
    let state = state.inner().clone();
    let id = require_id(&id, "Profile id")?;
    blocking(move || {
        let profile = state
            .profiles()
            .load()?
            .profiles
            .into_iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| {
                BridgeError::validation("That profile no longer exists.").with_hint(
                    "It may have been deleted in another window. Reload the profile list.",
                )
            })?;
        Ok(summarise(&state, profile))
    })
    .await
}

/// The built-in provider presets, offered as a starting point (PRD §19).
#[tauri::command]
pub async fn list_provider_templates() -> CommandResult<Vec<ProviderTemplate>> {
    Ok(crate::profiles::provider_templates())
}

/// Outcome of saving a profile.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProfileResult {
    pub profile: ProfileSummary,
    /// Whether the API key changed, so the UI can say so.
    pub secret_updated: bool,
}

/// Create or update a profile.
#[tauri::command]
pub async fn save_profile(
    state: tauri::State<'_, AppState>,
    input: ProfileInput,
) -> CommandResult<SaveProfileResult> {
    let state = state.inner().clone();
    blocking(move || {
        let existing = match input.id.as_deref() {
            Some(id) => {
                let id = require_id(id, "Profile id")?;
                state
                    .profiles()
                    .load()?
                    .profiles
                    .into_iter()
                    .find(|profile| profile.id == id)
            }
            None => None,
        };

        let mut profile = input.clone().into_profile(existing.as_ref())?;
        let mut secret_updated = false;

        // A new key replaces the old one; `clear` removes it entirely. Both are
        // explicit, so a save that omits `apiKey` never destroys a stored key.
        if input.clear_api_key {
            if let Some(reference) = profile.secret_ref.clone() {
                state.secrets().delete(&reference)?;
                profile.secret_ref = None;
                secret_updated = true;
            }
        } else if let Some(key) = input.api_key.as_deref() {
            let key = key.trim();
            if key.is_empty() {
                return Err(BridgeError::validation("The API key cannot be empty.")
                    .with_hint("Use \"Remove key\" if you want to delete the stored key."));
            }
            if key.chars().any(char::is_control) {
                return Err(BridgeError::validation(
                    "The API key contains characters that are not allowed.",
                ));
            }
            // The secret is keyed by the profile id, so it can never be
            // confused with another profile's key. It is never written to
            // profiles.json.
            state.secrets().put(&profile.id, key)?;
            profile.secret_ref = Some(profile.id.clone());
            secret_updated = true;
        }

        let saved = state.profiles().update(|file| {
            match file.profiles.iter_mut().find(|item| item.id == profile.id) {
                Some(slot) => *slot = profile.clone(),
                None => file.profiles.push(profile.clone()),
            }
            Ok(file.clone())
        })?;

        let stored = saved
            .profiles
            .into_iter()
            .find(|item| item.id == profile.id)
            .unwrap_or(profile);

        Ok(SaveProfileResult {
            profile: summarise(&state, stored),
            secret_updated,
        })
    })
    .await
}

/// Whether deleting a profile is safe, and what the alternatives are.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteImpact {
    pub profile_name: String,
    /// Agents currently using this profile.
    pub active_for: Vec<AgentId>,
    /// Whether deletion should be blocked pending a restore decision.
    pub requires_decision: bool,
}

/// Explain what deleting a profile would do before doing it (PRD §22.5).
#[tauri::command]
pub async fn profile_delete_impact(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CommandResult<DeleteImpact> {
    let state = state.inner().clone();
    let id = require_id(&id, "Profile id")?;
    blocking(move || {
        let profile = state
            .profiles()
            .load()?
            .profiles
            .into_iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| BridgeError::validation("That profile no longer exists."))?;

        let mut active_for = Vec::new();
        for adapter in state.adapters() {
            if let Ok(active) = adapter.get_active_profile() {
                if active.profile_id.as_deref() == Some(profile.id.as_str()) {
                    active_for.push(adapter.id());
                }
            }
        }

        Ok(DeleteImpact {
            profile_name: profile.name,
            requires_decision: !active_for.is_empty(),
            active_for,
        })
    })
    .await
}

/// Duplicate a profile, including its stored credential (PRD §18.4).
///
/// The copy is a new profile with its own id, so the secret store gains an
/// independent record: rotating one key cannot affect the other. The plaintext
/// key never leaves the backend — `SecretStore::copy` moves it between sealed
/// records directly, rather than round-tripping through the frontend.
///
/// Compatibility verdicts are deliberately **not** copied. They describe a test
/// performed against specific inputs, and the usual reason to duplicate a
/// profile is to change the endpoint or model, at which point the inherited
/// verdict would be exactly the stale evidence the app must not present.
#[tauri::command]
pub async fn duplicate_profile(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CommandResult<ProfileSummary> {
    let state = state.inner().clone();
    let id = require_id(&id, "Profile id")?;
    blocking(move || {
        let source = state
            .profiles()
            .load()?
            .profiles
            .into_iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| BridgeError::validation("That profile no longer exists."))?;

        let mut copy = source.clone();
        copy.id = uuid::Uuid::new_v4().to_string();
        copy.name = unique_copy_name(&source.name, &state);
        copy.compatibility = Default::default();
        copy.secret_ref = None;

        // Move the credential across inside the secrets module. `copy` reports
        // false when the source had no key, which is a normal state.
        let has_secret = match source.secret_ref.as_deref() {
            Some(reference) => state.secrets().copy(reference, &copy.id)?,
            None => false,
        };
        if has_secret {
            copy.secret_ref = Some(copy.id.clone());
        }

        copy.validate().map_err(BridgeError::validation)?;

        let saved = copy.clone();
        state.profiles().update(|file| {
            file.profiles.push(saved);
            Ok(())
        })?;

        tracing::info!(
            target: "eclyzer_bridge",
            source_profile = %source.id,
            "profile duplicated"
        );

        Ok(summarise(&state, copy))
    })
    .await
}

/// A name for the copy that does not collide with an existing profile.
///
/// Starts from "<name> copy" and appends a number only when needed, so the
/// common case stays readable.
fn unique_copy_name(source_name: &str, state: &AppState) -> String {
    let taken: Vec<String> = state
        .profiles()
        .load()
        .map(|file| {
            file.profiles
                .into_iter()
                .map(|profile| profile.name)
                .collect()
        })
        .unwrap_or_default();

    let base = format!("{source_name} copy");
    if !taken.iter().any(|name| name == &base) {
        return base;
    }
    for n in 2..1000 {
        let candidate = format!("{base} {n}");
        if !taken.iter().any(|name| name == &candidate) {
            return candidate;
        }
    }
    base
}

/// Delete a profile and its stored key.
///
/// The caller must have resolved any active use first; this command refuses to
/// delete a profile that an agent is still configured to use, because doing so
/// would leave the agent pointing at a provider whose credential no longer
/// exists (PRD §22.5).
#[tauri::command]
pub async fn delete_profile(
    state: tauri::State<'_, AppState>,
    id: String,
    force: bool,
) -> CommandResult<()> {
    let state = state.inner().clone();
    let id = require_id(&id, "Profile id")?;
    blocking(move || {
        let profile = state
            .profiles()
            .load()?
            .profiles
            .into_iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| BridgeError::validation("That profile no longer exists."))?;

        if !force {
            for adapter in state.adapters() {
                if let Ok(active) = adapter.get_active_profile() {
                    if active.profile_id.as_deref() == Some(profile.id.as_str()) {
                        return Err(BridgeError::validation(format!(
                            "{} is currently using this profile.",
                            adapter.id().display_name()
                        ))
                        .with_hint(
                            "Restore the default configuration, or activate another profile, before deleting this one.",
                        ));
                    }
                }
            }
        }

        // Remove the secret first: a profile row without a key is a harmless
        // state, whereas a key without a profile row would be an orphan.
        if let Some(reference) = profile.secret_ref.as_deref() {
            state.secrets().delete(reference)?;
        }

        state.profiles().update(|file| {
            file.profiles.retain(|item| item.id != profile.id);
            Ok(())
        })?;
        Ok(())
    })
    .await
}

/// Test one profile against one agent (PRD §20).
///
/// This makes real, potentially billable requests to the provider.
#[tauri::command]
pub async fn test_profile_connection(
    state: tauri::State<'_, AppState>,
    id: String,
    agent: String,
) -> CommandResult<TestOutcome> {
    let state = state.inner().clone();
    let id = require_id(&id, "Profile id")?;
    let agent = parse_agent(&agent)?;

    let profile = {
        let state = state.clone();
        let id = id.clone();
        blocking(move || {
            state
                .profiles()
                .load()?
                .profiles
                .into_iter()
                .find(|profile| profile.id == id)
                .ok_or_else(|| BridgeError::validation("That profile no longer exists."))
        })
        .await?
    };

    // The secret is read for the duration of this call only.
    let secret = crate::testing::pipeline::resolve_secret(state.secrets(), &profile)?;

    let outcome = test_profile(&profile, agent, secret).await?;

    // Persist the verdict so the list and the activation gate see it.
    let stale_key = profile.compatibility_stale_key(agent, None);
    let record = outcome.to_record(&stale_key);

    let state_for_save = state.clone();
    let profile_id = profile.id.clone();
    blocking(move || {
        state_for_save.profiles().update(|file| {
            if let Some(slot) = file.profiles.iter_mut().find(|p| p.id == profile_id) {
                slot.compatibility.insert(agent, record);
                slot.updated_at = chrono::Local::now().to_rfc3339();
            }
            Ok(())
        })?;
        Ok(())
    })
    .await?;

    Ok(outcome)
}

/// Clear a cached compatibility verdict, forcing a re-test (PRD §45).
#[tauri::command]
pub async fn clear_compatibility(
    state: tauri::State<'_, AppState>,
    id: String,
    agent: String,
) -> CommandResult<ProfileSummary> {
    let state = state.inner().clone();
    let id = require_id(&id, "Profile id")?;
    let agent = parse_agent(&agent)?;
    blocking(move || {
        let saved = state.profiles().update(|file| {
            if let Some(slot) = file.profiles.iter_mut().find(|p| p.id == id) {
                slot.compatibility
                    .insert(agent, crate::profiles::CompatibilityRecord::default());
            }
            Ok(file.clone())
        })?;
        let profile = saved
            .profiles
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| BridgeError::validation("That profile no longer exists."))?;
        Ok(summarise(&state, profile))
    })
    .await
}

/// The cost notice text and whether it has been acknowledged (PRD §20.9).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostNotice {
    pub message: String,
    pub acknowledged: bool,
}

/// Read the cost notice state.
#[tauri::command]
pub async fn get_cost_notice(state: tauri::State<'_, AppState>) -> CommandResult<CostNotice> {
    Ok(CostNotice {
        message: crate::testing::pipeline::COST_NOTICE.to_string(),
        acknowledged: state.settings().cost_notice_seen,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ProfileInput {
        ProfileInput {
            id: None,
            name: "DeepSeek".to_string(),
            provider_name: "DeepSeek".to_string(),
            base_url: "https://api.deepseek.com/v1/".to_string(),
            model: "deepseek-chat".to_string(),
            api_key: None,
            clear_api_key: false,
            targets: vec!["codex".to_string(), "claude".to_string()],
            advanced: None,
            notes: None,
        }
    }

    #[test]
    fn a_valid_input_becomes_a_profile() {
        let profile = input().into_profile(None).unwrap();
        assert_eq!(profile.name, "DeepSeek");
        // The trailing slash is normalised away.
        assert_eq!(profile.base_url, "https://api.deepseek.com/v1");
        assert_eq!(profile.targets, vec![AgentId::Codex, AgentId::Claude]);
        assert!(profile.secret_ref.is_none(), "a key is never set here");
    }

    /// A profile with a passing verdict recorded against its current inputs.
    fn tested_profile() -> Profile {
        let mut profile = input().into_profile(None).unwrap();
        // Compute both keys against the untouched profile first: inserting a
        // record does not affect the key, but computing them up front keeps the
        // fixture obviously independent of insertion order.
        let codex_key = profile.compatibility_stale_key(AgentId::Codex, None);
        let claude_key = profile.compatibility_stale_key(AgentId::Claude, None);
        for (agent, key) in [(AgentId::Codex, codex_key), (AgentId::Claude, claude_key)] {
            profile.compatibility.insert(
                agent,
                crate::profiles::CompatibilityRecord {
                    status: crate::profiles::CompatibilityStatus::Compatible,
                    stale_key: Some(key),
                    ..Default::default()
                },
            );
        }
        profile
    }

    #[test]
    fn a_current_verdict_is_kept() {
        let mut profile = tested_profile();
        assert!(profile.is_ready());
        drop_stale_verdicts(&mut profile);
        assert_eq!(
            profile.compatibility.len(),
            2,
            "verdicts recorded against the current inputs must survive"
        );
        assert!(profile.is_ready(), "and activation must still be allowed");
    }

    #[test]
    fn a_verdict_for_an_edited_endpoint_is_dropped() {
        let mut profile = tested_profile();
        // The user repoints the profile at a different gateway.
        profile.base_url = "https://somewhere-else.example.com/v1".into();
        drop_stale_verdicts(&mut profile);
        assert!(
            profile.compatibility.is_empty(),
            "a verdict measured against a different endpoint must not be shown"
        );
        assert!(
            !profile.is_ready(),
            "a stale verdict must not permit activation"
        );
    }

    #[test]
    fn a_verdict_for_an_edited_model_is_dropped() {
        let mut profile = tested_profile();
        profile.model = "a-different-model".into();
        drop_stale_verdicts(&mut profile);
        assert!(profile.compatibility.is_empty());
        assert!(!profile.is_ready());
    }

    #[test]
    fn a_verdict_for_a_replaced_credential_is_dropped() {
        let mut profile = tested_profile();
        profile.secret_ref = Some("a-newly-stored-key".into());
        drop_stale_verdicts(&mut profile);
        assert!(
            profile.compatibility.is_empty(),
            "a verdict measured with a different key must not be shown"
        );
    }

    #[test]
    fn dropping_stale_verdicts_is_per_agent() {
        // Codex was re-tested after the edit; Claude was not. Only Claude's
        // verdict may be dropped.
        let mut profile = tested_profile();
        profile.model = "a-different-model".into();
        let refreshed = profile.compatibility_stale_key(AgentId::Codex, None);
        profile.compatibility.insert(
            AgentId::Codex,
            crate::profiles::CompatibilityRecord {
                status: crate::profiles::CompatibilityStatus::Compatible,
                stale_key: Some(refreshed),
                ..Default::default()
            },
        );

        drop_stale_verdicts(&mut profile);
        assert!(
            profile.compatibility.contains_key(&AgentId::Codex),
            "a fresh Codex verdict must survive"
        );
        assert!(
            !profile.compatibility.contains_key(&AgentId::Claude),
            "the stale Claude verdict must be dropped"
        );
    }

    #[test]
    fn dropping_stale_verdicts_leaves_a_profile_that_never_tested_alone() {
        let mut profile = input().into_profile(None).unwrap();
        drop_stale_verdicts(&mut profile);
        assert!(profile.compatibility.is_empty());
        assert!(!profile.is_ready());
    }

    #[test]
    fn a_duplicate_gets_a_new_id_and_name_and_no_verdicts() {
        // The three properties that make a copy safe. Anything else is the
        // source's own state and must not travel with it.
        let source = tested_profile();
        assert!(
            !source.compatibility.is_empty(),
            "fixture must have verdicts"
        );

        // Mirrors what `duplicate_profile` does.
        let mut copy = source.clone();
        copy.id = uuid::Uuid::new_v4().to_string();
        copy.compatibility = Default::default();

        assert_ne!(copy.id, source.id, "the copy must be a distinct profile");
        assert!(
            copy.compatibility.is_empty(),
            "a verdict describes a test of the source, not the copy"
        );
        assert!(
            !copy.is_ready(),
            "an untested copy must not claim to be ready to activate"
        );
        // Everything else is carried over.
        assert_eq!(copy.base_url, source.base_url);
        assert_eq!(copy.model, source.model);
        assert_eq!(copy.targets, source.targets);
        assert_eq!(copy.provider_name, source.provider_name);
    }

    #[test]
    fn a_duplicate_of_a_never_tested_profile_is_not_ready() {
        let source = input().into_profile(None).unwrap();
        let mut copy = source.clone();
        copy.id = uuid::Uuid::new_v4().to_string();
        assert!(!copy.is_ready());
    }

    #[test]
    fn duplicate_targets_are_collapsed() {
        let mut raw = input();
        raw.targets = vec!["codex".into(), "CODEX".into(), "codex".into()];
        let profile = raw.into_profile(None).unwrap();
        assert_eq!(profile.targets, vec![AgentId::Codex]);
    }

    #[test]
    fn an_unknown_target_is_rejected() {
        let mut raw = input();
        raw.targets = vec!["codex".into(), "not-an-agent".into()];
        assert!(raw.into_profile(None).is_err());
    }

    #[test]
    fn an_empty_target_list_is_rejected() {
        let mut raw = input();
        raw.targets = Vec::new();
        let err = raw.into_profile(None).unwrap_err();
        assert!(
            err.message.contains("at least one agent"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_missing_name_is_rejected() {
        let mut raw = input();
        raw.name = "   ".to_string();
        assert!(raw.into_profile(None).is_err());
    }

    #[test]
    fn a_name_with_control_characters_is_rejected() {
        let mut raw = input();
        raw.name = "bad\u{7}name".to_string();
        let err = raw.into_profile(None).unwrap_err();
        assert!(err.message.contains("not allowed"), "{}", err.message);
    }

    #[test]
    fn an_invalid_url_is_rejected() {
        let mut raw = input();
        raw.base_url = "not a url".to_string();
        assert!(raw.into_profile(None).is_err());
    }

    #[test]
    fn a_missing_model_is_rejected() {
        let mut raw = input();
        raw.model = "  ".to_string();
        assert!(raw.into_profile(None).is_err());
    }

    #[test]
    fn updating_preserves_the_id_and_creation_time() {
        let original = input().into_profile(None).unwrap();
        let mut raw = input();
        raw.name = "Renamed".to_string();
        let updated = raw.into_profile(Some(&original)).unwrap();

        assert_eq!(updated.id, original.id);
        assert_eq!(updated.created_at, original.created_at);
        assert_eq!(updated.name, "Renamed");
    }

    #[test]
    fn updating_never_invents_a_secret_ref() {
        // Only `save_profile` may attach a key, and only from an explicit field.
        let original = input().into_profile(None).unwrap();
        let updated = input().into_profile(Some(&original)).unwrap();
        assert!(updated.secret_ref.is_none());
    }

    #[test]
    fn blank_notes_are_dropped_rather_than_stored() {
        let mut raw = input();
        raw.notes = Some("   ".to_string());
        let profile = raw.into_profile(None).unwrap();
        assert!(profile.notes.is_none());
    }
}
