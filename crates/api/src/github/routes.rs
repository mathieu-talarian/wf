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

#[derive(Deserialize)]
struct TokenBody {
    token: String,
}

#[derive(Deserialize)]
struct DashboardQuery {
    tab: Option<String>,
}

#[derive(Deserialize)]
struct QueueQuery {
    key: String,
}

#[derive(Deserialize)]
struct ReposBody {
    repos: Vec<String>,
}

#[derive(Deserialize)]
struct PullQuery {
    owner: String,
    repo: String,
    number: String,
}

#[derive(Deserialize)]
struct PullsBody {
    refs: Vec<GithubPullRef>,
}

#[derive(Deserialize)]
struct RepoQuery {
    owner: String,
    repo: String,
}

#[derive(Deserialize)]
struct WorkflowInputsQuery {
    owner: String,
    repo: String,
    path: String,
}

#[derive(Deserialize)]
struct WorkflowRunsQuery {
    owner: String,
    repo: String,
    #[serde(rename = "workflowId")]
    workflow_id: String,
    branch: String,
}

fn ref_of(owner: &str, repo: &str) -> RepoRef {
    RepoRef { owner: owner.to_string(), repo: repo.to_string() }
}

#[derive(Deserialize)]
struct DispatchBody {
    owner: String,
    repo: String,
    #[serde(rename = "workflowId")]
    workflow_id: i64,
    #[serde(rename = "ref")]
    git_ref: String,
    inputs: HashMap<String, String>,
}

#[derive(Deserialize)]
struct CreatePullBody {
    owner: String,
    repo: String,
    base: String,
    head: String,
    title: String,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Deserialize)]
struct MergePullBody {
    owner: String,
    repo: String,
    number: i64,
    method: GithubMergeMethod,
}

#[derive(Deserialize)]
struct ClosePullBody {
    owner: String,
    repo: String,
    number: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetFavoritesBody {
    repo_full_name: String,
    workflow_ids: Vec<i64>,
}

fn user_id(user: &AuthUser) -> Result<Uuid, AppError> {
    Uuid::parse_str(&user.0.id).map_err(|e| AppError::internal(anyhow::anyhow!(e)))
}

/// GET /me/github — connection summary.
async fn status(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let summary = pat::status(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(summary))
}

/// POST /me/github/token — validate against GitHub, then store.
async fn connect(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<TokenBody>,
) -> Result<HttpResponse, AppError> {
    let summary = pat::connect(&state, user_id(&user)?, body.token.trim()).await?;
    Ok(HttpResponse::Ok().json(summary))
}

/// POST /me/github/token/validate — re-validate the stored token.
async fn validate(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let summary = pat::validate(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(summary))
}

/// DELETE /me/github — disconnect; clears caches.
async fn disconnect(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    pat::disconnect(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "disconnected": true })))
}

/// GET /me/github/dashboard?tab= — SWR dashboard (counts + active queue).
async fn dashboard_route(
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

/// GET /me/github/queue?key= — a single queue's PRs.
async fn queue_route(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<QueueQuery>,
) -> Result<HttpResponse, AppError> {
    let key = GithubQueueKey::parse(&q.key)
        .ok_or_else(|| AppError::validation(format!("invalid queue key: {}", q.key)))?;
    let r = dashboard::get_queue(&state, user_id(&user)?, key).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/github/repos — available repos + current selection.
async fn repos_route(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let r = dashboard::list_repos(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// PUT /me/github/repos — set selection; nulls snapshot, clears caches.
async fn set_repos_route(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<ReposBody>,
) -> Result<HttpResponse, AppError> {
    let s = dashboard::set_selected_repos(&state, user_id(&user)?, &body.repos).await?;
    Ok(HttpResponse::Ok().json(s))
}

/// GET /me/github/pull?owner&repo&number — enrich a single PR.
async fn pull_route(
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

/// POST /me/github/pulls/enrich — batch-enrich the supplied PR refs.
async fn pulls_route(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<PullsBody>,
) -> Result<HttpResponse, AppError> {
    let r = dashboard::get_pull_enrichments(&state, user_id(&user)?, &body.refs).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/github/branches — branch→PR prompts across selected repos.
async fn branches_route(
    state: web::Data<AppState>,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let r = activity::list_branches(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/github/workflows — active workflows across selected repos.
async fn workflows_route(
    state: web::Data<AppState>,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let r = activity::list_workflows(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/github/workflow/inputs?owner&repo&path — workflow_dispatch inputs.
async fn workflow_inputs_route(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<WorkflowInputsQuery>,
) -> Result<HttpResponse, AppError> {
    let r =
        activity::workflow_inputs(&state, user_id(&user)?, ref_of(&q.owner, &q.repo), &q.path).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/github/repo/branches?owner&repo — plain branch-name list.
async fn repo_branches_route(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<RepoQuery>,
) -> Result<HttpResponse, AppError> {
    let r = activity::repo_branches(&state, user_id(&user)?, ref_of(&q.owner, &q.repo)).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/github/repo/environments?owner&repo — environment names.
async fn environments_route(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<RepoQuery>,
) -> Result<HttpResponse, AppError> {
    let r = activity::environments(&state, user_id(&user)?, ref_of(&q.owner, &q.repo)).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/github/workflow/runs?owner&repo&workflowId&branch — recent dispatch runs.
async fn workflow_runs_route(
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

/// POST /me/github/workflow/dispatch — trigger a workflow_dispatch.
async fn dispatch_route(
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

/// POST /me/github/pulls — create a PR.
async fn create_pull_route(
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

/// POST /me/github/pull/merge — merge a PR.
async fn merge_pull_route(
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

/// POST /me/github/pull/close — close a PR.
async fn close_pull_route(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<ClosePullBody>,
) -> Result<HttpResponse, AppError> {
    activity::close_pull(&state, user_id(&user)?, ref_of(&body.owner, &body.repo), body.number).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

/// GET /me/github/favorites — the user's favorite workflows per repo.
async fn favorites_route(
    state: web::Data<AppState>,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let r = github_pat::get_favorites(&state.db, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// PUT /me/github/favorites — set one repo's favorites; returns the full map.
async fn set_favorites_route(
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
