//! GitHub dashboard orchestration backed by durable sync projections.

use std::collections::HashSet;

use actix_web::web;
use chrono::SecondsFormat;
use sea_orm::prelude::Uuid;
use serde::Serialize;
use wf_db::tables::{github_pat_connections as gh, github_pull_requests};
use wf_github::{
    enrich_pull_request, enrich_pull_requests, list_repositories, GithubAccountSummary,
    GithubDashboard, GithubDashboardActor, GithubDashboardRepository, GithubError,
    GithubPullEnrichmentResult, GithubPullRef, GithubPullRequestBasic, GithubPullRequestEnrichment,
    GithubPullRequestQueue, GithubQueueCount, GithubQueueKey, GithubRepoOption, RepoRef,
};

use crate::error::AppError;
use crate::github::summary::json_string_array;
use crate::state::AppState;

const MAX_QUEUE_PULLS: usize = 30;
const MAX_ENRICH_REFS: usize = 8;
const MAX_SELECTED_REPOS: usize = 25;

struct DashboardPull {
    row: github_pull_requests::Model,
    assignees: HashSet<String>,
    reviewers: HashSet<String>,
}

/// `GET /me/github/repos` response.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepoSelection {
    pub available: Vec<GithubRepoOption>,
    pub selected: Vec<String>,
    pub next_page: Option<u16>,
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

fn queue_meta(key: GithubQueueKey) -> &'static str {
    match key {
        GithubQueueKey::Assigned => "Assigned",
        GithubQueueKey::ReviewRequested => "Review requested",
        GithubQueueKey::Authored => "Authored",
        GithubQueueKey::Mentioned => "Mentioned",
        GithubQueueKey::FailingCi => "Failing CI",
    }
}

fn json_logins(value: &serde_json::Value) -> Vec<String> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

fn dashboard_pull(row: github_pull_requests::Model) -> DashboardPull {
    DashboardPull {
        assignees: json_logins(&row.assignee_logins).into_iter().collect(),
        reviewers: json_logins(&row.requested_reviewer_logins).into_iter().collect(),
        row,
    }
}

fn matches_queue(pull: &DashboardPull, key: GithubQueueKey, login: &str) -> bool {
    match key {
        GithubQueueKey::Assigned => pull.assignees.contains(login),
        GithubQueueKey::ReviewRequested => pull.reviewers.contains(login),
        GithubQueueKey::Authored => pull.row.author_login.as_deref() == Some(login),
        GithubQueueKey::Mentioned | GithubQueueKey::FailingCi => false,
    }
}

fn actor(login: String) -> GithubDashboardActor {
    GithubDashboardActor { login, avatar_url: String::new(), url: String::new() }
}

fn pull_basic(pull: &github_pull_requests::Model) -> GithubPullRequestBasic {
    let repo_url = format!("https://github.com/{}", pull.repo);
    GithubPullRequestBasic {
        repository: GithubDashboardRepository {
            full_name: pull.repo.clone(),
            url: repo_url.clone(),
            actions_url: format!("{repo_url}/actions"),
            is_private: false,
            is_archived: false,
            default_branch: String::new(),
        },
        number: pull.number,
        title: pull.title.clone(),
        url: pull.url.clone(),
        author: actor(pull.author_login.clone().unwrap_or_else(|| "ghost".into())),
        assignees: json_logins(&pull.assignee_logins).into_iter().map(actor).collect(),
        labels: vec![],
        comments: 0,
        created_at: pull.updated_at.to_rfc3339(),
        updated_at: pull.updated_at.to_rfc3339(),
    }
}

/// Best-effort background bump of `last_used_at` (fire-and-forget).
fn spawn_touch_last_used(state: &web::Data<AppState>, user_id: Uuid) {
    let st = state.clone();
    tokio::spawn(async move {
        let _ = gh::touch_last_used(&st.db, user_id).await;
    });
}

