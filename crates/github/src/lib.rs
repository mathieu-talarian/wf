//! GitHub integration: raw `reqwest` REST client, the PAT validation state
//! machine, and (Phase 3 cont.) the GraphQL dashboard query + activity calls.
//! Migration plan §10.1.

pub mod client;
pub mod dashboard;
pub mod errors;
pub mod types;
pub mod validate;

pub use client::{parse_repo_ref, GithubClient, RepoRef};
pub use dashboard::enrich::{enrich_pull_request, enrich_pull_requests};
pub use dashboard::types::{
    GithubAccountSummary, GithubApprovalState, GithubCheckState, GithubDashboard,
    GithubPullEnrichmentResult, GithubPullRef, GithubPullRequestEnrichment, GithubQueueKey,
    GithubRepoOption, GithubRequestedReviewer, GithubRequiredCheck, GithubWorkflowRunSummary,
};
pub use dashboard::{fetch_dashboard, fetch_queue_pulls, list_repositories};
pub use errors::{GithubError, PatValidationError};
pub use types::{PatTokenKind, PatValidationResult, PatValidationStatus};
pub use validate::validate_token;
