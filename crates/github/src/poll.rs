//! Poller-specific repo reads (A1 spec §5): page-1, newest-first by update
//! recency, raw provider ids/timestamps. Deliberately separate from the
//! dashboard queue reads (user-queue shaped, open-PR-only). Cursor filtering
//! happens in `wf-sync`; these fetch one page.
//!
//! Documented limitation (spec §6.2 deviation, plan header): >50 updates per
//! scope per tick lose the older ones; dedup keeps replays safe.

use chrono::{DateTime, Utc};
use reqwest::Method;
use serde::Deserialize;

use crate::client::GithubClient;
use crate::errors::GithubError;

#[derive(Debug, Clone, Deserialize)]
pub struct GithubActor {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolledWorkflowRun {
    pub id: i64,
    #[serde(default = "default_attempt")]
    pub run_attempt: i64,
    pub name: Option<String>,
    pub display_title: Option<String>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub html_url: Option<String>,
    pub head_branch: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub actor: Option<GithubActor>,
}

fn default_attempt() -> i64 {
    1
}

#[derive(Deserialize)]
struct RunsResponse {
    workflow_runs: Vec<PolledWorkflowRun>,
}

/// Newest page of completed workflow runs for `owner/repo`.
pub async fn list_workflow_runs_page(
    client: &GithubClient,
    owner: &str,
    repo: &str,
) -> Result<Vec<PolledWorkflowRun>, GithubError> {
    let path = format!("/repos/{owner}/{repo}/actions/runs");
    let req = client
        .request(Method::GET, &path)
        .query(&[("status", "completed"), ("per_page", "50")]);
    let body: RunsResponse = send_json(req).await?;
    Ok(body.workflow_runs)
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolledPullRequest {
    pub number: i64,
    pub state: String,
    pub title: Option<String>,
    pub html_url: Option<String>,
    pub draft: Option<bool>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub merged_at: Option<DateTime<Utc>>,
    pub user: Option<GithubActor>,
}

/// Newest page of PRs (all states) for `owner/repo`, sorted by update time.
pub async fn list_pulls_page(
    client: &GithubClient,
    owner: &str,
    repo: &str,
) -> Result<Vec<PolledPullRequest>, GithubError> {
    let path = format!("/repos/{owner}/{repo}/pulls");
    let req = client.request(Method::GET, &path).query(&[
        ("state", "all"),
        ("sort", "updated"),
        ("direction", "desc"),
        ("per_page", "50"),
    ]);
    send_json(req).await
}

/// Sends a poller request, mapping transport / non-2xx / parse failures to
/// `GithubError::Api`.
async fn send_json<T: serde::de::DeserializeOwned>(
    req: reqwest::RequestBuilder,
) -> Result<T, GithubError> {
    let resp = req.send().await.map_err(|e| GithubError::Api(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(GithubError::Api(format!("poll HTTP {}", status.as_u16())));
    }
    resp.json().await.map_err(|e| GithubError::Api(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn run_fixture() -> serde_json::Value {
        serde_json::json!({ "workflow_runs": [{
            "id": 42, "run_attempt": 2, "name": "CI",
            "display_title": "Fix the build", "status": "completed",
            "conclusion": "success", "html_url": "https://github.com/o/r/actions/runs/42",
            "head_branch": "main",
            "created_at": "2026-06-01T10:00:00Z", "updated_at": "2026-06-01T10:05:00Z",
            "actor": { "login": "octocat" }
        }]})
    }

    #[tokio::test]
    async fn parses_workflow_runs_page() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/actions/runs"))
            .and(query_param("status", "completed"))
            .and(query_param("per_page", "50"))
            .respond_with(ResponseTemplate::new(200).set_body_json(run_fixture()))
            .mount(&server)
            .await;
        let client = GithubClient::with_base("t", server.uri());
        let runs = list_workflow_runs_page(&client, "o", "r").await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, 42);
        assert_eq!(runs[0].run_attempt, 2);
        assert_eq!(runs[0].conclusion.as_deref(), Some("success"));
    }

    #[tokio::test]
    async fn parses_pulls_page_and_maps_errors() {
        let server = MockServer::start().await;
        let pulls = serde_json::json!([{
            "number": 7, "state": "closed", "title": "Add feature",
            "html_url": "https://github.com/o/r/pull/7", "draft": false,
            "created_at": "2026-06-01T09:00:00Z", "updated_at": "2026-06-02T09:00:00Z",
            "closed_at": "2026-06-02T09:00:00Z", "merged_at": "2026-06-02T09:00:00Z",
            "user": { "login": "octocat" }
        }]);
        Mock::given(method("GET"))
            .and(path("/repos/o/r/pulls"))
            .and(query_param("state", "all"))
            .and(query_param("sort", "updated"))
            .and(query_param("direction", "desc"))
            .and(query_param("per_page", "50"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pulls))
            .mount(&server)
            .await;
        let client = GithubClient::with_base("t", server.uri());
        let prs = list_pulls_page(&client, "o", "r").await.unwrap();
        assert_eq!(prs[0].number, 7);
        assert!(prs[0].merged_at.is_some());

        let bad = GithubClient::with_base("t", server.uri());
        let err = list_workflow_runs_page(&bad, "missing", "repo").await;
        assert!(err.is_err()); // 404 from unmatched mock
    }
}
