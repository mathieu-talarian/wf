//! GitHub connection routes (migration plan §14.2; port of
//! `github/routes/pat.ts`). All require a valid Supabase JWT.

use actix_web::{web, HttpResponse};
use sea_orm::prelude::Uuid;
use serde::Deserialize;
use std::collections::HashMap;

use wf_github::{
    GithubCreatePullInput, GithubMergeMethod, GithubPullRef, GithubQueueKey, RepoRef,
};

use crate::auth::AuthUser;
use crate::error::AppError;
use wf_db::repositories::github_pat;

use crate::github::{activity, dashboard, pat};
use crate::state::AppState;

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct TokenBody {
    token: String,
}

#[derive(Deserialize)]
pub(crate) struct DashboardQuery {
    tab: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct QueueQuery {
    key: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct ReposBody {
    repos: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct PullQuery {
    owner: String,
    repo: String,
    number: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct PullsBody {
    refs: Vec<GithubPullRef>,
}

#[derive(Deserialize)]
pub(crate) struct RepoQuery {
    owner: String,
    repo: String,
}

#[derive(Deserialize)]
pub(crate) struct WorkflowInputsQuery {
    owner: String,
    repo: String,
    path: String,
}

#[derive(Deserialize)]
pub(crate) struct WorkflowRunsQuery {
    owner: String,
    repo: String,
    #[serde(rename = "workflowId")]
    workflow_id: String,
    branch: String,
}

fn ref_of(owner: &str, repo: &str) -> RepoRef {
    RepoRef { owner: owner.to_string(), repo: repo.to_string() }
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct DispatchBody {
    owner: String,
    repo: String,
    #[serde(rename = "workflowId")]
    workflow_id: i64,
    #[serde(rename = "ref")]
    git_ref: String,
    inputs: HashMap<String, String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct CreatePullBody {
    owner: String,
    repo: String,
    base: String,
    head: String,
    title: String,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct MergePullBody {
    owner: String,
    repo: String,
    number: i64,
    method: GithubMergeMethod,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct ClosePullBody {
    owner: String,
    repo: String,
    number: i64,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetFavoritesBody {
    repo_full_name: String,
    workflow_ids: Vec<i64>,
}

fn user_id(user: &AuthUser) -> Result<Uuid, AppError> {
    Uuid::parse_str(&user.0.id).map_err(|e| AppError::internal(anyhow::anyhow!(e)))
}

#[utoipa::path(
    get, path = "/api/me/github", operation_id = "githubStatus", tag = "github",
    security(("bearer" = [])),
    responses((status = 200, body = crate::github::summary::GithubConnectionSummary))
)]
/// GET /me/github — connection summary.
pub(crate) async fn status(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let summary = pat::status(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(summary))
}

#[utoipa::path(
    post, path = "/api/me/github/token", operation_id = "githubConnect", tag = "github",
    security(("bearer" = [])), request_body = TokenBody,
    responses((status = 200, body = crate::github::summary::GithubConnectionSummary))
)]
/// POST /me/github/token — validate against GitHub, then store.
pub(crate) async fn connect(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<TokenBody>,
) -> Result<HttpResponse, AppError> {
    let summary = pat::connect(&state, user_id(&user)?, body.token.trim()).await?;
    Ok(HttpResponse::Ok().json(summary))
}

#[utoipa::path(
    post, path = "/api/me/github/token/validate", operation_id = "githubValidate", tag = "github",
    security(("bearer" = [])),
    responses((status = 200, body = crate::github::summary::GithubConnectionSummary))
)]
/// POST /me/github/token/validate — re-validate the stored token.
pub(crate) async fn validate(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let summary = pat::validate(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(summary))
}

#[utoipa::path(
    delete, path = "/api/me/github", operation_id = "githubDisconnect", tag = "github",
    security(("bearer" = [])),
    responses((status = 200, body = crate::dto::DisconnectedResponse))
)]
/// DELETE /me/github — disconnect; clears caches.
pub(crate) async fn disconnect(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    pat::disconnect(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "disconnected": true })))
}

