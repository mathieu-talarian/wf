//! DTOs for the `hub` tag (Workflow Hub backend contract §2).

use serde::{Deserialize, Serialize};
use wf_jira::JiraUser;

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubBoard {
    pub columns: Vec<HubColumn>,
    pub orphans: HubOrphans,
    pub context: HubBoardContext,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubBoardContext {
    pub jira_project_key: Option<String>,
    pub repos: Vec<String>,
    pub generated_at: String,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubColumn {
    pub id: String,
    pub title: String,
    pub jira_statuses: Vec<String>,
    pub tickets: Vec<HubTicketCard>,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubTicketCard {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub assignee: Option<JiraUser>,
    pub updated_at: String,
    pub bug_count: i64,
    pub ready: bool,
    pub branch: Option<String>,
    pub prs: Vec<HubPrRef>,
    /// `success` | `running` | `failed` | `none` (aggregate over linked PRs)
    pub check_state: String,
    pub deployment: Option<HubDeployment>,
    pub slack_unread: i64,
    pub notes_count: i64,
    pub reminders_due: i64,
    pub favorite_workflow: Option<HubFavoriteWorkflow>,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubPrRef {
    /// `owner/name`
    pub repo: String,
    pub number: i64,
    pub title: String,
    /// `open` | `draft` | `merged` | `closed`
    pub state: String,
    /// `success` | `running` | `failed` | `none`
    pub check_state: String,
    pub branch: String,
    pub url: String,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubDeployment {
    pub environment: String,
    pub version: Option<String>,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubFavoriteWorkflow {
    pub repo: String,
    pub workflow_id: String,
    pub workflow_name: String,
    pub path: String,
    /// ISO-8601 millis of the favorite workflow's most recent run, or null.
    pub last_run: Option<String>,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubOrphans {
    pub prs: Vec<HubOrphanPr>,
    pub branches: Vec<HubOrphanBranch>,
    pub tickets: Vec<HubOrphanTicket>,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubOrphanPr {
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub branch: String,
    pub url: String,
    pub suggestion: Option<HubLinkSuggestion>,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubOrphanBranch {
    pub repo: String,
    pub name: String,
    pub suggestion: Option<HubLinkSuggestion>,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubOrphanTicket {
    pub key: String,
    pub summary: String,
    pub status: String,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubLinkSuggestion {
    pub ticket_key: String,
    pub confidence: f64,
    /// matcher that produced the suggestion — currently always `heuristic`
    pub source: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubLinkBody {
    pub ticket_key: String,
    /// `owner/name`
    pub repo: String,
    pub pr_number: Option<i64>,
    pub branch: Option<String>,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubInbox {
    pub items: Vec<HubInboxItem>,
    pub brief: Option<HubBrief>,
    /// `ai` | `heuristic`
    pub ranked_by: String,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubBrief {
    pub summary: String,
    pub generated_at: String,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubInboxItem {
    pub id: String,
    /// `qa` | `review` | `run` | `reminder`
    pub kind: String,
    pub ticket_key: Option<String>,
    pub source_label: String,
    pub title: String,
    pub occurred_at: String,
    /// Rank position as a string, ascending (`"0"` = most urgent).
    // ponytail: contract types this `string` but names no vocabulary; we
    // stringify the existing rank. Swap to labels if the UI ever needs them.
    pub urgency: String,
    pub qa: Option<HubInboxQa>,
    pub run: Option<HubInboxRun>,
    pub review: Option<HubInboxReview>,
    pub reminder: Option<crate::notes::routes::Reminder>,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubInboxQa {
    pub channel_id: String,
    pub thread_ts: String,
    pub permalink: Option<String>,
    pub unread: i64,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubInboxRun {
    pub repo: String,
    pub run_id: String,
    pub workflow_name: String,
    pub conclusion: String,
    pub url: String,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubInboxReview {
    pub repo: String,
    pub pr_number: i64,
    pub title: String,
    pub url: String,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubRuns {
    pub runs: Vec<HubRunPill>,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubRunPill {
    /// `owner/name`
    pub repo: String,
    pub workflow_id: String,
    pub workflow_name: String,
    pub run_id: String,
    /// `success` | `running` | `failed`
    pub status: String,
    pub version: Option<String>,
    pub started_at: String,
    pub url: String,
    pub is_favorite: bool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubSyncStatus {
    pub sources: Vec<HubSyncSource>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubSyncSource {
    pub source: String,
    pub state: String,
    pub as_of: Option<String>,
    pub last_error: Option<String>,
    pub scope_count: i64,
}
