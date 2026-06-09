//! Activity (Repositories hub) read DTOs: branch→PR prompts, runnable
//! workflows, and `workflow_dispatch` inputs (port of `activity/types.ts`, read
//! half). Write DTOs (create/merge PR results) land with 3d.2.
//! Serialize camelCase to match the API contract.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubBranchPrompt {
    pub name: String,
    pub last_commit_date: String,
    pub last_commit_message: String,
    pub compare_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepoBranches {
    pub repo_full_name: String,
    pub repo_url: String,
    pub default_branch: String,
    pub branches: Vec<GithubBranchPrompt>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubWorkflowSummary {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub state: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepoWorkflows {
    pub repo_full_name: String,
    pub repo_url: String,
    pub default_branch: String,
    pub workflows: Vec<GithubWorkflowSummary>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum GithubWorkflowInputType {
    String,
    Boolean,
    Choice,
    Number,
    Environment,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubWorkflowInput {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
    pub r#type: GithubWorkflowInputType,
    pub default: Option<String>,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubWorkflowInputs {
    pub dispatchable: bool,
    pub inputs: Vec<GithubWorkflowInput>,
}

// --- Write DTOs (port of the write half of `activity/types.ts`) ---

/// Body for creating a PR (port of `GithubCreatePullInputT`).
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct GithubCreatePullInput {
    pub base: String,
    pub head: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubCreatePullResult {
    pub number: i64,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum GithubMergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl GithubMergeMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubMergePullResult {
    pub merged: bool,
    pub message: String,
}
