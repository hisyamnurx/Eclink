//! Recovery commands: Restore Default, Restore Original, Snapshots, and Undo
//! (PRD §25–§30).
//!
//! Every one of these is reversible: each restore snapshots the state it is
//! about to replace, so a user who restores the wrong thing can undo it.

use serde::Serialize;

use crate::adapters::ApplyOptions;
use crate::app::AppState;
use crate::backup::HistoryEntry;
use crate::commands::{blocking, parse_agent, require_id, CommandResult};
use crate::error::BridgeError;

/// A snapshot as the History screen shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotView {
    pub id: String,
    pub agent: String,
    /// Human label, e.g. "Activated DeepSeek".
    pub label: String,
    pub reason: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub bytes: u64,
}

/// Restore an agent's own default provider (PRD §17, §30).
#[tauri::command]
pub async fn restore_default(
    state: tauri::State<'_, AppState>,
    agent: String,
) -> CommandResult<crate::adapters::ApplyOutcome> {
    let state = state.inner().clone();
    let agent = parse_agent(&agent)?;
    blocking(move || {
        let adapter = state.adapter(agent);
        let options = ApplyOptions {
            secret: None,
            helper_path: state.helper_path(),
            // No hash guard: this is an explicit, confirmed user action.
            expected_sha256: None,
            agent_version: None,
        };
        adapter.restore_default(&options)
    })
    .await
}

/// Report what a stock reset would remove, without changing anything.
///
/// Callers must show this before confirming, because the operation is
/// deliberately willing to delete provider and model settings the user created
/// by hand.
#[tauri::command]
pub async fn preview_stock_reset(
    agent: String,
) -> CommandResult<Option<crate::adapters::claude::StockResetPreview>> {
    let agent = parse_agent(&agent)?;
    blocking(move || {
        // Only Claude Code has a settings file with these keys; for Codex the
        // reset targets `model` and `model_provider` in TOML, which has no
        // equivalent preview yet, so it reports nothing rather than a wrong list.
        Ok(match agent {
            crate::profiles::AgentId::Claude => {
                Some(crate::adapters::claude::stock_reset_preview())
            }
            crate::profiles::AgentId::Codex => None,
        })
    })
    .await
}

/// Remove every provider and model override, returning the agent to stock.
///
/// Stronger than `restore_default`: it also removes overrides the user created
/// themselves, which is the only way to get back to the built-in behaviour. The
/// caller is responsible for confirming first, having shown the preview.
#[tauri::command]
pub async fn reset_to_stock_defaults(
    state: tauri::State<'_, AppState>,
    agent: String,
) -> CommandResult<crate::adapters::ApplyOutcome> {
    let state = state.inner().clone();
    let agent = parse_agent(&agent)?;
    blocking(move || {
        let adapter = state.adapter(agent);
        let options = ApplyOptions {
            secret: None,
            helper_path: state.helper_path(),
            // No hash guard: this is an explicit, previewed, confirmed action.
            expected_sha256: None,
            agent_version: None,
        };
        adapter.reset_to_stock_defaults(&options)
    })
    .await
}

/// Restore the immutable pre-Bridge original (PRD §29).
#[tauri::command]
pub async fn restore_original(
    state: tauri::State<'_, AppState>,
    agent: String,
) -> CommandResult<crate::adapters::ApplyOutcome> {
    let state = state.inner().clone();
    let agent = parse_agent(&agent)?;
    blocking(move || {
        let adapter = state.adapter(agent);
        let options = ApplyOptions {
            secret: None,
            helper_path: state.helper_path(),
            expected_sha256: None,
            agent_version: None,
        };
        adapter.restore_original(&options)
    })
    .await
}

/// List snapshots for one agent, newest first (PRD §26).
#[tauri::command]
pub async fn list_snapshots(
    state: tauri::State<'_, AppState>,
    agent: Option<String>,
) -> CommandResult<Vec<SnapshotView>> {
    let state = state.inner().clone();
    let agent = agent.as_deref().map(parse_agent).transpose()?;
    blocking(move || {
        let backups = state.backups();
        let agents: Vec<String> = match agent {
            Some(agent) => vec![agent.as_str().to_string()],
            None => crate::adapters::AgentId::all()
                .iter()
                .map(|a| a.as_str().to_string())
                .collect(),
        };

        let mut out = Vec::new();
        for agent in agents {
            // `list` returns metadata only; snapshot payloads are read lazily
            // when one is actually restored, so listing stays cheap.
            for meta in backups.snapshots().list(&agent)? {
                out.push(SnapshotView {
                    label: label_for_reason(&meta.reason),
                    id: meta.id,
                    agent: meta.agent,
                    reason: meta.reason,
                    created_at: meta.created_at,
                    profile_id: meta.profile_id,
                    bytes: meta.bytes,
                });
            }
        }
        // Newest first across agents.
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(out)
    })
    .await
}

fn label_for_reason(reason: &str) -> String {
    match reason {
        "activate_profile" => "Activated a profile".to_string(),
        "restore_default" => "Restored default configuration".to_string(),
        "restore_original" => "Restored original configuration".to_string(),
        "restore_snapshot" => "Restored a snapshot".to_string(),
        "undo" => "Undid a change".to_string(),
        other => other.replace('_', " "),
    }
}

