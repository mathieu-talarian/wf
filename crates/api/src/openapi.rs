//! OpenAPI document aggregation (Phase 5a, migration plan §5a). Collects every
//! `#[utoipa::path]` handler and `ToSchema` DTO into a single document, adds the
//! bearer (Supabase JWT) security scheme, and serves it at `/api/openapi.json`.

use actix_web::{web, HttpResponse};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Workflow API",
        version = "0.1.0",
        description = "Rust port of the Workflow backend (GitHub + Jira integrations)."
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "system", description = "Liveness and info"),
        (name = "me", description = "Authenticated user"),
        (name = "github", description = "GitHub integration"),
        (name = "jira", description = "Jira integration")
    ),
    paths(
        crate::routes::health::health,
        crate::routes::health::hello,
        crate::routes::me::me,
        // GitHub
        crate::github::routes::status,
        crate::github::routes::connect,
        crate::github::routes::validate,
        crate::github::routes::disconnect,
        crate::github::routes::dashboard_route,
        crate::github::routes::queue_route,
        crate::github::routes::repos_route,
        crate::github::routes::set_repos_route,
        crate::github::routes::pull_route,
        crate::github::routes::pulls_route,
        crate::github::routes::branches_route,
        crate::github::routes::workflows_route,
        crate::github::routes::workflow_inputs_route,
        crate::github::routes::workflow_runs_route,
        crate::github::routes::repo_branches_route,
        crate::github::routes::environments_route,
        crate::github::routes::dispatch_route,
        crate::github::routes::create_pull_route,
        crate::github::routes::merge_pull_route,
        crate::github::routes::close_pull_route,
        crate::github::routes::favorites_route,
        crate::github::routes::set_favorites_route,
        // Jira
        crate::jira::routes::status,
        crate::jira::routes::connect,
        crate::jira::routes::validate,
        crate::jira::routes::disconnect,
        crate::jira::routes::set_projects,
        crate::jira::routes::dashboard,
        crate::jira::routes::queue,
        crate::jira::routes::search,
        crate::jira::routes::issue,
        crate::jira::routes::projects,
        crate::jira::routes::issue_types,
        crate::jira::routes::boards,
        crate::jira::routes::sprint,
        crate::jira::routes::transitions,
        crate::jira::routes::users,
        crate::jira::routes::create_meta,
        crate::jira::routes::edit_meta,
        crate::jira::routes::transition,
        crate::jira::routes::comment,
        crate::jira::routes::assign,
        crate::jira::routes::worklog,
        crate::jira::routes::create_issue,
        crate::jira::routes::edit_issue,
    ),
    components(schemas(
        // wf-api response + body DTOs
        crate::routes::me::MeResponse,
        crate::routes::health::HealthResponse,
        crate::routes::health::HelloResponse,
        crate::dto::OkResponse,
        crate::dto::DisconnectedResponse,
        crate::github::summary::GithubConnectionSummary,
        crate::github::dashboard::RepoSelection,
        crate::jira::summary::JiraConnectionSummary,
        crate::github::routes::TokenBody,
        crate::github::routes::ReposBody,
        crate::github::routes::PullsBody,
        crate::github::routes::DispatchBody,
        crate::github::routes::CreatePullBody,
        crate::github::routes::MergePullBody,
        crate::github::routes::ClosePullBody,
        crate::github::routes::SetFavoritesBody,
        crate::jira::routes::ConnectBody,
        crate::jira::routes::ProjectsBody,
        crate::jira::routes::SearchBody,
        crate::jira::routes::TransitionBody,
        crate::jira::routes::CommentBody,
        crate::jira::routes::AssignBody,
        crate::jira::routes::WorklogBody,
        crate::jira::routes::EditBody,
        // wf-github DTOs
        wf_github::GithubQueueKey,
        wf_github::dashboard::types::GithubDashboardActor,
        wf_github::dashboard::types::GithubDashboardLabel,
        wf_github::dashboard::types::GithubDashboardRepository,
        wf_github::dashboard::types::GithubPullRequestBasic,
        wf_github::dashboard::types::GithubQueueCount,
        wf_github::dashboard::types::GithubPullRequestQueue,
        wf_github::dashboard::types::DashboardData,
        wf_github::GithubAccountSummary,
        wf_github::GithubRepoOption,
        wf_github::GithubDashboard,
        wf_github::GithubApprovalState,
        wf_github::GithubCheckState,
        wf_github::dashboard::types::GithubRequestedReviewerKind,
        wf_github::GithubWorkflowRunSummary,
        wf_github::GithubRequestedReviewer,
        wf_github::GithubRequiredCheck,
        wf_github::GithubPullRequestEnrichment,
        wf_github::GithubPullRef,
        wf_github::GithubPullEnrichmentResult,
        wf_github::GithubBranchPrompt,
        wf_github::GithubRepoBranches,
        wf_github::GithubWorkflowSummary,
        wf_github::GithubRepoWorkflows,
        wf_github::GithubWorkflowInputType,
        wf_github::GithubWorkflowInput,
        wf_github::GithubWorkflowInputs,
        wf_github::GithubCreatePullInput,
        wf_github::GithubCreatePullResult,
        wf_github::GithubMergeMethod,
        wf_github::GithubMergePullResult,
        // wf-jira DTOs
        wf_jira::JiraQueueKey,
        wf_jira::QueueErrorKind,
        wf_jira::JiraValidationStatus,
        wf_jira::JiraUser,
        wf_jira::JiraStatus,
        wf_jira::JiraNamedIcon,
        wf_jira::JiraIssueSummary,
        wf_jira::JiraComment,
        wf_jira::JiraTransition,
        wf_jira::JiraProject,
        wf_jira::JiraIssueDetail,
        wf_jira::JiraIssuePage,
        wf_jira::JiraQueueResult,
        wf_jira::JiraAccountSummary,
        wf_jira::JiraDashboard,
        wf_jira::JiraIssueType,
        wf_jira::JiraDescriptorSchema,
        wf_jira::JiraAllowedRef,
        wf_jira::JiraFieldDescriptor,
        wf_jira::JiraCreateMeta,
        wf_jira::JiraEditMeta,
        wf_jira::JiraBoard,
        wf_jira::JiraCreateIssueResult,
        wf_jira::JiraWorklogInput,
        wf_jira::JiraCreateIssueInput,
    ))
)]
pub struct ApiDoc;

