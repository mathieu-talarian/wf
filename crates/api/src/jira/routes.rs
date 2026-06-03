//! Jira connection routes (port of `jira/routes/pat.ts`). All require a valid
//! Supabase JWT.

use actix_web::{web, HttpResponse};
use sea_orm::prelude::Uuid;
use serde::Deserialize;
use serde_json::{Map, Value};
use wf_jira::{AssignableQuery, JiraConnectInput, JiraCreateIssueInput, JiraQueueKey, JiraWorklogInput};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::jira::{actions, data, pat};
use crate::state::AppState;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransitionBody {
    key: String,
    transition_id: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct CommentBody {
    key: String,
    body: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssignBody {
    key: String,
    account_id: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorklogBody {
    key: String,
    time_spent: String,
    started: Option<String>,
    comment: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct EditBody {
    key: String,
    fields: Map<String, Value>,
}

fn cursor_of(raw: &Option<String>) -> Option<&str> {
    raw.as_deref().filter(|s| !s.is_empty())
}

#[derive(Deserialize)]
pub(crate) struct QueueQuery {
    key: String,
    cursor: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct SearchBody {
    jql: String,
    cursor: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct KeyQuery {
    key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectKeyQuery {
    project_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BoardQuery {
    board_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateMetaQuery {
    project_key: String,
    issue_type_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsersQuery {
    query: String,
    issue_key: Option<String>,
    project_key_or_id: Option<String>,
    action_descriptor_id: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectBody {
    site_url: String,
    email: String,
    token: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct ProjectsBody {
    projects: Vec<String>,
}

fn user_id(user: &AuthUser) -> Result<Uuid, AppError> {
    Uuid::parse_str(&user.0.id).map_err(|e| AppError::internal(anyhow::anyhow!(e)))
}

#[utoipa::path(
    get, path = "/api/me/jira", operation_id = "jiraStatus", tag = "jira",
    security(("bearer" = [])),
    responses((status = 200, body = crate::jira::summary::JiraConnectionSummary))
)]
/// GET /me/jira — connection summary.
pub(crate) async fn status(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let summary = pat::status(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(summary))
}

#[utoipa::path(
    post, path = "/api/me/jira/token", operation_id = "jiraConnect", tag = "jira",
    security(("bearer" = [])), request_body = ConnectBody,
    responses((status = 200, body = crate::jira::summary::JiraConnectionSummary))
)]
/// POST /me/jira/token — validate credentials against Jira, then store.
pub(crate) async fn connect(
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

#[utoipa::path(
    post, path = "/api/me/jira/token/validate", operation_id = "jiraValidate", tag = "jira",
    security(("bearer" = [])),
    responses((status = 200, body = crate::jira::summary::JiraConnectionSummary))
)]
/// POST /me/jira/token/validate — re-validate the stored credentials.
pub(crate) async fn validate(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let summary = pat::validate(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(summary))
}

#[utoipa::path(
    delete, path = "/api/me/jira", operation_id = "jiraDisconnect", tag = "jira",
    security(("bearer" = [])),
    responses((status = 200, body = crate::dto::DisconnectedResponse))
)]
/// DELETE /me/jira — disconnect.
pub(crate) async fn disconnect(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    pat::disconnect(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "disconnected": true })))
}

#[utoipa::path(
    put, path = "/api/me/jira/projects", operation_id = "jiraSetProjects", tag = "jira",
    security(("bearer" = [])), request_body = ProjectsBody,
    responses((status = 200, body = crate::jira::summary::JiraConnectionSummary))
)]
/// PUT /me/jira/projects — set selected projects.
pub(crate) async fn set_projects(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<ProjectsBody>,
) -> Result<HttpResponse, AppError> {
    let summary = pat::set_projects(&state, user_id(&user)?, &body.projects).await?;
    Ok(HttpResponse::Ok().json(summary))
}

#[utoipa::path(
    get, path = "/api/me/jira/dashboard", operation_id = "jiraDashboard", tag = "jira",
    security(("bearer" = [])),
    responses((status = 200, body = wf_jira::JiraDashboard))
)]
/// GET /me/jira/dashboard — multi-queue dashboard.
pub(crate) async fn dashboard(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let d = data::dashboard(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(d))
}

