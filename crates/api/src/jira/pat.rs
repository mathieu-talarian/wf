//! Jira connection orchestration (port of `jira/pat/runners.ts` connect /
//! validate / disconnect / projects). Free functions over `&AppState`. Per the
//! spec (§19.5) the Jira token is decrypted per request — no cache.

use sea_orm::prelude::Uuid;
use wf_core::Sealed;
use wf_db::tables::jira_pat_connections::{self as jira, UpsertJiraInput};
use wf_jira::{validate_credentials, JiraConnectInput, JiraNotConnected, JiraValidated};

use crate::error::AppError;
use crate::jira::summary::{self, JiraConnectionSummary};
use crate::state::AppState;

pub async fn status(state: &AppState, user_id: Uuid) -> Result<JiraConnectionSummary, AppError> {
    let row = jira::select_row(&state.db, user_id).await?;
    Ok(summary::from_row(row))
}

fn open_token(state: &AppState, row: &jira::Model) -> Result<String, AppError> {
    state
        .cipher
        .open(&Sealed {
            ciphertext: row.api_token_ciphertext.clone(),
            iv: row.api_token_iv.clone(),
            auth_tag: row.api_token_auth_tag.clone(),
        })
        .map_err(|e| AppError::internal(anyhow::anyhow!(e)))
}

/// Seals + upserts a freshly validated credential set.
async fn store_validated(
    state: &AppState,
    user_id: Uuid,
    token: &str,
    email: String,
    validated: JiraValidated,
) -> Result<(), AppError> {
    let sealed: Sealed = state.cipher.seal(token).map_err(|e| AppError::internal(anyhow::anyhow!(e)))?;
    jira::upsert_jira(
        &state.db,
        UpsertJiraInput {
            user_id,
            site_url: validated.origin,
            account_id: validated.account_id,
            email,
            display_name: validated.display_name,
            sealed,
            last_four: last_four(token),
        },
    )
    .await?;
    Ok(())
}

pub async fn connect(
    state: &AppState,
    user_id: Uuid,
    input: JiraConnectInput,
) -> Result<JiraConnectionSummary, AppError> {
    let validated = validate_credentials(&input).await?;
    store_validated(state, user_id, &input.token, input.email.clone(), validated).await?;
    status(state, user_id).await
}

/// Re-validates the stored credentials. On a validation failure, persists the
/// status before surfacing it (port of `markAndFail`).
pub async fn validate(state: &AppState, user_id: Uuid) -> Result<JiraConnectionSummary, AppError> {
    let row = jira::select_row(&state.db, user_id)
        .await?
        .ok_or_else(|| AppError::from(JiraNotConnected("No Jira connection".to_string())))?;
    let token = open_token(state, &row)?;
    let input = JiraConnectInput { site_url: row.site_url.clone(), email: row.email.clone(), token };

    match validate_credentials(&input).await {
        Ok(validated) => {
            store_validated(state, user_id, &input.token, input.email.clone(), validated).await?;
        }
        Err(e) => {
            let _ = jira::mark_validation(&state.db, user_id, e.status.as_str(), &e.message).await;
            return Err(e.into());
        }
    }
    status(state, user_id).await
}

pub async fn disconnect(state: &AppState, user_id: Uuid) -> Result<(), AppError> {
    jira::disconnect(&state.db, user_id).await?;
    Ok(())
}

pub async fn set_projects(
    state: &AppState,
    user_id: Uuid,
    projects: &[String],
) -> Result<JiraConnectionSummary, AppError> {
    jira::set_selected_projects(&state.db, user_id, projects).await?;
    status(state, user_id).await
}

fn last_four(token: &str) -> String {
    let start = token.len().saturating_sub(4);
    token[start..].to_string()
}
