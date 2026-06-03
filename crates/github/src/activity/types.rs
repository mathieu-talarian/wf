//! Activity (Repositories hub) read DTOs: branch→PR prompts, runnable
//! workflows, and `workflow_dispatch` inputs (port of `activity/types.ts`, read
//! half). Write DTOs (create/merge PR results) land with 3d.2.
//! Serialize camelCase to match the API contract.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubBranchPrompt {
    pub name: String,
    pub last_commit_date: String,
    pub last_commit_message: String,
    pub compare_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepoBranches {
    pub repo_full_name: String,
    pub repo_url: String,
    pub default_branch: String,
    pub branches: Vec<GithubBranchPrompt>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubWorkflowSummary {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub state: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepoWorkflows {
    pub repo_full_name: String,
    pub repo_url: String,
    pub default_branch: String,
    pub workflows: Vec<GithubWorkflowSummary>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GithubWorkflowInputType {
    String,
    Boolean,
    Choice,
    Number,
    Environment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubWorkflowInput {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
    pub r#type: GithubWorkflowInputType,
    pub default: Option<String>,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubWorkflowInputs {
    pub dispatchable: bool,
    pub inputs: Vec<GithubWorkflowInput>,
}