/// Restore one snapshot (PRD §26).
#[tauri::command]
pub async fn restore_snapshot(
    state: tauri::State<'_, AppState>,
    agent: String,
    snapshot_id: String,
) -> CommandResult<crate::adapters::ApplyOutcome> {
    let state = state.inner().clone();
    let agent = parse_agent(&agent)?;
    let snapshot_id = require_id(&snapshot_id, "Snapshot id")?;

    blocking(move || {
        let adapter = state.adapter(agent);
        let snapshot = state
            .backups()
            .snapshots()
            .get(agent.as_str(), &snapshot_id)?
            .ok_or_else(|| {
                BridgeError::validation("That snapshot is no longer available.")
                    .with_hint("Snapshots are pruned once 20 exist for an agent.")
            })?;

        let options = ApplyOptions {
            secret: None,
            helper_path: state.helper_path(),
            expected_sha256: None,
            agent_version: None,
        };
        adapter.restore_snapshot(&snapshot, &options)
    })
    .await
}

/// Undo the last successful change for an agent (PRD §28).
#[tauri::command]
pub async fn undo_last_change(
    state: tauri::State<'_, AppState>,
    agent: String,
) -> CommandResult<crate::adapters::ApplyOutcome> {
    let state = state.inner().clone();
    let agent = parse_agent(&agent)?;

    blocking(move || {
        let adapter = state.adapter(agent);
        let backups = state.backups();

        // Failed attempts are recorded but are not undo targets; only a
        // successful change has a state worth returning to.
        let entry = backups
            .history()
            .latest_successful(agent.as_str())?
            .ok_or_else(|| {
                BridgeError::validation("There is nothing to undo for this agent.")
                    .with_hint("Eclyzer Bridge has not changed this agent's configuration yet.")
            })?;

        let snapshot_id = entry.snapshot_id.ok_or_else(|| {
            BridgeError::internal("That change has no snapshot to return to.")
                .with_hint("Use Restore Original or Restore Default instead.")
        })?;

        let snapshot = backups
            .snapshots()
            .get(agent.as_str(), &snapshot_id)?
            .ok_or_else(|| {
                BridgeError::validation("The snapshot for that change is no longer available.")
                    .with_hint("Snapshots are pruned once 20 exist for an agent.")
            })?;

        let options = ApplyOptions {
            secret: None,
            helper_path: state.helper_path(),
            expected_sha256: None,
            agent_version: None,
        };
        adapter.restore_snapshot(&snapshot, &options)
    })
    .await
}

/// The change ledger for the History screen (PRD §27).
#[tauri::command]
pub async fn list_history(
    state: tauri::State<'_, AppState>,
    agent: Option<String>,
    limit: Option<usize>,
) -> CommandResult<Vec<HistoryEntry>> {
    let state = state.inner().clone();
    let agent = agent.as_deref().map(parse_agent).transpose()?;
    let limit = limit.unwrap_or(50).min(crate::backup::HISTORY_LIMIT);

    blocking(move || {
        let mut entries = state.backups().history().list(agent.map(|a| a.as_str()))?;
        entries.truncate(limit);
        Ok(entries)
    })
    .await
}

/// Whether recovery is possible for an agent, and what is available.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryStatus {
    pub agent: String,
    /// Whether an immutable original backup exists.
    pub has_original: bool,
    pub snapshot_count: usize,
    /// Whether anything Bridge-owned is currently applied.
    pub has_managed_configuration: bool,
    /// Whether an undo target exists.
    pub can_undo: bool,
    /// What the last change was, for the confirmation copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_change_label: Option<String>,
    /// Whether the current configuration is unmanaged but customized.
    pub external_configuration: bool,
}

/// Report what recovery options exist before offering them (PRD §49).
#[tauri::command]
pub async fn recovery_status(
    state: tauri::State<'_, AppState>,
    agent: String,
) -> CommandResult<RecoveryStatus> {
    let state = state.inner().clone();
    let agent = parse_agent(&agent)?;
    blocking(move || {
        let backups = state.backups();
        let adapter = state.adapter(agent);

        let active = adapter.get_active_profile().ok();
        let has_managed_configuration = active
            .as_ref()
            .map(|a| a.kind == crate::adapters::ActiveConfigurationKind::Managed)
            .unwrap_or(false);
        let external_configuration = active
            .as_ref()
            .map(|a| a.kind == crate::adapters::ActiveConfigurationKind::External)
            .unwrap_or(false);

        let last_change = backups
            .history()
            .latest_successful(agent.as_str())
            .ok()
            .flatten();
        let can_undo = last_change
            .as_ref()
            .map(|entry| entry.snapshot_id.is_some())
            .unwrap_or(false);

        Ok(RecoveryStatus {
            agent: agent.as_str().to_string(),
            has_original: backups.has_original(agent.as_str()),
            snapshot_count: backups.snapshot_count(agent.as_str()),
            has_managed_configuration,
            can_undo,
            last_change_label: last_change.map(|entry| entry.label),
            external_configuration,
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_labels_are_human_readable() {
        assert_eq!(label_for_reason("activate_profile"), "Activated a profile");
        assert_eq!(
            label_for_reason("restore_default"),
            "Restored default configuration"
        );
    }

    #[test]
    fn an_unknown_reason_is_still_readable() {
        // A future reason code must not render as a raw identifier.
        assert_eq!(label_for_reason("some_new_reason"), "some new reason");
    }
}
