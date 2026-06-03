//! GitHub connection routes (migration plan §14.2; port of
//! `github/routes/pat.ts`). All require a valid Supabase JWT.

use actix_web::{web, HttpResponse};
use sea_orm::prelude::Uuid;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::github::pat;
use crate::state::AppState;

#[derive(Deserialize)]
struct TokenBody {
    token: String,
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

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/me/github", web::get().to(status))
        .route("/me/github/token", web::post().to(connect))
        .route("/me/github/token/validate", web::post().to(validate))
        .route("/me/github", web::delete().to(disconnect));
}
