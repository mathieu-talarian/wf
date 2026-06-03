//! Jira connection routes (port of `jira/routes/pat.ts`). All require a valid
//! Supabase JWT.

use actix_web::{web, HttpResponse};
use sea_orm::prelude::Uuid;
use serde::Deserialize;
use wf_jira::{AssignableQuery, JiraConnectInput, JiraQueueKey};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::jira::{data, pat};
use crate::state::AppState;

fn cursor_of(raw: &Option<String>) -> Option<&str> {
    raw.as_deref().filter(|s| !s.is_empty())
}

#[derive(Deserialize)]
struct QueueQuery {
    key: String,
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct SearchBody {
    jql: String,
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct KeyQuery {
    key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectKeyQuery {
    project_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoardQuery {
    board_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateMetaQuery {
    project_key: String,
    issue_type_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsersQuery {
    query: String,
    issue_key: Option<String>,
    project_key_or_id: Option<String>,
    action_descriptor_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectBody {
    site_url: String,
    email: String,
    token: String,
}

#[derive(Deserialize)]
struct ProjectsBody {
    projects: Vec<String>,
}

fn user_id(user: &AuthUser) -> Result<Uuid, AppError> {
    Uuid::parse_str(&user.0.id).map_err(|e| AppError::internal(anyhow::anyhow!(e)))
}

/// GET /me/jira — connection summary.
async fn status(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let summary = pat::status(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(summary))
}

/// POST /me/jira/token — validate credentials against Jira, then store.
async fn connect(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<ConnectBody>,
) -> Result<HttpResponse, AppError> {
    let input = JiraConnectInput {
        site_url: body.site_url.trim().to_string(),
        email: body.email.trim().to_string(),
        token: body.token.trim().to_string(),
    };
    let summary = pat::connect(&state, user_id(&user)?, input).await?;
    Ok(HttpResponse::Ok().json(summary))
}

/// POST /me/jira/token/validate — re-validate the stored credentials.
async fn validate(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let summary = pat::validate(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(summary))
}

/// DELETE /me/jira — disconnect.
async fn disconnect(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    pat::disconnect(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "disconnected": true })))
}

/// PUT /me/jira/projects — set selected projects.
async fn set_projects(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<ProjectsBody>,
) -> Result<HttpResponse, AppError> {
    let summary = pat::set_projects(&state, user_id(&user)?, &body.projects).await?;
    Ok(HttpResponse::Ok().json(summary))
}

/// GET /me/jira/dashboard — multi-queue dashboard.
async fn dashboard(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let d = data::dashboard(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(d))
}

/// GET /me/jira/queue?key&cursor — one queue's issue page.
async fn queue(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<QueueQuery>,
) -> Result<HttpResponse, AppError> {
    let key = JiraQueueKey::parse(&q.key)
        .ok_or_else(|| AppError::validation(format!("invalid queue key: {}", q.key)))?;
    let r = data::queue(&state, user_id(&user)?, key, cursor_of(&q.cursor)).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// POST /me/jira/search — arbitrary JQL search.
async fn search(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<SearchBody>,
) -> Result<HttpResponse, AppError> {
    let r = data::search(&state, user_id(&user)?, &body.jql, cursor_of(&body.cursor)).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/jira/issue?key — issue detail.
async fn issue(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<KeyQuery>,
) -> Result<HttpResponse, AppError> {
    let r = data::issue(&state, user_id(&user)?, &q.key).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/jira/projects — selectable projects.
async fn projects(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let r = data::projects(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/jira/issuetypes?projectKey — issue types for a project.
async fn issue_types(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<ProjectKeyQuery>,
) -> Result<HttpResponse, AppError> {
    let r = data::issue_types(&state, user_id(&user)?, &q.project_key).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/jira/boards — agile boards.
async fn boards(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let r = data::boards(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/jira/sprint/issues?boardId — active sprint issues.
async fn sprint(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<BoardQuery>,
) -> Result<HttpResponse, AppError> {
    let board_id: i64 = q
        .board_id
        .parse()
        .map_err(|_| AppError::validation(format!("invalid boardId: {}", q.board_id)))?;
    let r = data::sprint(&state, user_id(&user)?, board_id).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/jira/issue/transitions?key — available transitions.
async fn transitions(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<KeyQuery>,
) -> Result<HttpResponse, AppError> {
    let r = data::transitions(&state, user_id(&user)?, &q.key).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/jira/users — assignable user search.
async fn users(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<UsersQuery>,
) -> Result<HttpResponse, AppError> {
    let input = AssignableQuery {
        query: q.query.clone(),
        issue_key: q.issue_key.clone(),
        project_key_or_id: q.project_key_or_id.clone(),
        action_descriptor_id: q.action_descriptor_id.clone(),
    };
    let r = data::users(&state, user_id(&user)?, &input).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/jira/createmeta?projectKey&issueTypeId — create field metadata.
async fn create_meta(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<CreateMetaQuery>,
) -> Result<HttpResponse, AppError> {
    let r = data::create_meta(&state, user_id(&user)?, &q.project_key, &q.issue_type_id).await?;
    Ok(HttpResponse::Ok().json(r))
}

/// GET /me/jira/editmeta?key — edit field metadata.
async fn edit_meta(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<KeyQuery>,
) -> Result<HttpResponse, AppError> {
    let r = data::edit_meta(&state, user_id(&user)?, &q.key).await?;
    Ok(HttpResponse::Ok().json(r))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/me/jira", web::get().to(status))
        .route("/me/jira/token", web::post().to(connect))
        .route("/me/jira/token/validate", web::post().to(validate))
        .route("/me/jira", web::delete().to(disconnect))
        .route("/me/jira/projects", web::put().to(set_projects))
        // Data reads
        .route("/me/jira/dashboard", web::get().to(dashboard))
        .route("/me/jira/queue", web::get().to(queue))
        .route("/me/jira/search", web::post().to(search))
        .route("/me/jira/issue", web::get().to(issue))
        .route("/me/jira/projects", web::get().to(projects))
        .route("/me/jira/issuetypes", web::get().to(issue_types))
        .route("/me/jira/boards", web::get().to(boards))
        .route("/me/jira/sprint/issues", web::get().to(sprint))
        .route("/me/jira/issue/transitions", web::get().to(transitions))
        .route("/me/jira/users", web::get().to(users))
        .route("/me/jira/createmeta", web::get().to(create_meta))
        .route("/me/jira/editmeta", web::get().to(edit_meta));
}