/// `GET /api/openapi.json` — the generated spec.
async fn openapi_json() -> HttpResponse {
    HttpResponse::Ok().json(ApiDoc::openapi())
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/openapi.json", web::get().to(openapi_json));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_complete() {
        let doc = ApiDoc::openapi();

        // 41 unique path keys (48 operations; some paths carry multiple methods).
        assert_eq!(doc.paths.paths.len(), 41, "unexpected path count");

        // Bearer security scheme is registered.
        let components = doc.components.as_ref().expect("components");
        assert!(components.security_schemes.contains_key("bearer"));

        // A representative endpoint from each area is present.
        assert!(doc.paths.paths.contains_key("/api/me"));
        assert!(doc.paths.paths.contains_key("/api/me/github/dashboard"));
        assert!(doc.paths.paths.contains_key("/api/me/jira/issue"));

        // Serializes to JSON.
        let json = serde_json::to_string(&doc).expect("serialize");
        assert!(json.contains("\"openapi\""));
        assert!(json.contains("GithubDashboard"));
        assert!(json.contains("JiraIssueDetail"));
    }

    /// CI spec-drift check (Phase 5): the generated spec must match the
    /// committed `openapi.json` (the artifact the web client is generated from).
    /// Refresh with `UPDATE_OPENAPI=1 cargo test -p wf-api openapi_spec_committed`.
    #[test]
    fn openapi_spec_committed() {
        let generated = serde_json::to_value(ApiDoc::openapi()).expect("to value");
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.json");

        if std::env::var("UPDATE_OPENAPI").is_ok() {
            let pretty = serde_json::to_string_pretty(&generated).expect("pretty");
            std::fs::write(path, pretty + "\n").expect("write openapi.json");
            return;
        }

        let committed: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path)
                .expect("openapi.json missing — run UPDATE_OPENAPI=1 cargo test"),
        )
        .expect("parse committed openapi.json");

        assert_eq!(
            generated, committed,
            "OpenAPI spec drifted from openapi.json — run `UPDATE_OPENAPI=1 cargo test -p wf-api openapi_spec_committed` to refresh"
        );
    }
}
