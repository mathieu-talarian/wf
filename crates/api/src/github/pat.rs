//! GitHub PAT connection orchestration (port of `pat/runners.ts` connect /
//! validate + the service wiring). Free functions over `&AppState`.

use sea_orm::prelude::Uuid;
use wf_core::Sealed;
use wf_db::tables::github_pat_connections::{self as gh, UpsertPatInput};
use wf_github::types::PatValidationResult;
use wf_github::{validate_token, GithubError};

use crate::error::AppError;
use crate::github::summary::{self, GithubConnectionSummary};
use crate::github::token_cache::CachedPat;
use crate::state::AppState;

pub async fn status(
    state: &AppState,
    user_id: Uuid,
) -> Result<GithubConnectionSummary, AppError> {
    let row = gh::select_row(&state.db, user_id).await?;
    Ok(summary::from_row(row))
}

/// Seals + upserts a freshly validated token, then busts the caches.
async fn store_validated(
    state: &AppState,
    user_id: Uuid,
    token: &str,
    result: PatValidationResult,
) -> Result<(), AppError> {
    let sealed: Sealed = state
        .cipher
        .seal(token)
        .map_err(|e| AppError::internal(anyhow::anyhow!(e)))?;

    gh::upsert_pat(
        &state.db,
        UpsertPatInput {
            user_id,
            github_user_id: result.github_user_id,
            github_login: result.login,
            token_kind: result.token_kind.as_str().to_string(),
            sealed,
            scopes: result.scopes,
            expires_at: result.expires_at.map(Into::into),
            last_four: last_four(token),
            validation_status: "valid".to_string(),
        },
    )
    .await?;
    state.token_cache.clear(user_id);
    Ok(())
}

pub async fn connect(
    state: &AppState,
    user_id: Uuid,
    token: &str,
) -> Result<GithubConnectionSummary, AppError> {
    let result = validate_token(token).await?;
    store_validated(state, user_id, token, result).await?;
    status(state, user_id).await
}

/// Re-validates the stored token. On a validation failure, persists the status
/// before surfacing it (port of `markAndFail`).
pub async fn validate(
    state: &AppState,
    user_id: Uuid,
) -> Result<GithubConnectionSummary, AppError> {
    let row = gh::select_row(&state.db, user_id)
        .await?
        .ok_or(AppError::from(GithubError::NotConnected))?;

    let token = state
        .cipher
        .open(&Sealed {
            ciphertext: row.access_token_ciphertext.clone(),
            iv: row.access_token_iv.clone(),
            auth_tag: row.access_token_auth_tag.clone(),
        })
        .map_err(|e| AppError::internal(anyhow::anyhow!(e)))?;

    match validate_token(&token).await {
        Ok(result) => {
            store_validated(state, user_id, &token, result).await?;
        }
        Err(e) => {
            let _ = gh::mark_validation(&state.db, user_id, e.status.as_str(), &e.message)
                .await;
            return Err(e.into());
        }
    }
    status(state, user_id).await
}

pub async fn disconnect(state: &AppState, user_id: Uuid) -> Result<(), AppError> {
    gh::disconnect(&state.db, user_id).await?;
    state.token_cache.clear(user_id);
    Ok(())
}

/// Cache-first PAT resolution for the data/activity paths.
pub async fn resolve_pat(
    state: &AppState,
    user_id: Uuid,
) -> Result<Option<CachedPat>, AppError> {
    if let Some(cached) = state.token_cache.get(user_id) {
        return Ok(Some(cached));
    }
    let Some(row) = gh::select_row(&state.db, user_id).await? else {
        return Ok(None);
    };
    let token = state
        .cipher
        .open(&Sealed {
            ciphertext: row.access_token_ciphertext,
            iv: row.access_token_iv,
            auth_tag: row.access_token_auth_tag,
        })
        .map_err(|_| AppError::from(GithubError::Api("token decryption failed".into())))?;
    let value = CachedPat {
        token,
        login: row.github_login,
        selected_repos: summary::json_string_array(&row.selected_repos),
    };
    state.token_cache.set(user_id, value.clone());
    Ok(Some(value))
}

fn last_four(token: &str) -> String {
    let start = token.len().saturating_sub(4);
    token[start..].to_string()
}
