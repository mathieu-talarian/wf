//! Shared Jira DTOs (port of `jira/types.ts`). Mapping logic lives in
//! `issues/mappers.rs`. Serialize camelCase to match the API contract.

use serde::{Deserialize, Serialize};

use crate::issues::jql::JiraQueueKey;
use crate::status::QueueErrorKind;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraUser {
    pub account_id: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub email_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JiraStatus {
    pub name: String,
    pub category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraNamedIcon {
    pub name: String,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraIssueSummary {
    pub key: String,
    pub summary: String,
    pub status: JiraStatus,
    pub issue_type: JiraNamedIcon,
    pub priority: Option<JiraNamedIcon>,
    pub assignee: Option<JiraUser>,
    pub project_key: String,
    pub updated: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JiraComment {
    pub id: String,
    pub author: Option<JiraUser>,
    pub body: String,
    pub created: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraTransition {
    pub id: String,
    pub name: String,
    pub to_status: String,
    pub to_category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraProject {
    pub id: String,
    pub key: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraIssueDetail {
    #[serde(flatten)]
    pub summary: JiraIssueSummary,
    pub description: String,
    pub reporter: Option<JiraUser>,
    pub labels: Vec<String>,
    pub comments: Vec<JiraComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraIssuePage {
    pub issues: Vec<JiraIssueSummary>,
    pub next_cursor: Option<String>,
    pub is_last: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraQueueResult {
    pub key: JiraQueueKey,
    pub approximate_total: Option<i64>,
    pub issues: Vec<JiraIssueSummary>,
    pub next_cursor: Option<String>,
    pub is_last: bool,
    pub error: Option<QueueErrorKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraAccountSummary {
    pub connected: bool,
    pub site_url: Option<String>,
    pub account_id: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraDashboard {
    pub account: JiraAccountSummary,
    pub queues: Vec<JiraQueueResult>,
    pub selected_projects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraIssueType {
    pub id: String,
    pub name: String,
    pub icon_url: Option<String>,
    pub subtask: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JiraDescriptorSchema {
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JiraAllowedRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraFieldDescriptor {
    pub field_id: String,
    pub name: String,
    pub required: bool,
    pub schema: JiraDescriptorSchema,
    pub allowed_values: Vec<JiraAllowedRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraCreateMeta {
    pub issue_type_id: String,
    pub fields: Vec<JiraFieldDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JiraEditMeta {
    pub fields: Vec<JiraFieldDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JiraBoard {
    pub id: i64,
    pub name: String,
    pub r#type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JiraCreateIssueResult {
    pub id: String,
    pub key: String,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraWorklogInput {
    pub time_spent: String,
    pub started: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraCreateIssueInput {
    pub project_key: String,
    pub issue_type_id: String,
    pub fields: serde_json::Map<String, serde_json::Value>,
}

pub const SUMMARY_FIELDS: &[&str] =
    &["summary", "status", "assignee", "priority", "issuetype", "updated", "project"];

pub const DETAIL_FIELDS: &[&str] = &[
    "summary", "status", "assignee", "priority", "issuetype", "updated", "project", "description",
    "reporter", "labels", "comment",
];