fn queue(
    pulls: &[DashboardPull],
    key: GithubQueueKey,
    login: &str,
) -> GithubPullRequestQueue {
    let matching: Vec<_> = pulls.iter().filter(|p| matches_queue(p, key, login)).collect();
    GithubPullRequestQueue {
        key,
        label: queue_meta(key).to_string(),
        total_count: matching.len() as i64,
        incomplete_results: false,
        pull_requests: matching
            .into_iter()
            .take(MAX_QUEUE_PULLS)
            .map(|pull| pull_basic(&pull.row))
            .collect(),
    }
}

fn queue_count(pulls: &[DashboardPull], key: GithubQueueKey, login: &str) -> GithubQueueCount {
    let total_count = pulls.iter().filter(|pull| matches_queue(pull, key, login)).count() as i64;
    GithubQueueCount {
        key,
        label: queue_meta(key).to_string(),
        total_count,
        incomplete_results: false,
    }
}

fn queue_keys() -> [GithubQueueKey; 5] {
    [
        GithubQueueKey::Assigned,
        GithubQueueKey::ReviewRequested,
        GithubQueueKey::Authored,
        GithubQueueKey::Mentioned,
        GithubQueueKey::FailingCi,
    ]
}

/// Dashboard assembled from the durable pull-request projection.
pub async fn get_dashboard(
    state: &web::Data<AppState>,
    user_id: Uuid,
    tab: GithubQueueKey,
) -> Result<GithubDashboard, AppError> {
    let Some(row) = gh::select_row(&state.db, user_id).await? else {
        return Ok(GithubDashboard::empty());
    };
    spawn_touch_last_used(state, user_id);
    let repos = json_string_array(&row.selected_repos);
    let pulls = github_pull_requests::list_recent(&state.db, user_id, &repos, 500)
        .await?
        .into_iter()
        .map(dashboard_pull)
        .collect::<Vec<_>>();
    let queues = queue_keys().into_iter().map(|key| queue_count(&pulls, key, &row.github_login)).collect();
    Ok(GithubDashboard {
        account: account_summary(&row),
        queues,
        queue_pulls: vec![queue(&pulls, tab, &row.github_login)],
    })
}

/// Queue assembled from the durable pull-request projection.
pub async fn get_queue(
    state: &AppState,
    user_id: Uuid,
    key: GithubQueueKey,
) -> Result<GithubPullRequestQueue, AppError> {
    let Some(row) = gh::select_row(&state.db, user_id).await? else {
        return Ok(queue(&[], key, ""));
    };
    let repos = json_string_array(&row.selected_repos);
    let pulls = github_pull_requests::list_recent(&state.db, user_id, &repos, 500)
        .await?
        .into_iter()
        .map(dashboard_pull)
        .collect::<Vec<_>>();
    Ok(queue(&pulls, key, &row.github_login))
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
    Ok(enrich_pull_requests(&pat.token, &refs[..refs.len().min(MAX_ENRICH_REFS)]).await)
}

/// `GET /me/github/repos` (port of `runListRepos`).
pub async fn list_repos(
    state: &AppState,
    user_id: Uuid,
    page: u16,
    per_page: u16,
) -> Result<RepoSelection, AppError> {
    let Some(pat) = super::pat::resolve_pat(state, user_id).await? else {
        return Ok(RepoSelection { available: vec![], selected: vec![], next_page: None });
    };
    let per_page = per_page.clamp(1, 100);
    let available = list_repositories(&pat.token, page, per_page).await?;
    let next_page = (available.len() == usize::from(per_page)).then_some(page.saturating_add(1));
    Ok(RepoSelection { available, selected: pat.selected_repos, next_page })
}

/// `PUT /me/github/repos`: set the bounded projection scope.
pub async fn set_selected_repos(
    state: &AppState,
    user_id: Uuid,
    repos: &[String],
) -> Result<crate::github::summary::GithubConnectionSummary, AppError> {
    if repos.len() > MAX_SELECTED_REPOS {
        return Err(AppError::validation("At most 25 GitHub repositories may be selected."));
    }
    gh::set_selected_repos(&state.db, user_id, repos).await?;
    state.token_cache.clear(user_id);
    super::pat::status(state, user_id).await
}