#[utoipa::path(
    get, path = "/api/me/github/dashboard", operation_id = "githubDashboard", tag = "github",
    security(("bearer" = [])),
    params(("tab" = Option<String>, Query, description = "Queue tab (default assigned)")),
    responses((status = 200, body = wf_github::GithubDashboard))
)]
/// GET /me/github/dashboard?tab= — SWR dashboard (counts + active queue).
pub(crate) async fn dashboard_route(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<DashboardQuery>,
) -> Result<HttpResponse, AppError> {
    let tab = q
        .tab
        .as_deref()
        .and_then(GithubQueueKey::parse)
        .unwrap_or(GithubQueueKey::Assigned);
    let d = dashboard::get_dashboard(&state, user_id(&user)?, tab).await?;
    Ok(HttpResponse::Ok().json(d))
}

#[utoipa::path(
    get, path = "/api/me/github/queue", operation_id = "githubQueue", tag = "github",
    security(("bearer" = [])),
    params(("key" = String, Query, description = "Queue key")),
    responses((status = 200, body = wf_github::dashboard::types::GithubPullRequestQueue))
)]
/// GET /me/github/queue?key= — a single queue's PRs.
pub(crate) async fn queue_route(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<QueueQuery>,
) -> Result<HttpResponse, AppError> {
    let key = GithubQueueKey::parse(&q.key)
        .ok_or_else(|| AppError::validation(format!("invalid queue key: {}", q.key)))?;
    let r = dashboard::get_queue(&state, user_id(&user)?, key).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/github/repos", operation_id = "githubRepos", tag = "github",
    security(("bearer" = [])),
    responses((status = 200, body = crate::github::dashboard::RepoSelection))
)]
/// GET /me/github/repos — available repos + current selection.
pub(crate) async fn repos_route(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let r = dashboard::list_repos(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    put, path = "/api/me/github/repos", operation_id = "githubSetRepos", tag = "github",
    security(("bearer" = [])), request_body = ReposBody,
    responses((status = 200, body = crate::github::summary::GithubConnectionSummary))
)]
/// PUT /me/github/repos — set selection; nulls snapshot, clears caches.
pub(crate) async fn set_repos_route(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<ReposBody>,
) -> Result<HttpResponse, AppError> {
    let s = dashboard::set_selected_repos(&state, user_id(&user)?, &body.repos).await?;
    Ok(HttpResponse::Ok().json(s))
}

#[utoipa::path(
    get, path = "/api/me/github/pull", operation_id = "githubPull", tag = "github",
    security(("bearer" = [])),
    params(
        ("owner" = String, Query, description = "Repo owner"),
        ("repo" = String, Query, description = "Repo name"),
        ("number" = String, Query, description = "PR number")
    ),
    responses((status = 200, body = wf_github::GithubPullRequestEnrichment))
)]
/// GET /me/github/pull?owner&repo&number — enrich a single PR.
pub(crate) async fn pull_route(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<PullQuery>,
) -> Result<HttpResponse, AppError> {
    let number: i64 = q
        .number
        .parse()
        .map_err(|_| AppError::validation(format!("invalid pull number: {}", q.number)))?;
    let r = RepoRef { owner: q.owner.clone(), repo: q.repo.clone() };
    let e = dashboard::get_pull_enrichment(&state, user_id(&user)?, r, number).await?;
    Ok(HttpResponse::Ok().json(e))
}

