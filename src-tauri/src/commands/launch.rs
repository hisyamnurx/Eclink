//! Activation and launch commands.

use serde::Serialize;

use crate::adapters::{ApplyOptions, ApplyOutcome, LaunchPlan};
use crate::app::AppState;
use crate::commands::{blocking, parse_agent, require_id, CommandResult};
use crate::error::{BridgeError, BridgeResult};

/// Outcome of an activation, with the copy the UI shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationResult {
    #[serde(flatten)]
    pub outcome: ApplyOutcome,
    /// One-line confirmation, e.g. "DeepSeek is now active for Codex."
    pub message: String,
    /// Whether the credential helper was used, or an env fallback.
    pub credential_mode: CredentialMode,
}

/// How the agent will authenticate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMode {
    /// A credential-helper command reads the key on demand. Preferred.
    CredentialHelper,
    /// The key is written into the agent's own settings file.
    SettingsFile,
    /// The key is supplied through an environment variable at launch.
    EnvironmentVariable,
}

/// Activate a profile for an agent (PRD §17.4, §31).
#[tauri::command]
pub async fn activate_profile(
    state: tauri::State<'_, AppState>,
    id: String,
    agent: String,
) -> CommandResult<ActivationResult> {
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

    // Refuse to activate something that has never been verified, so a user
    // cannot silently end up with a broken agent (PRD §21.2).
    let status = profile.compatibility_for(agent);
    if !status.allows_activation() {
        return Err(BridgeError::incompatible(match status {
            crate::profiles::CompatibilityStatus::Untested => {
                "Test this profile against the agent before activating it."
            }
            crate::profiles::CompatibilityStatus::Testing => {
                "A test is already running for this profile."
            }
            crate::profiles::CompatibilityStatus::Incompatible => {
                "This profile is not compatible with this agent, so it cannot be activated."
            }
            crate::profiles::CompatibilityStatus::TestFailed => {
                "The last test could not reach the endpoint, so this profile cannot be activated yet."
            }
            _ => "This profile has not been verified for this agent.",
        })
        .with_hint("Run Test Connection, then activate once it passes.")
        .into());
    }

    let secret = crate::testing::pipeline::resolve_secret(state.secrets(), &profile)?;
    let helper_path = state.helper_path();
    let credential_mode = match agent {
        crate::adapters::AgentId::Codex if helper_path.is_some() => {
            CredentialMode::CredentialHelper
        }
        crate::adapters::AgentId::Codex => CredentialMode::EnvironmentVariable,
        crate::adapters::AgentId::Claude => CredentialMode::SettingsFile,
    };

    // The config hash the UI last saw, so an edit made in another window aborts
    // the write instead of being clobbered (PRD §31.3).
    let state_for_hash = state.clone();
    let expected_sha256 = blocking(move || {
        Ok(state_for_hash
            .adapter(agent)
            .read_config()?
            .sha256)
    })
    .await?;

    let profile_name = profile.name.clone();
    let state_for_apply = state.clone();

    // The secret is moved into the blocking task so it lives exactly as long as
    // the apply needs it, and no longer. `ApplyOptions` borrows it, so the whole
    // construction happens inside the closure.
    let outcome = blocking(move || {
        let options = ApplyOptions {
            secret: Some(secret.expose()),
            helper_path,
            expected_sha256,
            agent_version: None,
        };
        state_for_apply.adapter(agent).apply_profile(&profile, &options)
    })
    .await?;

    Ok(ActivationResult {
        outcome,
        message: format!(
            "{} is now active for {}.",
            profile_name,
            agent.display_name()
        ),
        credential_mode,
    })
}

/// How the agent will be launched.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPreview {
    pub plan: LaunchPlan,
    /// Environment variable names that will be set, never their values.
    pub env_names: Vec<String>,
    /// The exact command line, shown so the user can audit it.
    pub display: String,
}

/// Build the launch plan without starting anything.
#[tauri::command]
pub async fn preview_launch(
    state: tauri::State<'_, AppState>,
    agent: String,
    project_directory: String,
) -> CommandResult<LaunchPreview> {
    let state = state.inner().clone();
    let agent = parse_agent(&agent)?;
    let directory = project_directory.trim().to_string();
    if directory.is_empty() {
        return Err(BridgeError::validation("Choose a project folder first.").into());
    }

    blocking(move || {
        let adapter = state.adapter(agent);
        let plan = adapter.launch_argv(std::path::Path::new(&directory))?;
        let profile = find_active_profile(&state, agent)?;
        let secret = match &profile {
            Some(profile) => {
                crate::testing::pipeline::resolve_secret(state.secrets(), profile).ok()
            }
            None => None,
        };
        let env = adapter.launch_env(secret.as_ref().map(|s| s.expose()))?;
        Ok(LaunchPreview {
            display: plan.display.clone(),
            plan,
            env_names: env.into_iter().map(|(name, _)| name).collect(),
        })
    })
    .await
}

/// Launch an agent in a project folder (PRD §32).
#[tauri::command]
pub async fn launch_agent(
    state: tauri::State<'_, AppState>,
    agent: String,
    project_directory: String,
) -> CommandResult<u32> {
    let state = state.inner().clone();
    let agent = parse_agent(&agent)?;
    let directory = project_directory.trim().to_string();
    if directory.is_empty() {
        return Err(BridgeError::validation("Choose a project folder first.").into());
    }

    // Remember the folder only when the user asked us to (PRD §54).
    if state.settings().remember_project_directory {
        let _ = state.update_settings(|settings| {
            settings.last_project_directory = Some(directory.clone());
        });
    }

    blocking(move || {
        let adapter = state.adapter(agent);
        let plan = adapter.launch_argv(std::path::Path::new(&directory))?;

        let profile = find_active_profile(&state, agent)?;
        let secret = match &profile {
            Some(profile) => {
                crate::testing::pipeline::resolve_secret(state.secrets(), profile).ok()
            }
            None => None,
        };
        // The overlay is computed while `secret` is alive, then applied to the
        // child process only; the user's global environment is never modified
        // (PRD §32.3).
        let env = adapter.launch_env(secret.as_ref().map(|value| value.expose()))?;
        let pid = crate::process::spawn_launch(&plan, &env)?;
        tracing::info!(
            target: "eclyzer_bridge",
            agent = agent.as_str(),
            pid,
            "launched agent"
        );
        Ok(pid)
    })
    .await
}

/// The profile an agent is currently configured to use, when Bridge owns it.
fn find_active_profile(
    state: &AppState,
    agent: crate::adapters::AgentId,
) -> BridgeResult<Option<crate::profiles::Profile>> {
    let active = state.adapter(agent).get_active_profile()?;
    let Some(profile_id) = active.profile_id else {
        return Ok(None);
    };
    Ok(state
        .profiles()
        .load()?
        .profiles
        .into_iter()
        .find(|profile| profile.id == profile_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_modes_are_distinguishable() {
        assert_ne!(
            CredentialMode::CredentialHelper,
            CredentialMode::EnvironmentVariable
        );
    }
}
