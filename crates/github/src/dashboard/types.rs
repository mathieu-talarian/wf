//! Dashboard response DTOs (port of `dashboard/types.ts` + `account.ts`). Field
//! names serialize camelCase to match the existing API contract. All derive
//! `Deserialize` too, since the full dashboard is persisted as a jsonb snapshot
//! (`dashboard_snapshot`) and read back on a cold start.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GithubQueueKey {
    Assigned,
    ReviewRequested,
    Authored,
    Mentioned,
    FailingCi,
}

impl GithubQueueKey {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "assigned" => Self::Assigned,
            "review_requested" => Self::ReviewRequested,
            "authored" => Self::Authored,
            "mentioned" => Self::Mentioned,
            "failing_ci" => Self::FailingCi,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assigned => "assigned",
            Self::ReviewRequested => "review_requested",
            Self::Authored => "authored",
            Self::Mentioned => "mentioned",
            Self::FailingCi => "failing_ci",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GithubDashboardActor {
    pub login: String,
    pub avatar_url: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GithubDashboardLabel {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubDashboardRepository {
    pub full_name: String,
    pub url: String,
    pub actions_url: String,
    pub is_private: bool,
    pub is_archived: bool,
    pub default_branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullRequestBasic {
    pub repository: GithubDashboardRepository,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub author: GithubDashboardActor,
    pub assignees: Vec<GithubDashboardActor>,
    pub labels: Vec<GithubDashboardLabel>,
    pub comments: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubQueueCount {
    pub key: GithubQueueKey,
    pub label: String,
    pub total_count: i64,
    pub incomplete_results: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullRequestQueue {
    pub key: GithubQueueKey,
    pub label: String,
    pub total_count: i64,
    pub incomplete_results: bool,
    pub pull_requests: Vec<GithubPullRequestBasic>,
}

/// `fetchDashboard` result: counts for every queue + nodes for the active one.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardData {
    pub queues: Vec<GithubQueueCount>,
    pub queue_pulls: Vec<GithubPullRequestQueue>,
}

/// Account header on the dashboard response (port of `GithubAccountSummaryT`).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubAccountSummary {
    pub connected: bool,
    pub login: Option<String>,
    pub scope: Option<String>,
    pub connected_at: Option<String>,
}

impl GithubAccountSummary {
    pub fn disconnected() -> Self {
        Self { connected: false, login: None, scope: None, connected_at: None }
    }
}

/// One selectable repository (port of `GithubRepoOptionT`).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepoOption {
    pub full_name: String,
    pub is_private: bool,
    pub is_archived: bool,
}

/// The full dashboard response (port of `GithubDashboardT`). Persisted as the
/// `dashboard_snapshot` jsonb for cold-start SWR.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubDashboard {
    pub account: GithubAccountSummary,
    pub queues: Vec<GithubQueueCount>,
    pub queue_pulls: Vec<GithubPullRequestQueue>,
}

impl GithubDashboard {
    pub fn empty() -> Self {
        Self {
            account: GithubAccountSummary::disconnected(),
            queues: vec![],
            queue_pulls: vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// PR enrichment DTOs (port of the detail-derived half of `dashboard/types.ts`).
// Fetched lazily per PR via the enrichment endpoints (`/me/github/pull`,
// `/me/github/pulls/enrich`). All serialize camelCase to match the API contract.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GithubApprovalState {
    Approved,
    ChangesRequested,
    ReviewRequired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GithubCheckState {
    Success,
    Failure,
    Pending,
    Skipped,
    Neutral,
    Cancelled,
    TimedOut,
    ActionRequired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GithubRequestedReviewerKind {
    User,
    Team,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubWorkflowRunSummary {
    pub id: i64,
    pub name: Option<String>,
    pub status: String,
    pub conclusion: Option<String>,
    pub url: String,
    pub run_number: i64,
    pub event: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubRequestedReviewer {
    pub name: String,
    pub avatar_url: Option<String>,
    pub url: String,
    pub kind: GithubRequestedReviewerKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubRequiredCheck {
    pub name: String,
    pub state: GithubCheckState,
    pub url: Option<String>,
    pub required: bool,
}

/// Detail-derived fields, fetched lazily per PR (port of
/// `GithubPullRequestEnrichmentT`).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullRequestEnrichment {
    pub repository: GithubDashboardRepository,
    pub review_comments: Option<i64>,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub changed_files: Option<i64>,
    pub draft: Option<bool>,
    pub head_ref: Option<String>,
    pub base_ref: Option<String>,
    pub head_sha: Option<String>,
    pub latest_run: Option<GithubWorkflowRunSummary>,
    pub approval_state: GithubApprovalState,
    pub requested_reviewers: Vec<GithubRequestedReviewer>,
    pub mergeable: Option<bool>,
    pub mergeable_state: Option<String>,
    pub branch_behind: Option<bool>,
    pub required_checks: Vec<GithubRequiredCheck>,
    pub readiness_badges: Vec<String>,
    pub blocker_summary: Vec<String>,
}

/// One PR to enrich in a batch request (port of `GithubPullRefT`).
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct GithubPullRef {
    pub owner: String,
    pub repo: String,
    pub number: i64,
}

/// A batch enrichment entry: the ref echoed back with its enrichment (port of
/// `GithubPullEnrichmentResultT`).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullEnrichmentResult {
    pub owner: String,
    pub repo: String,
    pub number: i64,
    pub enrichment: GithubPullRequestEnrichment,
}
