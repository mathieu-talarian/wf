//! Issue search via Jira Cloud's enhanced search (POST /rest/api/3/search/jql),
//! cursor-paged (nextPageToken), plus the dedicated approximate-count endpoint
//! and single-issue detail (port of `issues/issues.ts` + `runIssue`).

use serde::Deserialize;
use serde_json::json;

use super::enc;
use super::mappers::{map_issue_detail, map_issue_summary, RawIssue};
use crate::client::JiraClient;
use crate::errors::JiraApiError;
use crate::types::{JiraIssueDetail, JiraIssuePage, DETAIL_FIELDS};

const DEFAULT_PAGE_SIZE: i64 = 25;

#[derive(Deserialize)]
struct SearchResponse {
    issues: Option<Vec<RawIssue>>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    #[serde(rename = "isLast")]
    is_last: Option<bool>,
}

pub async fn fetch_issue_page(
    client: &JiraClient,
    jql: &str,
    fields: &[&str],
    cursor: Option<&str>,
) -> Result<JiraIssuePage, JiraApiError> {
    let mut body = json!({
        "jql": jql,
        "maxResults": DEFAULT_PAGE_SIZE,
        "fields": fields,
    });
    if let Some(cursor) = cursor {
        body["nextPageToken"] = json!(cursor);
    }
    let res: SearchResponse = client.post("/rest/api/3/search/jql", &body).await?;
    let issues = res
        .issues
        .unwrap_or_default()
        .iter()
        .map(|i| map_issue_summary(client.site_url(), i))
        .collect();
    let is_last = res.is_last.unwrap_or_else(|| res.next_page_token.is_none());
    Ok(JiraIssuePage { issues, next_cursor: res.next_page_token, is_last })
}

#[derive(Deserialize)]
struct CountResponse {
    count: Option<i64>,
}

pub async fn approximate_count(client: &JiraClient, jql: &str) -> Result<Option<i64>, JiraApiError> {
    let res: CountResponse =
        client.post("/rest/api/3/search/approximate-count", &json!({ "jql": jql })).await?;
    Ok(res.count)
}

/// Single issue with detail fields (port of `runIssue`).
pub async fn fetch_issue_detail(
    client: &JiraClient,
    key: &str,
) -> Result<JiraIssueDetail, JiraApiError> {
    let path = format!("/rest/api/3/issue/{}", enc(key));
    let raw: RawIssue = client.get(&path, &[("fields", DETAIL_FIELDS.join(","))]).await?;
    Ok(map_issue_detail(client.site_url(), &raw))
}