#[utoipa::path(
    get, path = "/api/me/jira/queue", operation_id = "jiraQueue", tag = "jira",
    security(("bearer" = [])),
    params(("key" = String, Query), ("cursor" = Option<String>, Query)),
    responses((status = 200, body = wf_jira::JiraIssuePage))
)]
/// GET /me/jira/queue?key&cursor — one queue's issue page.
pub(crate) async fn queue(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<QueueQuery>,
) -> Result<HttpResponse, AppError> {
    let key = JiraQueueKey::parse(&q.key)
        .ok_or_else(|| AppError::validation(format!("invalid queue key: {}", q.key)))?;
    let r = data::queue(&state, user_id(&user)?, key, cursor_of(&q.cursor)).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    post, path = "/api/me/jira/search", operation_id = "jiraSearch", tag = "jira",
    security(("bearer" = [])), request_body = SearchBody,
    responses((status = 200, body = wf_jira::JiraIssuePage))
)]
/// POST /me/jira/search — arbitrary JQL search.
pub(crate) async fn search(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<SearchBody>,
) -> Result<HttpResponse, AppError> {
    let r = data::search(&state, user_id(&user)?, &body.jql, cursor_of(&body.cursor)).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/jira/issue", operation_id = "jiraIssue", tag = "jira",
    security(("bearer" = [])),
    params(("key" = String, Query, description = "Issue key")),
    responses((status = 200, body = wf_jira::JiraIssueDetail))
)]
/// GET /me/jira/issue?key — issue detail.
pub(crate) async fn issue(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<KeyQuery>,
) -> Result<HttpResponse, AppError> {
    let r = data::issue(&state, user_id(&user)?, &q.key).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/jira/projects", operation_id = "jiraProjects", tag = "jira",
    security(("bearer" = [])),
    responses((status = 200, body = Vec<wf_jira::JiraProject>))
)]
/// GET /me/jira/projects — selectable projects.
pub(crate) async fn projects(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let r = data::projects(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/jira/issuetypes", operation_id = "jiraIssueTypes", tag = "jira",
    security(("bearer" = [])),
    params(("projectKey" = String, Query)),
    responses((status = 200, body = Vec<wf_jira::JiraIssueType>))
)]
/// GET /me/jira/issuetypes?projectKey — issue types for a project.
pub(crate) async fn issue_types(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<ProjectKeyQuery>,
) -> Result<HttpResponse, AppError> {
    let r = data::issue_types(&state, user_id(&user)?, &q.project_key).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/jira/boards", operation_id = "jiraBoards", tag = "jira",
    security(("bearer" = [])),
    responses((status = 200, body = Vec<wf_jira::JiraBoard>))
)]
/// GET /me/jira/boards — agile boards.
pub(crate) async fn boards(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let r = data::boards(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/jira/sprint/issues", operation_id = "jiraSprintIssues", tag = "jira",
    security(("bearer" = [])),
    params(("boardId" = String, Query)),
    responses((status = 200, body = wf_jira::JiraIssuePage))
)]
/// GET /me/jira/sprint/issues?boardId — active sprint issues.
pub(crate) async fn sprint(
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

#[utoipa::path(
    get, path = "/api/me/jira/issue/transitions", operation_id = "jiraTransitions", tag = "jira",
    security(("bearer" = [])),
    params(("key" = String, Query, description = "Issue key")),
    responses((status = 200, body = Vec<wf_jira::JiraTransition>))
)]
/// GET /me/jira/issue/transitions?key — available transitions.
pub(crate) async fn transitions(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<KeyQuery>,
) -> Result<HttpResponse, AppError> {
    let r = data::transitions(&state, user_id(&user)?, &q.key).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/jira/users", operation_id = "jiraUsers", tag = "jira",
    security(("bearer" = [])),
    params(
        ("query" = String, Query), ("issueKey" = Option<String>, Query),
        ("projectKeyOrId" = Option<String>, Query), ("actionDescriptorId" = Option<String>, Query)
    ),
    responses((status = 200, body = Vec<wf_jira::JiraUser>))
)]
/// GET /me/jira/users — assignable user search.
pub(crate) async fn users(
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

#[utoipa::path(
    get, path = "/api/me/jira/createmeta", operation_id = "jiraCreateMeta", tag = "jira",
    security(("bearer" = [])),
    params(("projectKey" = String, Query), ("issueTypeId" = String, Query)),
    responses((status = 200, body = wf_jira::JiraCreateMeta))
)]
/// GET /me/jira/createmeta?projectKey&issueTypeId — create field metadata.
pub(crate) async fn create_meta(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<CreateMetaQuery>,
) -> Result<HttpResponse, AppError> {
    let r = data::create_meta(&state, user_id(&user)?, &q.project_key, &q.issue_type_id).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    get, path = "/api/me/jira/editmeta", operation_id = "jiraEditMeta", tag = "jira",
    security(("bearer" = [])),
    params(("key" = String, Query, description = "Issue key")),
    responses((status = 200, body = wf_jira::JiraEditMeta))
)]
/// GET /me/jira/editmeta?key — edit field metadata.
pub(crate) async fn edit_meta(
    state: web::Data<AppState>,
    user: AuthUser,
    q: web::Query<KeyQuery>,
) -> Result<HttpResponse, AppError> {
    let r = data::edit_meta(&state, user_id(&user)?, &q.key).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    post, path = "/api/me/jira/issue/transition", operation_id = "jiraTransition", tag = "jira",
    security(("bearer" = [])), request_body = TransitionBody,
    responses((status = 200, body = crate::dto::OkResponse))
)]
/// POST /me/jira/issue/transition — apply a transition.
pub(crate) async fn transition(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<TransitionBody>,
) -> Result<HttpResponse, AppError> {
    actions::transition(&state, user_id(&user)?, &body.key, &body.transition_id).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post, path = "/api/me/jira/issue/comment", operation_id = "jiraComment", tag = "jira",
    security(("bearer" = [])), request_body = CommentBody,
    responses((status = 200, body = wf_jira::JiraComment))
)]
/// POST /me/jira/issue/comment — add a comment.
pub(crate) async fn comment(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<CommentBody>,
) -> Result<HttpResponse, AppError> {
    let c = actions::comment(&state, user_id(&user)?, &body.key, &body.body).await?;
    Ok(HttpResponse::Ok().json(c))
}

