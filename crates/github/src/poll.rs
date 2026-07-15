//! Poller-specific repo reads (A1 spec §5): page-1, newest-first by update
//! recency, raw provider ids/timestamps. Deliberately separate from the
//! dashboard queue reads (user-queue shaped, open-PR-only). Cursor filtering
//! happens in `wf-sync`; these fetch one page.
//!
//! Documented limitation (spec §6.2 deviation, plan header): >50 updates per
//! scope per tick lose the older ones; dedup keeps replays safe.

use std::error::Error;

use chrono::{DateTime, Utc};
use reqwest::Method;
use serde::{Deserialize, Deserializer};

use crate::client::GithubClient;
use crate::errors::GithubError;

#[derive(Debug, Clone, Deserialize)]
pub struct GithubActor {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolledWorkflowRun {
    pub id: i64,
    #[serde(default)]
    pub workflow_id: Option<i64>,
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

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
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
    #[serde(default, deserialize_with = "null_default")]
    pub assignees: Vec<GithubActor>,
    #[serde(default, deserialize_with = "null_default")]
    pub requested_reviewers: Vec<GithubActor>,
    #[serde(default)]
    pub head: Option<PolledPullHead>,
}

/// `head` projection of the REST PR listing (branch name only).
#[derive(Debug, Clone, Deserialize)]
pub struct PolledPullHead {
    #[serde(rename = "ref")]
    pub ref_name: String,
}

/// Newest workflow runs for `owner/repo` regardless of status (running
/// included) — feeds the hub actions strip.
pub async fn list_runs_any_status(
    client: &GithubClient,
    owner: &str,
    repo: &str,
) -> Result<Vec<PolledWorkflowRun>, GithubError> {
    let path = format!("/repos/{owner}/{repo}/actions/runs");
    let req = client.request(Method::GET, &path).query(&[("per_page", "30")]);
    let body: RunsResponse = send_json(req).await?;
    Ok(body.workflow_runs)
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
    req: reqwest_middleware::RequestBuilder,
) -> Result<T, GithubError> {
    let decode_retry = req.try_clone();
    let resp = send_checked(req).await?;
    match resp.json().await {
        Ok(body) => Ok(body),
        Err(first) => retry_decode(decode_retry, first).await,
    }
}

async fn send_checked(
    req: reqwest_middleware::RequestBuilder,
) -> Result<reqwest::Response, GithubError> {
    let resp = send_get_retry(req)
        .await
        .map_err(|e| GithubError::Api(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(GithubError::Api(format!("poll HTTP {}", status.as_u16())));
    }
    Ok(resp)
}

async fn retry_decode<T: serde::de::DeserializeOwned>(
    retry: Option<reqwest_middleware::RequestBuilder>,
    first: reqwest::Error,
) -> Result<T, GithubError> {
    let Some(retry) = retry else {
        return Err(decode_error(first));
    };
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    send_checked(retry)
        .await?
        .json()
        .await
        .map_err(decode_error)
}

fn decode_error(error: reqwest::Error) -> GithubError {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    GithubError::Api(message)
}

/// Transient statuses worth one retry (rate-limit / upstream blip).
fn is_transient(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

/// Retry delay: honor `Retry-After` seconds (capped), else a short fixed pause.
fn retry_delay(resp: Option<&reqwest::Response>) -> std::time::Duration {
    resp.and_then(|r| r.headers().get(reqwest::header::RETRY_AFTER))
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|s| std::time::Duration::from_secs(s).min(std::time::Duration::from_secs(15)))
        .unwrap_or(std::time::Duration::from_millis(500))
}

/// One retry for a poller GET (all idempotent) on transport errors or transient
/// statuses. ponytail: 1 retry, 15s cap — past that the scope backoff takes over.
async fn send_get_retry(req: reqwest_middleware::RequestBuilder) -> reqwest_middleware::Result<reqwest::Response> {
    let retry = req.try_clone();
    let resp = req.send().await;
    let needs_retry = resp.as_ref().map(|r| is_transient(r.status().as_u16())).unwrap_or(true);
    match retry {
        Some(c) if needs_retry => {
            tokio::time::sleep(retry_delay(resp.as_ref().ok())).await;
            c.send().await
        }
        _ => resp,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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

    #[test]
    fn parses_null_pull_collections() {
        let pull = serde_json::json!({
            "number": 7, "state": "open", "title": "Add feature",
            "html_url": "https://github.com/o/r/pull/7", "draft": false,
            "created_at": "2026-06-01T09:00:00Z", "updated_at": "2026-06-02T09:00:00Z",
            "closed_at": null, "merged_at": null, "user": null,
            "assignees": null, "requested_reviewers": null, "head": null
        });
        let pull: PolledPullRequest = serde_json::from_value(pull).unwrap();
        assert!(pull.assignees.is_empty());
        assert!(pull.requested_reviewers.is_empty());
    }

    #[tokio::test]
    async fn retries_response_decode_errors() {
        let server = MockServer::start().await;
        let attempts = Arc::new(AtomicUsize::new(0));
        let responder_attempts = Arc::clone(&attempts);
        Mock::given(method("GET"))
            .and(path("/repos/o/r/pulls"))
            .respond_with(decode_retry_responder(responder_attempts))
            .expect(2)
            .mount(&server)
            .await;
        let client = GithubClient::with_base("t", server.uri());
        assert!(list_pulls_page(&client, "o", "r").await.unwrap().is_empty());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    fn decode_retry_responder(
        attempts: Arc<AtomicUsize>,
    ) -> impl Fn(&wiremock::Request) -> ResponseTemplate {
        move |_| {
            if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200).set_body_string("[")
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!([]))
            }
        }
    }
}
