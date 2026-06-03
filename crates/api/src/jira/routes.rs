//! Jira connection routes (port of `jira/routes/pat.ts`). All require a valid
//! Supabase JWT.

use actix_web::{web, HttpResponse};
use sea_orm::prelude::Uuid;
use serde::Deserialize;
use wf_jira::JiraConnectInput;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::jira::pat;
use crate::state::AppState;

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

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/me/jira", web::get().to(status))
        .route("/me/jira/token", web::post().to(connect))
        .route("/me/jira/token/validate", web::post().to(validate))
        .route("/me/jira", web::delete().to(disconnect))
        .route("/me/jira/projects", web::put().to(set_projects));
}