#[utoipa::path(
    post, path = "/api/me/github/pulls/enrich", operation_id = "githubPullsEnrich", tag = "github",
    security(("bearer" = [])), request_body = PullsBody,
    responses((status = 200, body = Vec<wf_github::GithubPullEnrichmentResult>))
)]
/// POST /me/github/pulls/enrich — batch-enrich the supplied PR refs.
pub(crate) async fn pulls_route(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<PullsBody>,
) -> Result<HttpResponse, AppError> {
    let r = dashboard::get_pull_enrichments(&state, user_id(&user)?, &body.refs).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/github/branches", operation_id = "githubBranches", tag = "github",
    security(("bearer" = [])),
    responses((status = 200, body = Vec<wf_github::GithubRepoBranches>))
)]
/// GET /me/github/branches — branch→PR prompts across selected repos.
pub(crate) async fn branches_route(
    state: web::Data<AppState>,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let r = activity::list_branches(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/github/workflows", operation_id = "githubWorkflows", tag = "github",
    security(("bearer" = [])),
    responses((status = 200, body = Vec<wf_github::GithubRepoWorkflows>))
)]
/// GET /me/github/workflows — active workflows across selected repos.
pub(crate) async fn workflows_route(
    state: web::Data<AppState>,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let r = activity::list_workflows(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/github/workflow/inputs", operation_id = "githubWorkflowInputs", tag = "github",
    security(("bearer" = [])),
    params(
        ("owner" = String, Query), ("repo" = String, Query),
        ("path" = String, Query, description = "Workflow file path")
    ),
    responses((status = 200, body = wf_github::GithubWorkflowInputs))
)]
/// GET /me/github/workflow/inputs?owner&repo&path — workflow_dispatch inputs.
pub(crate) async fn workflow_inputs_route(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<WorkflowInputsQuery>,
) -> Result<HttpResponse, AppError> {
    let r =
        activity::workflow_inputs(&state, user_id(&user)?, ref_of(&q.owner, &q.repo), &q.path).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/github/repo/branches", operation_id = "githubRepoBranches", tag = "github",
    security(("bearer" = [])),
    params(("owner" = String, Query), ("repo" = String, Query)),
    responses((status = 200, body = Vec<String>))
)]
/// GET /me/github/repo/branches?owner&repo — plain branch-name list.
pub(crate) async fn repo_branches_route(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<RepoQuery>,
) -> Result<HttpResponse, AppError> {
    let r = activity::repo_branches(&state, user_id(&user)?, ref_of(&q.owner, &q.repo)).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/github/repo/environments", operation_id = "githubRepoEnvironments", tag = "github",
    security(("bearer" = [])),
    params(("owner" = String, Query), ("repo" = String, Query)),
    responses((status = 200, body = Vec<String>))
)]
/// GET /me/github/repo/environments?owner&repo — environment names.
pub(crate) async fn environments_route(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<RepoQuery>,
) -> Result<HttpResponse, AppError> {
    let r = activity::environments(&state, user_id(&user)?, ref_of(&q.owner, &q.repo)).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/github/workflow/runs", operation_id = "githubWorkflowRuns", tag = "github",
    security(("bearer" = [])),
    params(
        ("owner" = String, Query), ("repo" = String, Query),
        ("workflowId" = String, Query), ("branch" = String, Query)
    ),
    responses((status = 200, body = Vec<wf_github::GithubWorkflowRunSummary>))
)]
/// GET /me/github/workflow/runs?owner&repo&workflowId&branch — recent dispatch runs.
pub(crate) async fn workflow_runs_route(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<WorkflowRunsQuery>,
) -> Result<HttpResponse, AppError> {
    let workflow_id: i64 = q
        .workflow_id
        .parse()
        .map_err(|_| AppError::validation(format!("invalid workflowId: {}", q.workflow_id)))?;
    let r = activity::workflow_runs(
        &state,
        user_id(&user)?,
        ref_of(&q.owner, &q.repo),
        workflow_id,
        &q.branch,
    )
    .await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    post, path = "/api/me/github/workflow/dispatch", operation_id = "githubWorkflowDispatch", tag = "github",
    security(("bearer" = [])), request_body = DispatchBody,
    responses((status = 200, body = crate::dto::OkResponse))
)]
/// POST /me/github/workflow/dispatch — trigger a workflow_dispatch.
pub(crate) async fn dispatch_route(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<DispatchBody>,
) -> Result<HttpResponse, AppError> {
    activity::dispatch(
        &state,
        user_id(&user)?,
        ref_of(&body.owner, &body.repo),
        body.workflow_id,
        &body.git_ref,
        &body.inputs,
    )
    .await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post, path = "/api/me/github/pulls", operation_id = "githubCreatePull", tag = "github",
    security(("bearer" = [])), request_body = CreatePullBody,
    responses((status = 200, body = wf_github::GithubCreatePullResult))
)]
/// POST /me/github/pulls — create a PR.
pub(crate) async fn create_pull_route(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<CreatePullBody>,
) -> Result<HttpResponse, AppError> {
    let input = GithubCreatePullInput {
        base: body.base.clone(),
        head: body.head.clone(),
        title: body.title.clone(),
        body: body.body.clone().unwrap_or_default(),
    };
    let r = activity::create_pull(&state, user_id(&user)?, ref_of(&body.owner, &body.repo), &input).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    post, path = "/api/me/github/pull/merge", operation_id = "githubMergePull", tag = "github",
    security(("bearer" = [])), request_body = MergePullBody,
    responses((status = 200, body = wf_github::GithubMergePullResult))
)]
/// POST /me/github/pull/merge — merge a PR.
pub(crate) async fn merge_pull_route(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<MergePullBody>,
) -> Result<HttpResponse, AppError> {
    let r = activity::merge_pull(
        &state,
        user_id(&user)?,
        ref_of(&body.owner, &body.repo),
        body.number,
        body.method,
    )
    .await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    post, path = "/api/me/github/pull/close", operation_id = "githubClosePull", tag = "github",
    security(("bearer" = [])), request_body = ClosePullBody,
    responses((status = 200, body = crate::dto::OkResponse))
)]
/// POST /me/github/pull/close — close a PR.
pub(crate) async fn close_pull_route(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<ClosePullBody>,
) -> Result<HttpResponse, AppError> {
    activity::close_pull(&state, user_id(&user)?, ref_of(&body.owner, &body.repo), body.number).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    get, path = "/api/me/github/favorites", operation_id = "githubFavorites", tag = "github",
    security(("bearer" = [])),
    responses((status = 200, body = Object, description = "Map of repoFullName -> workflow ids"))
)]
/// GET /me/github/favorites — the user's favorite workflows per repo.
pub(crate) async fn favorites_route(
    state: web::Data<AppState>,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let r = github_pat::get_favorites(&state.db, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    put, path = "/api/me/github/favorites", operation_id = "githubSetFavorites", tag = "github",
    security(("bearer" = [])), request_body = SetFavoritesBody,
    responses((status = 200, body = Object, description = "Map of repoFullName -> workflow ids"))
)]
/// PUT /me/github/favorites — set one repo's favorites; returns the full map.
pub(crate) async fn set_favorites_route(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<SetFavoritesBody>,
) -> Result<HttpResponse, AppError> {
    let r = github_pat::set_repo_favorites(
        &state.db,
        user_id(&user)?,
        &body.repo_full_name,
        &body.workflow_ids,
    )
    .await?;
    Ok(HttpResponse::Ok().json(r))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/me/github", web::get().to(status))
        .route("/me/github/token", web::post().to(connect))
        .route("/me/github/token/validate", web::post().to(validate))
        .route("/me/github", web::delete().to(disconnect))
        .route("/me/github/dashboard", web::get().to(dashboard_route))
        .route("/me/github/queue", web::get().to(queue_route))
        .route("/me/github/repos", web::get().to(repos_route))
        .route("/me/github/repos", web::put().to(set_repos_route))
        .route("/me/github/pull", web::get().to(pull_route))
        .route("/me/github/pulls/enrich", web::post().to(pulls_route))
        .route("/me/github/branches", web::get().to(branches_route))
        .route("/me/github/workflows", web::get().to(workflows_route))
        .route("/me/github/workflow/inputs", web::get().to(workflow_inputs_route))
        .route("/me/github/workflow/runs", web::get().to(workflow_runs_route))
        .route("/me/github/repo/branches", web::get().to(repo_branches_route))
        .route("/me/github/repo/environments", web::get().to(environments_route))
        .route("/me/github/workflow/dispatch", web::post().to(dispatch_route))
        .route("/me/github/pulls", web::post().to(create_pull_route))
        .route("/me/github/pull/merge", web::post().to(merge_pull_route))
        .route("/me/github/pull/close", web::post().to(close_pull_route))
        .route("/me/github/favorites", web::get().to(favorites_route))
        .route("/me/github/favorites", web::put().to(set_favorites_route));
}
