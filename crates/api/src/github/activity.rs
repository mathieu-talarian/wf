//! GitHub Activity / Repositories-hub read orchestration (port of
//! `activity/runners.ts` read paths). Each call resolves the user's PAT
//! (cache-first) and fails with "No GitHub token connected" when unconnected,
//! matching the TS `withConn` guard.

use std::collections::HashMap;

use sea_orm::prelude::Uuid;
use wf_github::{
    fetch_branch_prompts, fetch_workflow_inputs, fetch_workflows, list_environments,
    list_repo_branch_names, list_workflow_runs, GithubCreatePullInput, GithubCreatePullResult,
    GithubError, GithubMergeMethod, GithubMergePullResult, GithubRepoBranches, GithubRepoWorkflows,
    GithubWorkflowInputs, GithubWorkflowRunSummary, RepoRef,
};

use crate::error::AppError;
use crate::github::token_cache::CachedPat;
use crate::state::AppState;

async fn require_pat(state: &AppState, user_id: Uuid) -> Result<CachedPat, AppError> {
    super::pat::resolve_pat(state, user_id)
        .await?
        .ok_or_else(|| AppError::from(GithubError::Api("No GitHub token connected".into())))
}

/// `GET /me/github/branches` (port of `runListBranches`).
pub async fn list_branches(
    state: &AppState,
    user_id: Uuid,
) -> Result<Vec<GithubRepoBranches>, AppError> {
    let pat = require_pat(state, user_id).await?;
    Ok(fetch_branch_prompts(&pat.token, &pat.login, &pat.selected_repos).await)
}

/// `GET /me/github/workflows` (port of `runListWorkflows`).
pub async fn list_workflows(
    state: &AppState,
    user_id: Uuid,
) -> Result<Vec<GithubRepoWorkflows>, AppError> {
    let pat = require_pat(state, user_id).await?;
    Ok(fetch_workflows(&pat.token, &pat.selected_repos).await)
}

/// `GET /me/github/workflow/inputs` (port of `runWorkflowInputs`).
pub async fn workflow_inputs(
    state: &AppState,
    user_id: Uuid,
    r: RepoRef,
    path: &str,
) -> Result<GithubWorkflowInputs, AppError> {
    let pat = require_pat(state, user_id).await?;
    Ok(fetch_workflow_inputs(&pat.token, &r, path).await?)
}

/// `GET /me/github/repo/branches` (port of `runRepoBranches`).
pub async fn repo_branches(
    state: &AppState,
    user_id: Uuid,
    r: RepoRef,
) -> Result<Vec<String>, AppError> {
    let pat = require_pat(state, user_id).await?;
    Ok(list_repo_branch_names(&pat.token, &r).await?)
}

/// `GET /me/github/repo/environments` (port of `runListEnvironments`).
pub async fn environments(
    state: &AppState,
    user_id: Uuid,
    r: RepoRef,
) -> Result<Vec<String>, AppError> {
    let pat = require_pat(state, user_id).await?;
    Ok(list_environments(&pat.token, &r).await?)
}

/// `GET /me/github/workflow/runs` (port of `runWorkflowRuns`).
pub async fn workflow_runs(
    state: &AppState,
    user_id: Uuid,
    r: RepoRef,
    workflow_id: i64,
    branch: &str,
) -> Result<Vec<GithubWorkflowRunSummary>, AppError> {
    let pat = require_pat(state, user_id).await?;
    Ok(list_workflow_runs(&pat.token, &r, workflow_id, branch).await?)
}

/// `POST /me/github/workflow/dispatch` (port of `runDispatch`).
pub async fn dispatch(
    state: &AppState,
    user_id: Uuid,
    r: RepoRef,
    workflow_id: i64,
    git_ref: &str,
    inputs: &HashMap<String, String>,
) -> Result<(), AppError> {
    let pat = require_pat(state, user_id).await?;
    Ok(wf_github::dispatch_workflow(&pat.token, &r, workflow_id, git_ref, inputs).await?)
}

/// `POST /me/github/pulls` (port of `runCreatePull`).
pub async fn create_pull(
    state: &AppState,
    user_id: Uuid,
    r: RepoRef,
    input: &GithubCreatePullInput,
) -> Result<GithubCreatePullResult, AppError> {
    let pat = require_pat(state, user_id).await?;
    Ok(wf_github::create_pull(&pat.token, &r, input).await?)
}

/// `POST /me/github/pull/merge` (port of `runMergePull`).
pub async fn merge_pull(
    state: &AppState,
    user_id: Uuid,
    r: RepoRef,
    pull_number: i64,
    method: GithubMergeMethod,
) -> Result<GithubMergePullResult, AppError> {
    let pat = require_pat(state, user_id).await?;
    Ok(wf_github::merge_pull(&pat.token, &r, pull_number, method).await?)
}

/// `POST /me/github/pull/close` (port of `runClosePull`).
pub async fn close_pull(
    state: &AppState,
    user_id: Uuid,
    r: RepoRef,
    pull_number: i64,
) -> Result<(), AppError> {
    let pat = require_pat(state, user_id).await?;
    Ok(wf_github::close_pull(&pat.token, &r, pull_number).await?)
}
