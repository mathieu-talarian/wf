//! Actions-strip runs: recent workflow runs across the selected repos,
//! favorites flagged (and filtered to favorites when any exist), newest first.

use std::collections::HashMap;

use futures::stream::{self, StreamExt};
use sea_orm::prelude::Uuid;
use tracing::Instrument;
use wf_db::tables::github_pat_connections as gh;
use wf_github::{GithubClient, PolledWorkflowRun, list_runs_any_status};

use crate::error::AppError;
use crate::github::activity::require_pat;
use crate::hub::cache;
use crate::hub::types::{HubRunPill, HubRuns};
use crate::state::AppState;

const MAX_REPOS: usize = 6;
const MAX_PILLS: usize = 14;

pub(crate) fn run_status(run: &PolledWorkflowRun) -> &'static str {
    match (run.status.as_deref(), run.conclusion.as_deref()) {
        (_, Some("success")) => "success",
        (_, Some(_)) => "failed",
        (Some("completed"), None) => "failed",
        _ => "running",
    }
}

/// Stale-while-revalidate: serve fresh instantly, serve stale + refresh in the
/// background (never blocking the request on GitHub), and single-flight the cold
/// path so concurrent first hits do the live fetch once.
pub async fn runs(state: &AppState, user_id: Uuid) -> Result<HubRuns, AppError> {
    match cache::get_runs_swr(user_id) {
        cache::Freshness::Fresh(v) => return Ok(v),
        cache::Freshness::Stale(v) => {
            spawn_refresh(state, user_id);
            return Ok(v);
        }
        cache::Freshness::Missing => {}
    }
    let lock = cache::refresh_lock("runs", user_id);
    let _guard = lock.lock().await;
    if let Some(cached) = cache::get_runs(user_id) {
        return Ok(cached);
    }
    let result = compute_runs(state, user_id).await?;
    cache::put_runs(user_id, &result);
    Ok(result)
}

/// Background revalidation, guarded so only one refresh per user runs at a time.
fn spawn_refresh(state: &AppState, user_id: Uuid) {
    let lock = cache::refresh_lock("runs", user_id);
    let Ok(guard) = lock.try_lock_owned() else { return };
    let state = state.clone();
    // actix's runtime spawn (no `Send` bound) — consistent with board's refresh.
    actix_web::rt::spawn(
        async move {
            let _guard = guard;
            if let Ok(result) = compute_runs(&state, user_id).await {
                cache::put_runs(user_id, &result);
            }
        }
        .instrument(tracing::info_span!("hub.refresh", kind = "runs", user_id = %user_id)),
    );
}

/// The live fetch+assemble (GitHub). Kept on the live path on purpose: the
/// `events` sync table lacks `workflowId` and in-progress runs, so it can't
/// back favorites filtering or "running" pills.
async fn compute_runs(state: &AppState, user_id: Uuid) -> Result<HubRuns, AppError> {
    let pat = require_pat(state, user_id).await?;
    let favorites: HashMap<String, Vec<i64>> = gh::get_favorites(&state.db, user_id).await?;
    let repos: Vec<String> = pat.selected_repos.iter().take(MAX_REPOS).cloned().collect();
    let pages = fetch_run_pages(&pat.token, &repos).await;

    let mut pills = build_pills(pages, &favorites);
    pills.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    pills.truncate(MAX_PILLS);
    Ok(HubRuns { runs: pills })
}

/// Fetches recent runs (any status) per repo, bounded concurrency.
async fn fetch_run_pages(token: &str, repos: &[String]) -> Vec<(String, Vec<PolledWorkflowRun>)> {
    let client = GithubClient::new(token);
    stream::iter(repos.to_vec())
        .map(|repo| {
            let client = &client;
            async move {
                let Some((owner, name)) = repo.split_once('/') else {
                    return (repo, vec![]);
                };
                let runs = list_runs_any_status(client, owner, name)
                    .await
                    .unwrap_or_default();
                (repo, runs)
            }
        })
        .buffered(MAX_REPOS)
        .collect()
        .await
}

/// Flattens run pages into pills; when any favorites exist, keeps only those.
fn build_pills(
    pages: Vec<(String, Vec<PolledWorkflowRun>)>,
    favorites: &HashMap<String, Vec<i64>>,
) -> Vec<HubRunPill> {
    let any_favorites = favorites.values().any(|ids| !ids.is_empty());
    let mut pills: Vec<HubRunPill> = Vec::new();
    for (repo, runs) in pages {
        let favorite_ids = favorites.get(&repo).cloned().unwrap_or_default();
        for run in runs {
            let is_favorite = favorite_ids.contains(&run.workflow_id.unwrap_or_default());
            if any_favorites && !is_favorite {
                continue;
            }
            pills.push(run_pill(&repo, &run, is_favorite));
        }
    }
    pills
}

fn run_pill(repo: &str, run: &PolledWorkflowRun, is_favorite: bool) -> HubRunPill {
    HubRunPill {
        repo: repo.to_string(),
        workflow_id: run.workflow_id.unwrap_or_default().to_string(),
        workflow_name: run.name.clone().unwrap_or_else(|| "workflow".to_string()),
        run_id: run.id.to_string(),
        status: run_status(run).to_string(),
        version: None,
        started_at: run.created_at.to_rfc3339(),
        url: run.html_url.clone().unwrap_or_default(),
        is_favorite,
    }
}
