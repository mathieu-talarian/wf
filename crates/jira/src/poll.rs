//! Poller-specific issue read (A1 spec §5): newest page by `updated` for one
//! project, with the raw status id / timestamps the normalizer needs.
//! Cursor filtering happens in `wf-sync`. Relative-ordering DESC (not the
//! spec's ASC sketch): absolute JQL timestamps are interpreted in the
//! account's timezone — a correctness trap; see plan header.

use serde::Deserialize;
use serde_json::json;

use crate::client::JiraClient;
use crate::errors::JiraApiError;
use crate::issues::jql::quote_jql_string;

/// A single issue row as returned by the poller (raw provider strings).
#[derive(Debug, Clone)]
pub struct PolledIssue {
    pub key: String,
    pub summary: Option<String>,
    pub status_id: Option<String>,
    pub status_name: Option<String>,
    pub status_category: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub url: String,
}

#[derive(Deserialize)]
struct PollSearchResponse {
    issues: Option<Vec<RawPollIssue>>,
}

#[derive(Deserialize)]
struct RawPollIssue {
    key: String,
    fields: RawPollFields,
}

#[derive(Deserialize)]
struct RawPollFields {
    summary: Option<String>,
    status: Option<RawStatus>,
    created: Option<String>,
    updated: Option<String>,
}

#[derive(Deserialize)]
struct RawStatus {
    id: Option<String>,
    name: Option<String>,
    #[serde(rename = "statusCategory")]
    status_category: Option<RawCategory>,
}

#[derive(Deserialize)]
struct RawCategory {
    key: Option<String>,
}

/// JQL for the newest page of a project's issues by update recency.
pub fn recent_issues_jql(project_key: &str) -> String {
    format!("project = {} ORDER BY updated DESC", quote_jql_string(project_key))
}

/// Newest page (≤50) of issues for `project_key` with poller fields.
pub async fn fetch_recent_issues(
    client: &JiraClient,
    project_key: &str,
) -> Result<Vec<PolledIssue>, JiraApiError> {
    let body = json!({
        "jql": recent_issues_jql(project_key),
        "maxResults": 50,
        "fields": ["summary", "status", "created", "updated"],
    });
    let res: PollSearchResponse = client.post("/rest/api/3/search/jql", &body).await?;
    let site = client.site_url().to_string();
    Ok(res.issues.unwrap_or_default().into_iter().map(|r| to_polled(&site, r)).collect())
}

fn to_polled(site_url: &str, raw: RawPollIssue) -> PolledIssue {
    let status = raw.fields.status;
    PolledIssue {
        // site_url is a normalized origin (no trailing slash) — see site_url.rs
        url: format!("{site_url}/browse/{}", raw.key),
        key: raw.key,
        summary: raw.fields.summary,
        status_id: status.as_ref().and_then(|s| s.id.clone()),
        status_name: status.as_ref().and_then(|s| s.name.clone()),
        status_category: status
            .as_ref()
            .and_then(|s| s.status_category.as_ref())
            .and_then(|c| c.key.clone()),
        created: raw.fields.created,
        updated: raw.fields.updated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::JiraCreds;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn jql_quotes_project_key() {
        assert_eq!(recent_issues_jql("PROJ"), "project = \"PROJ\" ORDER BY updated DESC");
    }

    #[tokio::test]
    async fn parses_poll_page() {
        let fixture = serde_json::json!({ "issues": [{"key": "PROJ-1", "fields": {"summary": "Fix login", "status": { "id": "3", "name": "In Progress", "statusCategory": { "key": "indeterminate" }}, "created": "2026-06-01T10:00:00.000+0200", "updated": "2026-06-02T11:00:00.000+0200"}}]});
        let expected = serde_json::json!({"jql": "project = \"PROJ\" ORDER BY updated DESC", "maxResults": 50, "fields": ["summary", "status", "created", "updated"]});
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/rest/api/3/search/jql"))
            .and(body_json(&expected))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture))
            .mount(&server)
            .await;
        let creds = JiraCreds { site_url: server.uri(), email: "e@x.com".into(), token: "t".into() };
        let issues = fetch_recent_issues(&JiraClient::new(&creds), "PROJ").await.unwrap();
        assert_eq!(issues[0].key, "PROJ-1");
        assert_eq!(issues[0].status_id.as_deref(), Some("3"));
        assert!(issues[0].url.ends_with("/browse/PROJ-1"));
    }
}
