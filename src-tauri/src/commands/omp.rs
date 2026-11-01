//! Oh My Pi profile selection and dry-run preview commands.

use crate::adapters::{OmpPreview, OmpProfileSummary};
use crate::app::AppState;
use crate::commands::{blocking, require_id, CommandResult};
use crate::error::BridgeError;
use crate::profiles::Profile;

/// List the OMP default profile and every named profile directory.
#[tauri::command]
pub async fn list_omp_profiles(
    state: tauri::State<'_, AppState>,
) -> CommandResult<Vec<OmpProfileSummary>> {
    let state = state.inner().clone();
    blocking(move || state.omp().list_profiles()).await
}

/// The OMP profile Eclink currently manages.
#[tauri::command]
pub async fn get_omp_profile(state: tauri::State<'_, AppState>) -> CommandResult<String> {
    Ok(state.omp().selected_profile())
}

/// Switch the OMP profile Eclink manages.
///
/// This changes only Eclink's target. It never writes either YAML file.
#[tauri::command]
pub async fn set_omp_profile(
    state: tauri::State<'_, AppState>,
    profile: String,
) -> CommandResult<String> {
    let state = state.inner().clone();
    let profile = profile.trim().to_string();
    blocking(move || state.set_omp_profile(&profile)).await
}

/// Preview exactly what activation would write, without modifying disk.
#[tauri::command]
pub async fn preview_omp_profile(
    state: tauri::State<'_, AppState>,
    profile: Profile,
) -> CommandResult<OmpPreview> {
    let state = state.inner().clone();
    blocking(move || state.omp().preview(&profile)).await
}

/// Preview a saved profile by ID.
#[tauri::command]
pub async fn preview_saved_omp_profile(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CommandResult<OmpPreview> {
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
        state.omp().preview(&profile)
    })
    .await
}
