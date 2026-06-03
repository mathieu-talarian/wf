//! Runnable workflows + their recent dispatch runs (read half of
//! `workflows/workflows.ts`; `dispatchWorkflow` lands with 3d.2). Per-repo
//! workflow lookups swallow errors into an `error` entry; the single-repo run
//! list propagates failures.

use std::collections::HashMap;

use futures::future::join_all;
use reqwest::Method;
use serde::Deserialize;

use super::branches_graphql::{to_coord, RepoCoord};
use super::types::{GithubRepoWorkflows, GithubWorkflowSummary};
use super::write::write_send;
use crate::client::{GithubClient, RepoRef};
use crate::dashboard::types::GithubWorkflowRunSummary;
use crate::errors::GithubError;

#[derive(Deserialize)]
struct ApiWorkflow {
    id: i64,
    name: String,
    path: String,
    state: String,
    html_url: String,
}

#[derive(Deserialize)]
struct ApiWorkflows {
    workflows: Vec<ApiWorkflow>,
}

fn to_summary(w: ApiWorkflow) -> GithubWorkflowSummary {
    GithubWorkflowSummary { id: w.id, name: w.name, path: w.path, state: w.state, url: w.html_url }
}

fn error_repo(coord: &RepoCoord) -> GithubRepoWorkflows {
    GithubRepoWorkflows {
        repo_full_name: coord.full_name.clone(),
        repo_url: format!("https://github.com/{}", coord.full_name),
        default_branch: String::new(),
        workflows: vec![],
        error: Some("Could not load workflows for this repository.".to_string()),
    }
}

async fn fetch_repo_workflows(client: &GithubClient, coord: &RepoCoord) -> GithubRepoWorkflows {
    let path = format!("/repos/{}/{}/actions/workflows", coord.owner, coord.name);
    let result: Option<ApiWorkflows> = async {
        let resp = client.request(Method::GET, &path).query(&[("per_page", "100")]).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json::<ApiWorkflows>().await.ok()
    }
    .await;
    match result {
        None => error_repo(coord),
        Some(data) => GithubRepoWorkflows {
            repo_full_name: coord.full_name.clone(),
            repo_url: format!("https://github.com/{}", coord.full_name),
            default_branch: String::new(),
            workflows: data
                .workflows
                .into_iter()
                .filter(|w| w.state == "active")
                .map(to_summary)
                .collect(),
            error: None,
        },
    }
}

/// Active workflows across the user's selected repos (port of `fetchWorkflows`).
pub async fn fetch_workflows(token: &str, repos: &[String]) -> Vec<GithubRepoWorkflows> {
    let coords: Vec<RepoCoord> = repos.iter().filter_map(|r| to_coord(r)).collect();
    if coords.is_empty() {
        return vec![];
    }
    let client = GithubClient::new(token);
    join_all(coords.iter().map(|c| fetch_repo_workflows(&client, c))).await
}

#[derive(Deserialize)]
struct ApiRun {
    id: i64,
    #[serde(default)]
    name: Option<String>,
    status: Option<String>,
    conclusion: Option<String>,
    html_url: String,
    run_number: i64,
    event: String,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct ApiRuns {
    workflow_runs: Vec<ApiRun>,
}

fn to_run(run: ApiRun) -> GithubWorkflowRunSummary {
    GithubWorkflowRunSummary {
        id: run.id,
        name: run.name,
        status: run.status.unwrap_or_else(|| "unknown".to_string()),
        conclusion: run.conclusion,
        url: run.html_url,
        run_number: run.run_number,
        event: run.event,
        created_at: run.created_at,
        updated_at: run.updated_at,
    }
}

/// Recent `workflow_dispatch` runs for one workflow on a branch (port of
/// `listWorkflowRuns`): 5 most recent, dispatch-triggered only.
pub async fn list_workflow_runs(
    token: &str,
    r: &RepoRef,
    workflow_id: i64,
    branch: &str,
) -> Result<Vec<GithubWorkflowRunSummary>, GithubError> {
    let client = GithubClient::new(token);
    let path = format!("/repos/{}/{}/actions/workflows/{workflow_id}/runs", r.owner, r.repo);
    let resp = client
        .request(Method::GET, &path)
        .query(&[("branch", branch), ("event", "workflow_dispatch"), ("per_page", "5")])
        .send()
        .await
        .map_err(|e| GithubError::Api(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(GithubError::Api(format!("workflow runs HTTP {}", resp.status().as_u16())));
    }
    let runs: ApiRuns = resp.json().await.map_err(|e| GithubError::Api(e.to_string()))?;
    Ok(runs.workflow_runs.into_iter().map(to_run).collect())
}

/// Trigger a `workflow_dispatch` (port of `dispatchWorkflow`). 204 on success;
/// failures pass the status through as a write error.
pub async fn dispatch_workflow(
    token: &str,
    r: &RepoRef,
    workflow_id: i64,
    git_ref: &str,
    inputs: &HashMap<String, String>,
) -> Result<(), GithubError> {
    let client = GithubClient::new(token);
    let payload = serde_json::json!({ "ref": git_ref, "inputs": inputs });
    let path = format!("/repos/{}/{}/actions/workflows/{workflow_id}/dispatches", r.owner, r.repo);
    write_send(client.request(Method::POST, &path).json(&payload), "Failed to dispatch the workflow.")
        .await?;
    Ok(())
}
