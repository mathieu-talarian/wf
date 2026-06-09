//! GitHub dashboard orchestration (port of `pat/dashboard-load.ts` + the data
//! runners). Stale-while-revalidate over the in-memory cache and the durable
//! `dashboard_snapshot`, with single-flight background refresh.

use actix_web::web;
use chrono::SecondsFormat;
use sea_orm::prelude::Uuid;
use serde::Serialize;
use wf_core::Sealed;
use wf_db::tables::github_pat_connections as gh;
use wf_github::{
    enrich_pull_request, enrich_pull_requests, fetch_dashboard, fetch_queue_pulls,
    list_repositories, GithubAccountSummary, GithubDashboard, GithubError, GithubPullEnrichmentResult,
    GithubPullRef, GithubPullRequestEnrichment, GithubQueueKey, GithubRepoOption, RepoRef,
};

use crate::error::AppError;
use crate::github::summary::json_string_array;
use crate::github::token_cache::CachedPat;
use crate::state::AppState;

/// `GET /me/github/repos` response.
#[derive(Serialize, utoipa::ToSchema)]
pub struct RepoSelection {
    pub available: Vec<GithubRepoOption>,
    pub selected: Vec<String>,
}

fn account_summary(row: &gh::Model) -> GithubAccountSummary {
    GithubAccountSummary {
        connected: true,
        login: Some(row.github_login.clone()),
        scope: row.scope.clone(),
        connected_at: Some(
            row.created_at
                .with_timezone(&chrono::Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true),
        ),
    }
}

/// The durable snapshot for `tab`, if it matches (`{ tab, data }`).
fn snapshot_for(row: &gh::Model, tab: GithubQueueKey) -> Option<GithubDashboard> {
    let snap = row.dashboard_snapshot.as_ref()?;
    if snap.get("tab").and_then(|t| t.as_str()) != Some(tab.as_str()) {
        return None;
    }
    serde_json::from_value(snap.get("data")?.clone()).ok()
}

/// Decrypts the stored access token, mapping a failure to an opaque GitHub error.
fn decrypt_token(state: &AppState, row: &gh::Model) -> Result<String, AppError> {
    state
        .cipher
        .open(&Sealed {
            ciphertext: row.access_token_ciphertext.clone(),
            iv: row.access_token_iv.clone(),
            auth_tag: row.access_token_auth_tag.clone(),
        })
        .map_err(|_| AppError::from(GithubError::Api("token decryption failed".into())))
}

/// Writes a freshly fetched dashboard through to the in-memory cache and the
/// durable snapshot (best-effort; a snapshot write failure is non-fatal).
async fn write_through(
    state: &AppState,
    user_id: Uuid,
    tab: GithubQueueKey,
    dashboard: &GithubDashboard,
) {
    state.dashboard_cache.set(user_id, tab, dashboard.clone());
    let snapshot = serde_json::to_value(dashboard).unwrap_or(serde_json::Value::Null);
    let _ = gh::set_dashboard_snapshot(&state.db, user_id, tab.as_str(), snapshot).await;
}

/// Fetch fresh from GitHub; write through to the token cache, the dashboard
/// cache, and the durable snapshot (port of `dashboard-load.ts#refresh`).
async fn refresh(
    state: &AppState,
    user_id: Uuid,
    tab: GithubQueueKey,
    row: &gh::Model,
) -> Result<GithubDashboard, AppError> {
    let token = decrypt_token(state, row)?;
    let login = row.github_login.clone();
    let repos = json_string_array(&row.selected_repos);
    state.token_cache.set(
        user_id,
        CachedPat { token: token.clone(), login: login.clone(), selected_repos: repos.clone() },
    );

    let data = fetch_dashboard(&token, &login, &repos, tab).await?;
    let dashboard = GithubDashboard {
        account: account_summary(row),
        queues: data.queues,
        queue_pulls: data.queue_pulls,
    };
    write_through(state, user_id, tab, &dashboard).await;
    Ok(dashboard)
}

/// Best-effort background bump of `last_used_at` (fire-and-forget).
fn spawn_touch_last_used(state: &web::Data<AppState>, user_id: Uuid) {
    let st = state.clone();
    tokio::spawn(async move {
        let _ = gh::touch_last_used(&st.db, user_id).await;
    });
}

