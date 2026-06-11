//! Git ref writes: branch creation for the slide-over's `new branch` action.

use reqwest::Method;

use crate::activity::write::write_send;
use crate::client::GithubClient;
use crate::errors::GithubError;
use crate::RepoRef;

/// The created branch (name + head sha).
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubBranchCreated {
    pub repo: String,
    pub name: String,
    pub sha: String,
}

async fn ref_sha(client: &GithubClient, r: &RepoRef, branch: &str) -> Result<String, GithubError> {
    let path = format!("/repos/{}/{}/git/ref/heads/{branch}", r.owner, r.repo);
    let resp = client
        .request(Method::GET, &path)
        .send()
        .await
        .map_err(|e| GithubError::Api(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(GithubError::Api(format!("ref lookup HTTP {}", resp.status().as_u16())));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| GithubError::Api(e.to_string()))?;
    body.pointer("/object/sha")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| GithubError::Api("ref lookup: missing object.sha".to_string()))
}

async fn default_branch(client: &GithubClient, r: &RepoRef) -> Result<String, GithubError> {
    let path = format!("/repos/{}/{}", r.owner, r.repo);
    let resp = client
        .request(Method::GET, &path)
        .send()
        .await
        .map_err(|e| GithubError::Api(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(GithubError::Api(format!("repo lookup HTTP {}", resp.status().as_u16())));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| GithubError::Api(e.to_string()))?;
    body.get("default_branch")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| GithubError::Api("repo lookup: missing default_branch".to_string()))
}

/// Create `refs/heads/{name}` from `from_ref` (default: the repo's default
/// branch head).
pub async fn create_branch(
    token: &str,
    r: &RepoRef,
    name: &str,
    from_ref: Option<&str>,
) -> Result<GithubBranchCreated, GithubError> {
    let client = GithubClient::new(token);
    let base = match from_ref {
        Some(base) => base.to_string(),
        None => default_branch(&client, r).await?,
    };
    let sha = ref_sha(&client, r, &base).await?;
    let path = format!("/repos/{}/{}/git/refs", r.owner, r.repo);
    let payload = serde_json::json!({ "ref": format!("refs/heads/{name}"), "sha": sha });
    write_send(client.request(Method::POST, &path).json(&payload), "Failed to create the branch.")
        .await?;
    Ok(GithubBranchCreated {
        repo: format!("{}/{}", r.owner, r.repo),
        name: name.to_string(),
        sha,
    })
}
