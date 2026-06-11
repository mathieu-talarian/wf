//! Slack connection orchestration: connect / validate / disconnect / watched
//! channels. The bot token is sealed with the shared `TokenCipher` and, like
//! Jira, decrypted per request — no cache.

use sea_orm::prelude::Uuid;
use wf_core::Sealed;
use wf_db::tables::slack_connections::{self as slack, UpsertSlackConnectionInput};
use wf_slack::{SlackClient, SlackTokenState};

use crate::error::AppError;
use crate::slack::summary::{self, SlackChannelOption, SlackChannelRef, SlackConnectionSummary};
use crate::state::AppState;

pub async fn status(state: &AppState, user_id: Uuid) -> Result<SlackConnectionSummary, AppError> {
    let row = slack::select_row(&state.db, user_id).await?;
    Ok(summary::from_row(row))
}

pub(crate) fn open_token(state: &AppState, row: &slack::Model) -> Result<String, AppError> {
    state
        .cipher
        .open(&Sealed {
            ciphertext: row.bot_token_ciphertext.clone(),
            iv: row.bot_token_iv.clone(),
            auth_tag: row.bot_token_auth_tag.clone(),
        })
        .map_err(|e| AppError::internal(anyhow::anyhow!(e)))
}

pub(crate) async fn require_row(
    state: &AppState,
    user_id: Uuid,
) -> Result<slack::Model, AppError> {
    slack::select_row(&state.db, user_id)
        .await?
        .ok_or_else(AppError::slack_not_connected)
}

/// Marks the stored token after a Slack error that implicates it, so the
/// Settings pill flips to `expiring` / `invalid` without a manual validate.
pub(crate) async fn note_token_error(
    state: &AppState,
    user_id: Uuid,
    error: &wf_slack::SlackApiError,
) {
    let status = match error.token_state() {
        SlackTokenState::Expiring => "expiring",
        SlackTokenState::Invalid => "invalid",
        SlackTokenState::Unrelated => return,
    };
    let _ = slack::set_validation(&state.db, user_id, status, Some(&error.message)).await;
}

pub async fn connect(
    state: &AppState,
    user_id: Uuid,
    token: &str,
) -> Result<SlackConnectionSummary, AppError> {
    let auth = SlackClient::new(token).auth_test().await.map_err(AppError::from)?;
    let sealed: Sealed =
        state.cipher.seal(token).map_err(|e| AppError::internal(anyhow::anyhow!(e)))?;
    slack::upsert_connection(
        &state.db,
        UpsertSlackConnectionInput {
            user_id,
            bot_token_ciphertext: sealed.ciphertext,
            bot_token_iv: sealed.iv,
            bot_token_auth_tag: sealed.auth_tag,
            team_name: auth.team,
            bot_user_id: auth.user_id,
            validation_status: "valid".to_string(),
        },
    )
    .await?;
    status(state, user_id).await
}

/// Re-validates the stored token; on a token failure the status is persisted
/// before surfacing the error (same shape as the Jira flow).
pub async fn validate(state: &AppState, user_id: Uuid) -> Result<SlackConnectionSummary, AppError> {
    let row = require_row(state, user_id).await?;
    let token = open_token(state, &row)?;
    match SlackClient::new(&token).auth_test().await {
        Ok(_) => slack::set_validation(&state.db, user_id, "valid", None).await?,
        Err(e) => {
            note_token_error(state, user_id, &e).await;
            return Err(e.into());
        }
    }
    status(state, user_id).await
}

pub async fn disconnect(state: &AppState, user_id: Uuid) -> Result<(), AppError> {
    slack::delete_connection(&state.db, user_id).await?;
    Ok(())
}

pub async fn list_channels(
    state: &AppState,
    user_id: Uuid,
) -> Result<Vec<SlackChannelOption>, AppError> {
    let row = require_row(state, user_id).await?;
    let token = open_token(state, &row)?;
    let channels = SlackClient::new(&token).list_channels().await.map_err(|e| {
        let app: AppError = e.into();
        app
    })?;
    Ok(channels
        .into_iter()
        .map(|c| SlackChannelOption {
            id: c.id,
            name: c.name,
            is_member: c.is_member,
            topic: c.topic.map(|t| t.value),
        })
        .collect())
}

/// Sets the watched-channel list; ids are resolved to names via
/// `conversations.list` so the stored refs stay self-describing.
pub async fn set_channels(
    state: &AppState,
    user_id: Uuid,
    channel_ids: &[String],
) -> Result<SlackConnectionSummary, AppError> {
    let row = require_row(state, user_id).await?;
    let token = open_token(state, &row)?;
    let known = SlackClient::new(&token).list_channels().await.map_err(AppError::from)?;
    let refs: Vec<SlackChannelRef> = known
        .into_iter()
        .filter(|c| channel_ids.contains(&c.id))
        .map(|c| SlackChannelRef { id: c.id, name: c.name })
        .collect();
    let json = serde_json::to_value(&refs).map_err(|e| AppError::internal(anyhow::anyhow!(e)))?;
    slack::set_channels(&state.db, user_id, json).await?;
    status(state, user_id).await
}