/// SWR dashboard load (port of `runDashboard`).
pub async fn get_dashboard(
    state: &web::Data<AppState>,
    user_id: Uuid,
    tab: GithubQueueKey,
) -> Result<GithubDashboard, AppError> {
    if let Some(hit) = state.dashboard_cache.peek(user_id, tab) {
        if hit.fresh {
            return Ok(hit.value);
        }
    }
    let Some(row) = gh::select_row(&state.db, user_id).await? else {
        return Ok(GithubDashboard::empty());
    };
    spawn_touch_last_used(state, user_id);

    let stale = state
        .dashboard_cache
        .peek(user_id, tab)
        .map(|h| h.value)
        .or_else(|| snapshot_for(&row, tab));

    match stale {
        None => refresh(state, user_id, tab, &row).await,
        Some(stale) => {
            spawn_revalidate(state, user_id, tab, row);
            Ok(stale)
        }
    }
}

/// Single-flight background revalidation: refresh in the background iff no other
/// refresh for this `(user, tab)` is already in flight.
fn spawn_revalidate(
    state: &web::Data<AppState>,
    user_id: Uuid,
    tab: GithubQueueKey,
    row: gh::Model,
) {
    if state.dashboard_cache.try_begin_refresh(user_id, tab) {
        let st = state.clone();
        tokio::spawn(async move {
            let _ = refresh(&st, user_id, tab, &row).await;
            st.dashboard_cache.end_refresh(user_id, tab);
        });
    }
}

/// `GET /me/github/queue` (port of `runQueue`).
pub async fn get_queue(
    state: &AppState,
    user_id: Uuid,
    key: GithubQueueKey,
) -> Result<wf_github::dashboard::types::GithubPullRequestQueue, AppError> {
    let pat = super::pat::resolve_pat(state, user_id)
        .await?
        .ok_or_else(|| AppError::from(GithubError::Api("No GitHub token connected".into())))?;
    Ok(fetch_queue_pulls(&pat.token, &pat.login, &pat.selected_repos, key).await?)
}

/// `GET /me/github/pull` (port of `runPullEnrichment`): enrich a single PR.
pub async fn get_pull_enrichment(
    state: &AppState,
    user_id: Uuid,
    r: RepoRef,
    number: i64,
) -> Result<GithubPullRequestEnrichment, AppError> {
    let pat = super::pat::resolve_pat(state, user_id)
        .await?
        .ok_or_else(|| AppError::from(GithubError::Api("No GitHub token connected".into())))?;
    Ok(enrich_pull_request(&pat.token, &r, number).await)
}

/// `POST /me/github/pulls/enrich` (port of `runPullEnrichments`): batch-enrich.
pub async fn get_pull_enrichments(
    state: &AppState,
    user_id: Uuid,
    refs: &[GithubPullRef],
) -> Result<Vec<GithubPullEnrichmentResult>, AppError> {
    let pat = super::pat::resolve_pat(state, user_id)
        .await?
        .ok_or_else(|| AppError::from(GithubError::Api("No GitHub token connected".into())))?;
    Ok(enrich_pull_requests(&pat.token, refs).await)
}

/// `GET /me/github/repos` (port of `runListRepos`).
pub async fn list_repos(state: &AppState, user_id: Uuid) -> Result<RepoSelection, AppError> {
    let Some(pat) = super::pat::resolve_pat(state, user_id).await? else {
        return Ok(RepoSelection { available: vec![], selected: vec![] });
    };
    let available = list_repositories(&pat.token).await?;
    Ok(RepoSelection { available, selected: pat.selected_repos })
}

/// `PUT /me/github/repos` (port of `runSetRepos`): set selection, bust caches,
/// return the refreshed connection summary.
pub async fn set_selected_repos(
    state: &AppState,
    user_id: Uuid,
    repos: &[String],
) -> Result<crate::github::summary::GithubConnectionSummary, AppError> {
    gh::set_selected_repos(&state.db, user_id, repos).await?;
    state.token_cache.clear(user_id);
    state.dashboard_cache.clear(user_id);
    super::pat::status(state, user_id).await
}
