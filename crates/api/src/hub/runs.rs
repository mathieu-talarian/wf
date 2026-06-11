//! Actions-strip runs: recent workflow runs across the selected repos,
//! favorites flagged (and filtered to favorites when any exist), newest first.

use std::collections::HashMap;

use futures::stream::{self, StreamExt};
use sea_orm::prelude::Uuid;
use wf_db::tables::github_pat_connections as gh;
use wf_github::{list_runs_any_status, GithubClient, PolledWorkflowRun};

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

pub async fn runs(state: &AppState, user_id: Uuid) -> Result<HubRuns, AppError> {
    if let Some(cached) = cache::get_runs(user_id) {
        return Ok(cached);
    }
    let pat = require_pat(state, user_id).await?;
    let favorites: HashMap<String, Vec<i64>> = gh::get_favorites(&state.db, user_id).await?;
    let client = GithubClient::new(&pat.token);

    let repos: Vec<String> = pat.selected_repos.iter().take(MAX_REPOS).cloned().collect();
    let pages: Vec<(String, Vec<PolledWorkflowRun>)> = stream::iter(repos)
        .map(|repo| {
            let client = &client;
            async move {
                let Some((owner, name)) = repo.split_once('/') else { return (repo, vec![]) };
                let runs = list_runs_any_status(client, owner, name).await.unwrap_or_default();
                (repo, runs)
            }
        })
        .buffered(4)
        .collect()
        .await;

    let any_favorites = favorites.values().any(|ids| !ids.is_empty());
    let mut pills: Vec<HubRunPill> = Vec::new();
    for (repo, runs) in pages {
        let favorite_ids = favorites.get(&repo).cloned().unwrap_or_default();
        for run in runs {
            let workflow_id = run.workflow_id.unwrap_or_default();
            let is_favorite = favorite_ids.contains(&workflow_id);
            if any_favorites && !is_favorite {
                continue;
            }
            pills.push(HubRunPill {
                repo: repo.clone(),
                workflow_id,
                workflow_name: run.name.clone().unwrap_or_else(|| "workflow".to_string()),
                run_id: run.id,
                status: run_status(&run).to_string(),
                version: None,
                started_at: run.created_at.to_rfc3339(),
                url: run.html_url.clone().unwrap_or_default(),
                is_favorite,
            });
        }
    }
    pills.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    pills.truncate(MAX_PILLS);

    let result = HubRuns { runs: pills };
    cache::put_runs(user_id, &result);
    Ok(result)
}
