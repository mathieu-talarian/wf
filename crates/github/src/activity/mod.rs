//! GitHub Activity / Repositories-hub reads (port of `github/activity/*`,
//! `branches*.ts`, `workflows/*`, read half). Write paths (dispatch, create/
//! merge/close PR) land with 3d.2.

pub mod branches;
pub mod branches_graphql;
pub mod environments;
pub mod inputs;
pub mod types;
pub mod workflows;

pub use branches::{fetch_branch_prompts, list_repo_branch_names};
pub use environments::list_environments;
pub use inputs::{fetch_workflow_inputs, parse_workflow_inputs};
pub use types::{
    GithubBranchPrompt, GithubRepoBranches, GithubRepoWorkflows, GithubWorkflowInput,
    GithubWorkflowInputType, GithubWorkflowInputs, GithubWorkflowSummary,
};
pub use workflows::{fetch_workflows, list_workflow_runs};
