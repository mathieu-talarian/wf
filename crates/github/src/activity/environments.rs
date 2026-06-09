//! Repo deployment-environment names (port of `workflows/environments.ts`) —
//! the values GitHub offers for a `type: environment` workflow_dispatch input.

use reqwest::Method;
use serde::Deserialize;

use crate::client::{GithubClient, RepoRef};
use crate::errors::GithubError;

#[derive(Deserialize)]
struct ApiEnvironment {
    name: String,
}

#[derive(Deserialize, Default)]
struct ApiEnvironments {
    #[serde(default)]
    environments: Vec<ApiEnvironment>,
}

/// List a repo's environment names (port of `listEnvironments`).
pub async fn list_environments(token: &str, r: &RepoRef) -> Result<Vec<String>, GithubError> {
    let client = GithubClient::new(token);
    let path = format!("/repos/{}/{}/environments", r.owner, r.repo);
    let resp = client
        .request(Method::GET, &path)
        .query(&[("per_page", "100")])
        .send()
        .await
        .map_err(|e| GithubError::Api(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(GithubError::Api(format!("environments HTTP {}", resp.status().as_u16())));
    }
    let body: ApiEnvironments = resp.json().await.map_err(|e| GithubError::Api(e.to_string()))?;
    Ok(body.environments.into_iter().map(|e| e.name).collect())
}
