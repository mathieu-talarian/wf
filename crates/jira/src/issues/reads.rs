//! Read-side lookups: projects, issue types, boards/sprints, transitions,
//! assignable users, and create/edit field metadata (port of the read half of
//! `action-runners.ts`). Agile (board/sprint) calls may fail on non-Software
//! sites; the caller surfaces that as a typed data error.

use serde::Deserialize;
use serde_json::Value;

use super::enc;
use super::mappers::{
    map_board, map_field_descriptor, map_issue_summary, map_issue_type, map_project,
    map_transition, map_user, RawBoard, RawFieldMeta, RawIssue, RawIssueType, RawProject,
    RawTransition, RawUser,
};
use crate::client::JiraClient;
use crate::errors::JiraApiError;
use crate::types::{
    JiraBoard, JiraCreateMeta, JiraEditMeta, JiraFieldDescriptor, JiraIssuePage, JiraIssueType,
    JiraProject, JiraTransition, JiraUser, SUMMARY_FIELDS,
};

#[derive(Deserialize)]
struct ValuesList<T> {
    values: Option<Vec<T>>,
}

pub async fn list_projects(client: &JiraClient) -> Result<Vec<JiraProject>, JiraApiError> {
    let res: ValuesList<RawProject> = client
        .get("/rest/api/3/project/search", &[
            ("maxResults", "100".to_string()),
            ("orderBy", "name".to_string()),
        ])
        .await?;
    Ok(res.values.unwrap_or_default().iter().map(map_project).collect())
}

#[derive(Deserialize)]
struct IssueTypesResponse {
    #[serde(rename = "issueTypes")]
    issue_types: Option<Vec<RawIssueType>>,
}

pub async fn list_issue_types(
    client: &JiraClient,
    project_key: &str,
) -> Result<Vec<JiraIssueType>, JiraApiError> {
    let path = format!("/rest/api/3/issue/createmeta/{}/issuetypes", enc(project_key));
    let res: IssueTypesResponse = client.get(&path, &[]).await?;
    Ok(res.issue_types.unwrap_or_default().iter().map(map_issue_type).collect())
}

pub async fn list_boards(client: &JiraClient) -> Result<Vec<JiraBoard>, JiraApiError> {
    let res: ValuesList<RawBoard> =
        client.get("/rest/agile/1.0/board", &[("maxResults", "50".to_string())]).await?;
    Ok(res.values.unwrap_or_default().iter().map(map_board).collect())
}

#[derive(Deserialize)]
struct SprintId {
    id: Option<i64>,
}

#[derive(Deserialize)]
struct SprintIssuesResponse {
    issues: Option<Vec<RawIssue>>,
}

/// Issues of a board's active sprint (port of `runSprintIssues`).
pub async fn sprint_issues(
    client: &JiraClient,
    board_id: i64,
) -> Result<JiraIssuePage, JiraApiError> {
    let sprints_path = format!("/rest/agile/1.0/board/{}/sprint", enc(&board_id.to_string()));
    let sprints: ValuesList<SprintId> =
        client.get(&sprints_path, &[("state", "active".to_string())]).await?;
    let Some(sprint_id) = sprints.values.and_then(|v| v.into_iter().next()).and_then(|s| s.id) else {
        return Ok(JiraIssuePage { issues: vec![], next_cursor: None, is_last: true });
    };
    let issues_path = format!("/rest/agile/1.0/sprint/{}/issue", enc(&sprint_id.to_string()));
    let res: SprintIssuesResponse = client
        .get(&issues_path, &[
            ("fields", SUMMARY_FIELDS.join(",")),
            ("maxResults", "50".to_string()),
        ])
        .await?;
    let issues = res
        .issues
        .unwrap_or_default()
        .iter()
        .map(|i| map_issue_summary(client.site_url(), i))
        .collect();
    Ok(JiraIssuePage { issues, next_cursor: None, is_last: true })
}

#[derive(Deserialize)]
struct TransitionsResponse {
    transitions: Option<Vec<RawTransition>>,
}

pub async fn list_transitions(
    client: &JiraClient,
    key: &str,
) -> Result<Vec<JiraTransition>, JiraApiError> {
    let path = format!("/rest/api/3/issue/{}/transitions", enc(key));
    let res: TransitionsResponse = client.get(&path, &[]).await?;
    Ok(res.transitions.unwrap_or_default().iter().map(map_transition).collect())
}

#[derive(Debug, Clone)]
pub struct AssignableQuery {
    pub query: String,
    pub issue_key: Option<String>,
    pub project_key_or_id: Option<String>,
    pub action_descriptor_id: Option<String>,
}

pub async fn search_users(
    client: &JiraClient,
    input: &AssignableQuery,
) -> Result<Vec<JiraUser>, JiraApiError> {
    let mut params: Vec<(&str, String)> = vec![("query", input.query.clone())];
    if let Some(k) = &input.issue_key {
        params.push(("issueKey", k.clone()));
    }
    if let Some(p) = &input.project_key_or_id {
        params.push(("project", p.clone()));
    }
    if let Some(a) = &input.action_descriptor_id {
        params.push(("actionDescriptorId", a.clone()));
    }
    let res: Vec<RawUser> = client.get("/rest/api/3/user/assignable/search", &params).await?;
    Ok(res.iter().filter_map(|u| map_user(Some(u))).collect())
}

#[derive(Deserialize)]
struct RawCreateField {
    #[serde(rename = "fieldId")]
    field_id: Option<String>,
    #[serde(flatten)]
    meta: RawFieldMeta,
}

#[derive(Deserialize)]
struct CreateFieldsResponse {
    fields: Option<Vec<RawCreateField>>,
}

async fn fetch_create_fields(
    client: &JiraClient,
    project_key: &str,
    issue_type_id: &str,
) -> Result<Vec<JiraFieldDescriptor>, JiraApiError> {
    let path = format!(
        "/rest/api/3/issue/createmeta/{}/issuetypes/{}",
        enc(project_key),
        enc(issue_type_id)
    );
    let res: CreateFieldsResponse = client.get(&path, &[]).await?;
    Ok(res
        .fields
        .unwrap_or_default()
        .iter()
        .map(|f| map_field_descriptor(f.field_id.as_deref().unwrap_or(""), &f.meta))
        .collect())
}

pub async fn create_meta(
    client: &JiraClient,
    project_key: &str,
    issue_type_id: &str,
) -> Result<JiraCreateMeta, JiraApiError> {
    let fields = fetch_create_fields(client, project_key, issue_type_id).await?;
    Ok(JiraCreateMeta { issue_type_id: issue_type_id.to_string(), fields })
}

#[derive(Deserialize)]
struct EditFieldsResponse {
    fields: Option<serde_json::Map<String, Value>>,
}

async fn fetch_edit_fields(
    client: &JiraClient,
    key: &str,
) -> Result<Vec<JiraFieldDescriptor>, JiraApiError> {
    let path = format!("/rest/api/3/issue/{}/editmeta", enc(key));
    let res: EditFieldsResponse = client.get(&path, &[]).await?;
    let mut out = Vec::new();
    for (id, value) in res.fields.unwrap_or_default() {
        if let Ok(meta) = serde_json::from_value::<RawFieldMeta>(value) {
            out.push(map_field_descriptor(&id, &meta));
        }
    }
    Ok(out)
}

pub async fn edit_meta(client: &JiraClient, key: &str) -> Result<JiraEditMeta, JiraApiError> {
    Ok(JiraEditMeta { fields: fetch_edit_fields(client, key).await? })
}
