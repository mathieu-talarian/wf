//! Dashboard response DTOs (port of `dashboard/types.ts`). Field names are
//! serialized camelCase to match the existing API contract.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
}

#[derive(Debug, Clone, Serialize)]
pub struct GithubDashboardActor {
    pub login: String,
    pub avatar_url: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GithubDashboardLabel {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubDashboardRepository {
    pub full_name: String,
    pub url: String,
    pub actions_url: String,
    pub is_private: bool,
    pub is_archived: bool,
    pub default_branch: String,
}

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubQueueCount {
    pub key: GithubQueueKey,
    pub label: String,
    pub total_count: i64,
    pub incomplete_results: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullRequestQueue {
    pub key: GithubQueueKey,
    pub label: String,
    pub total_count: i64,
    pub incomplete_results: bool,
    pub pull_requests: Vec<GithubPullRequestBasic>,
}

/// `fetchDashboard` result: counts for every queue + nodes for the active one.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardData {
    pub queues: Vec<GithubQueueCount>,
    pub queue_pulls: Vec<GithubPullRequestQueue>,
}
