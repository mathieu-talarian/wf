//! Per-user AI-assist toggles, stored as jsonb on `users.ai_settings`.
//! Everything defaults to **off**.

use sea_orm::prelude::Uuid;
use serde::{Deserialize, Serialize};
use wf_db::tables::users;

use crate::error::AppError;
use crate::state::AppState;

#[derive(Serialize, Deserialize, Clone, Copy, Default, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct AiSettings {
    /// Morning brief in the Needs-you inbox (+ inbox ranking, orphan
    /// suggestions as they land).
    pub morning_brief: bool,
    /// `✦ draft reply` on QA threads.
    pub qa_draft_replies: bool,
    /// Auto-draft the QA handoff message on move to Testing.
    pub qa_handoff_drafts: bool,
}

pub async fn get(state: &AppState, user_id: Uuid) -> Result<AiSettings, AppError> {
    let stored = users::get_ai_settings(&state.db, user_id).await?;
    Ok(stored.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())
}

pub async fn set(state: &AppState, user_id: Uuid, settings: AiSettings) -> Result<AiSettings, AppError> {
    let json = serde_json::to_value(settings).map_err(|e| AppError::internal(anyhow::anyhow!(e)))?;
    users::set_ai_settings(&state.db, user_id, json).await?;
    Ok(settings)
}

/// Guard for AI endpoints: 409 (`ai-disabled`) when the toggle is off.
pub fn require_enabled(enabled: bool) -> Result<(), AppError> {
    if enabled {
        Ok(())
    } else {
        Err(AppError::ai_disabled())
    }
}
