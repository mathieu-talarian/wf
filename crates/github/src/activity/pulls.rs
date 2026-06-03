//! PR write operations (port of `pulls.ts`): create, merge, close. All route
//! failures through `write_send` so the HTTP status (403/404/422) is preserved.

use reqwest::Method;
use serde::Deserialize;

use super::types::{
    GithubCreatePullInput, GithubCreatePullResult, GithubMergeMethod, GithubMergePullResult,
};
use super::write::write_send;
use crate::client::{GithubClient, RepoRef};
use crate::errors::GithubError;

#[derive(Deserialize)]
struct ApiCreatedPull {
    number: i64,
    html_url: String,
}

/// Create a PR (port of `createPull`). `body` is omitted when empty, matching
/// the TS spread.
pub async fn create_pull(
    token: &str,
    r: &RepoRef,
    input: &GithubCreatePullInput,
) -> Result<GithubCreatePullResult, GithubError> {
    let client = GithubClient::new(token);
    let mut payload = serde_json::json!({
        "title": input.title,
        "head": input.head,
        "base": input.base,
    });
    if !input.body.is_empty() {
        payload["body"] = serde_json::Value::String(input.body.clone());
    }
    let path = format!("/repos/{}/{}/pulls", r.owner, r.repo);
    let resp =
        write_send(client.request(Method::POST, &path).json(&payload), "Failed to create the pull request.")
            .await?;
    let created: ApiCreatedPull =
        resp.json().await.map_err(|e| GithubError::Api(e.to_string()))?;
    Ok(GithubCreatePullResult { number: created.number, url: created.html_url })
}

#[derive(Deserialize)]
struct ApiMergeResult {
    merged: bool,
    message: String,
}

/// Merge a PR with the given method (port of `mergePull`).
pub async fn merge_pull(
    token: &str,
    r: &RepoRef,
    pull_number: i64,
    method: GithubMergeMethod,
) -> Result<GithubMergePullResult, GithubError> {
    let client = GithubClient::new(token);
    let payload = serde_json::json!({ "merge_method": method.as_str() });
    let path = format!("/repos/{}/{}/pulls/{pull_number}/merge", r.owner, r.repo);
    let resp =
        write_send(client.request(Method::PUT, &path).json(&payload), "Failed to merge the pull request.")
            .await?;
    let result: ApiMergeResult = resp.json().await.map_err(|e| GithubError::Api(e.to_string()))?;
    Ok(GithubMergePullResult { merged: result.merged, message: result.message })
}

/// Close a PR (port of `closePull`): PATCH state=closed.
pub async fn close_pull(token: &str, r: &RepoRef, pull_number: i64) -> Result<(), GithubError> {
    let client = GithubClient::new(token);
    let payload = serde_json::json!({ "state": "closed" });
    let path = format!("/repos/{}/{}/pulls/{pull_number}", r.owner, r.repo);
    write_send(client.request(Method::PATCH, &path).json(&payload), "Failed to close the pull request.")
        .await?;
    Ok(())
}
