//! Branch→PR prompts (port of `branches.ts`): a batched, per-repo GraphQL sweep
//! that never fails the whole call — inaccessible repos surface as an `error`
//! string in their own entry. Plus the plain REST branch-name list.

use futures::future::join_all;
use reqwest::Method;

use super::branches_graphql::{
    branch_variables, build_branch_query, decode_repo, select_branches, to_coord, GqlBranchRepo,
    RepoCoord,
};
use super::types::GithubRepoBranches;
use crate::client::{GithubClient, RepoRef};
use crate::errors::GithubError;

const REPO_BATCH: usize = 20;
const WINDOW_DAYS: i64 = 30;

fn error_repo(coord: &RepoCoord, message: &str) -> GithubRepoBranches {
    GithubRepoBranches {
        repo_full_name: coord.full_name.clone(),
        repo_url: format!("https://github.com/{}", coord.full_name),
        default_branch: String::new(),
        branches: vec![],
        error: Some(message.to_string()),
    }
}

fn to_repo_branches(
    coord: &RepoCoord,
    raw: Option<serde_json::Value>,
    login: &str,
    cutoff_ms: i64,
) -> GithubRepoBranches {
    let is_null = matches!(&raw, None | Some(serde_json::Value::Null));
    if is_null {
        return error_repo(coord, "Repository not found or inaccessible.");
    }
    let repo: GqlBranchRepo = match decode_repo(raw.unwrap()) {
        Ok(r) => r,
        Err(_) => return error_repo(coord, "Repository not found or inaccessible."),
    };
    GithubRepoBranches {
        repo_full_name: repo.name_with_owner().to_string(),
        repo_url: repo.url().to_string(),
        default_branch: repo.default_branch().to_string(),
        branches: select_branches(&repo, login, cutoff_ms),
        error: None,
    }
}

/// POSTs a GraphQL query and returns the `data` object, tolerating partial
/// responses (GitHub returns 200 with both `data` and `errors` when some repos
/// in the batch are inaccessible). `None` on transport error or non-2xx.
async fn graphql_data(
    client: &GithubClient,
    query: &str,
    variables: serde_json::Value,
) -> Option<serde_json::Value> {
    let body = serde_json::json!({ "query": query, "variables": variables });
    let resp = client.request(Method::POST, "/graphql").json(&body).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let mut payload: serde_json::Value = resp.json().await.ok()?;
    Some(payload.get_mut("data").map(serde_json::Value::take).unwrap_or(serde_json::Value::Null))
}

async fn run_branch_batch(
    client: &GithubClient,
    coords: &[RepoCoord],
    login: &str,
    cutoff_ms: i64,
) -> Vec<GithubRepoBranches> {
    let data = graphql_data(client, &build_branch_query(coords), branch_variables(coords)).await;
    match data {
        // Transport / non-2xx: every repo in the batch failed.
        None => coords.iter().map(|c| error_repo(c, "GitHub request failed.")).collect(),
        Some(mut data) => coords
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let raw = data.get_mut(format!("r{i}")).map(serde_json::Value::take);
                to_repo_branches(c, raw, login, cutoff_ms)
            })
            .collect(),
    }
}

/// Branch prompts across the user's selected repos (port of `fetchBranchPrompts`).
pub async fn fetch_branch_prompts(
    token: &str,
    login: &str,
    repos: &[String],
) -> Vec<GithubRepoBranches> {
    let coords: Vec<RepoCoord> = repos.iter().filter_map(|r| to_coord(r)).collect();
    // DIAGNOSTIC (temporary): record the inputs that drive the whole sweep.
    tracing::info!(
        target: "branch_prompts",
        login,
        selected_repos = repos.len(),
        valid_coords = coords.len(),
        "fetch_branch_prompts: inputs"
    );
    if coords.is_empty() {
        tracing::warn!(
            target: "branch_prompts",
            "no valid repo coordinates -> returning empty (is any repo selected?)"
        );
        return vec![];
    }
    let client = GithubClient::new(token);
    let cutoff_ms =
        chrono::Utc::now().timestamp_millis() - WINDOW_DAYS * 24 * 60 * 60 * 1000;
    let batches =
        join_all(coords.chunks(REPO_BATCH).map(|g| run_branch_batch(&client, g, login, cutoff_ms)))
            .await;
    batches.into_iter().flatten().collect()
}

#[derive(serde::Deserialize)]
struct ApiBranch {
    name: String,
}

/// Plain REST list of a repo's branch names (port of `listRepoBranchNames`).
/// Unlike the prompt sweep, an upstream failure here is surfaced as an error.
pub async fn list_repo_branch_names(token: &str, r: &RepoRef) -> Result<Vec<String>, GithubError> {
    let client = GithubClient::new(token);
    let path = format!("/repos/{}/{}/branches", r.owner, r.repo);
    let resp = client
        .request(Method::GET, &path)
        .query(&[("per_page", "100")])
        .send()
        .await
        .map_err(|e| GithubError::Api(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(GithubError::Api(format!("branches HTTP {}", resp.status().as_u16())));
    }
    let branches: Vec<ApiBranch> = resp.json().await.map_err(|e| GithubError::Api(e.to_string()))?;
    Ok(branches.into_iter().map(|b| b.name).collect())
}
