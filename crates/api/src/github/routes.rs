//! GitHub connection routes (migration plan §14.2; port of
//! `github/routes/pat.ts`). All require a valid Supabase JWT.

use actix_web::{web, HttpResponse};
use sea_orm::prelude::Uuid;
use serde::Deserialize;
use wf_github::GithubQueueKey;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::github::{dashboard, pat};
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

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/me/github", web::get().to(status))
        .route("/me/github/token", web::post().to(connect))
        .route("/me/github/token/validate", web::post().to(validate))
        .route("/me/github", web::delete().to(disconnect))
        .route("/me/github/dashboard", web::get().to(dashboard_route))
        .route("/me/github/queue", web::get().to(queue_route))
        .route("/me/github/repos", web::get().to(repos_route))
        .route("/me/github/repos", web::put().to(set_repos_route));
}