#[utoipa::path(
    post, path = "/api/me/jira/issue/assign", operation_id = "jiraAssign", tag = "jira",
    security(("bearer" = [])), request_body = AssignBody,
    responses((status = 200, body = crate::dto::OkResponse))
)]
/// POST /me/jira/issue/assign — (re)assign or unassign (null).
pub(crate) async fn assign(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<AssignBody>,
) -> Result<HttpResponse, AppError> {
    actions::assign(&state, user_id(&user)?, &body.key, body.account_id.as_deref()).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post, path = "/api/me/jira/issue/worklog", operation_id = "jiraWorklog", tag = "jira",
    security(("bearer" = [])), request_body = WorklogBody,
    responses((status = 200, body = crate::dto::OkResponse))
)]
/// POST /me/jira/issue/worklog — log work.
pub(crate) async fn worklog(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<WorklogBody>,
) -> Result<HttpResponse, AppError> {
    let input = JiraWorklogInput {
        time_spent: body.time_spent.clone(),
        started: body.started.clone(),
        comment: body.comment.clone(),
    };
    actions::worklog(&state, user_id(&user)?, &body.key, &input).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post, path = "/api/me/jira/issue", operation_id = "jiraCreateIssue", tag = "jira",
    security(("bearer" = [])), request_body = JiraCreateIssueInput,
    responses((status = 200, body = wf_jira::JiraCreateIssueResult))
)]
/// POST /me/jira/issue — create an issue (fields pass the metadata allowlist).
pub(crate) async fn create_issue(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<JiraCreateIssueInput>,
) -> Result<HttpResponse, AppError> {
    let r = actions::create(&state, user_id(&user)?, &body).await?;
    Ok(HttpResponse::Ok().json(r))
}

#[utoipa::path(
    put, path = "/api/me/jira/issue", operation_id = "jiraEditIssue", tag = "jira",
    security(("bearer" = [])), request_body = EditBody,
    responses((status = 200, body = crate::dto::OkResponse))
)]
/// PUT /me/jira/issue — edit an issue.
pub(crate) async fn edit_issue(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<EditBody>,
) -> Result<HttpResponse, AppError> {
    actions::edit(&state, user_id(&user)?, &body.key, &body.fields).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
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
        .route("/me/jira/editmeta", web::get().to(edit_meta))
        // Actions (writes)
        .route("/me/jira/issue/transition", web::post().to(transition))
        .route("/me/jira/issue/comment", web::post().to(comment))
        .route("/me/jira/issue/assign", web::post().to(assign))
        .route("/me/jira/issue/worklog", web::post().to(worklog))
        .route("/me/jira/issue", web::post().to(create_issue))
        .route("/me/jira/issue", web::put().to(edit_issue));
}
