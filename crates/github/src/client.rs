//! Raw `reqwest` GitHub REST client (replaces Octokit; migration plan §10.1).
//! One HTTP style for all GitHub calls: Bearer auth, the standard GitHub
//! headers, and a thin error classifier for token validation.

use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::{Method, RequestBuilder, Response, StatusCode};

use crate::errors::PatValidationError;
use crate::types::PatValidationStatus;

pub const REST_BASE: &str = "https://api.github.com";
const USER_AGENT_VALUE: &str = "workflow-server";
const API_VERSION: &str = "2022-11-28";

/// A repository reference (`owner`/`repo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
}

/// Parses `https://api.github.com/repos/{owner}/{repo}` (or any path whose last
/// two segments are owner/repo) — port of `parseRepoRef`.
pub fn parse_repo_ref(repository_url: &str) -> Option<RepoRef> {
    let mut parts: Vec<&str> = repository_url.split('/').filter(|s| !s.is_empty()).collect();
    let repo = parts.pop()?;
    let owner = parts.pop()?;
    Some(RepoRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

pub struct GithubClient {
    http: reqwest::Client,
    token: String,
}

impl GithubClient {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            token: token.into(),
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Builds a request with the standard GitHub REST headers.
    pub fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{REST_BASE}{path}"))
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
    }

    /// Executes a GraphQL query against `https://api.github.com/graphql` and
    /// returns the `data` object. GraphQL errors (or a non-2xx response) map to
    /// `GithubError::Api`. Mirrors `octokit.graphql`.
    pub async fn graphql(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, crate::errors::GithubError> {
        let body = serde_json::json!({ "query": query, "variables": variables });
        let mut payload = self.post_graphql(&body).await?;
        if let Some(errors) = graphql_errors(&payload) {
            return Err(crate::errors::GithubError::Api(format!(
                "graphql errors: {errors}"
            )));
        }
        Ok(payload["data"].take())
    }

    /// POSTs a GraphQL body and returns the parsed JSON payload, mapping a
    /// non-2xx response or transport/parse failure to `GithubError::Api`.
    async fn post_graphql(
        &self,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, crate::errors::GithubError> {
        let resp = self
            .http
            .post("https://api.github.com/graphql")
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header(ACCEPT, "application/vnd.github+json")
            .json(body)
            .send()
            .await
            .map_err(|e| crate::errors::GithubError::Api(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(crate::errors::GithubError::Api(format!(
                "graphql HTTP {}",
                resp.status().as_u16()
            )));
        }
        resp.json()
            .await
            .map_err(|e| crate::errors::GithubError::Api(e.to_string()))
    }
}

/// Returns the GraphQL `errors` array when present and non-empty (a null or
/// empty `errors` field is treated as success, matching `octokit.graphql`).
fn graphql_errors(payload: &serde_json::Value) -> Option<&serde_json::Value> {
    payload
        .get("errors")
        .filter(|errors| !errors.is_null() && errors.as_array().map(|a| !a.is_empty()).unwrap_or(true))
}

/// Maps a GitHub error status to a validation status (port of `statusFor`):
/// 401 → invalid, 403 → needs_sso (if the SSO header is present) else
/// missing_permissions, 404 → missing_permissions, anything else → unknown.
pub fn classify_status(http_status: u16, has_sso_header: bool) -> PatValidationStatus {
    match http_status {
        401 => PatValidationStatus::Invalid,
        403 => {
            if has_sso_header {
                PatValidationStatus::NeedsSso
            } else {
                PatValidationStatus::MissingPermissions
            }
        }
        404 => PatValidationStatus::MissingPermissions,
        _ => PatValidationStatus::Unknown,
    }
}

/// True when GitHub set the `x-github-sso` header (SAML SSO authorization
/// needed) — port of `hasSsoHeader`.
pub fn has_sso_header(resp: &Response) -> bool {
    resp.headers()
        .get("x-github-sso")
        .and_then(|v| v.to_str().ok())
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

/// Sends a request, turning transport errors and non-2xx responses into a
/// classified `PatValidationError`. Returns the `Response` on success.
pub async fn send_checked(req: RequestBuilder) -> Result<Response, PatValidationError> {
    let resp = req.send().await.map_err(|_| PatValidationError::unreachable())?;
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status().as_u16();
    let sso = has_sso_header(&resp);
    Err(PatValidationError::of(classify_status(status, sso), Some(status)))
}

/// True for the auth statuses that invalidate a token during best-effort probes
/// (port of `isAuthError`): only 401/403.
pub fn is_auth_status(status: StatusCode) -> bool {
    status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repo_ref_from_api_url() {
        let r = parse_repo_ref("https://api.github.com/repos/octocat/hello-world").unwrap();
        assert_eq!(r.owner, "octocat");
        assert_eq!(r.repo, "hello-world");
    }

    #[test]
    fn classify_matches_state_machine() {
        assert_eq!(classify_status(401, false), PatValidationStatus::Invalid);
        assert_eq!(classify_status(403, true), PatValidationStatus::NeedsSso);
        assert_eq!(classify_status(403, false), PatValidationStatus::MissingPermissions);
        assert_eq!(classify_status(404, false), PatValidationStatus::MissingPermissions);
        assert_eq!(classify_status(500, false), PatValidationStatus::Unknown);
    }
}
