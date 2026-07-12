//! Actions-strip runs: recent workflow runs across the selected repos,
//! favorites flagged (and filtered to favorites when any exist), newest first.

use std::collections::HashMap;

use sea_orm::prelude::Uuid;
use wf_db::tables::{github_pat_connections as gh, github_workflow_runs};
use wf_github::PolledWorkflowRun;

use crate::error::AppError;
use crate::hub::types::{HubRunPill, HubRuns};
use crate::state::AppState;

const MAX_PILLS: usize = 14;

pub(crate) fn run_status(run: &PolledWorkflowRun) -> &'static str {
    match (run.status.as_deref(), run.conclusion.as_deref()) {
        (_, Some("success")) => "success",
        (_, Some(_)) => "failed",
        (Some("completed"), None) => "failed",
        _ => "running",
    }
}

/// Durable read-model load: no provider I/O and no process-local data cache.
pub async fn runs(state: &AppState, user_id: Uuid) -> Result<HubRuns, AppError> {
    compute_runs(state, user_id).await
}

async fn compute_runs(state: &AppState, user_id: Uuid) -> Result<HubRuns, AppError> {
    let Some(row) = gh::select_row(&state.db, user_id).await? else {
        return Ok(HubRuns { runs: vec![] });
    };
    let repos = selected_repos(&row);
    let rows = github_workflow_runs::list_recent(&state.db, user_id, &repos, 200).await?;
    Ok(from_projection(rows, &gh::favorites_of(&row)))
}

pub(crate) fn from_projection(
    rows: Vec<github_workflow_runs::Model>,
    favorites: &HashMap<String, Vec<i64>>,
) -> HubRuns {
    let mut pills = build_pills(group_runs(rows), favorites);
    pills.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    pills.truncate(MAX_PILLS);
    HubRuns { runs: pills }
}

fn selected_repos(row: &gh::Model) -> Vec<String> {
    row.selected_repos
        .clone()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn group_runs(rows: Vec<github_workflow_runs::Model>) -> Vec<(String, Vec<PolledWorkflowRun>)> {
    let mut grouped: HashMap<String, Vec<PolledWorkflowRun>> = HashMap::new();
    for row in rows {
        grouped.entry(row.repo.clone()).or_default().push(projected_run(row));
    }
    grouped.into_iter().collect()
}

fn projected_run(row: github_workflow_runs::Model) -> PolledWorkflowRun {
    PolledWorkflowRun {
        id: row.run_id,
        workflow_id: row.workflow_id,
        run_attempt: 1,
        name: Some(row.name),
        display_title: None,
        status: Some(row.status),
        conclusion: row.conclusion,
        html_url: Some(row.url),
        head_branch: row.head_branch,
        created_at: row.created_at.with_timezone(&chrono::Utc),
        updated_at: row.updated_at.with_timezone(&chrono::Utc),
        actor: None,
    }
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
