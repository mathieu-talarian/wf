//! Hub routes (`hub` tag). All require a valid Supabase JWT.

use actix_web::{web, HttpRequest, HttpResponse};
use sea_orm::prelude::Uuid;
use serde::{Deserialize, Serialize};
use wf_db::tables::ticket_links;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::hub::types::HubLinkBody;
use crate::hub::{board, cache, inbox, runs};
use crate::state::AppState;

fn user_id(user: &AuthUser) -> Result<Uuid, AppError> {
    Uuid::parse_str(&user.0.id).map_err(|e| AppError::internal(anyhow::anyhow!(e)))
}

/// Weak ETag over the serialized body — the hub payloads are polled every
/// 20–60s and rarely change, so a 304 skips re-serialization and client re-render.
fn etag_for(body: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut h);
    format!("W/\"{:x}\"", h.finish())
}

/// Serializes `value` as JSON with an ETag, returning `304 Not Modified` when the
/// client's `If-None-Match` already matches. ponytail: exact match only — our
/// polling client echoes one ETag, so `*`/multi-value lists aren't handled.
fn json_or_304<T: Serialize>(req: &HttpRequest, value: &T) -> Result<HttpResponse, AppError> {
    let body = serde_json::to_vec(value).map_err(|e| AppError::internal(anyhow::anyhow!(e)))?;
    let etag = etag_for(&body);
    let matched = req
        .headers()
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        == Some(etag.as_str());
    if matched {
        return Ok(HttpResponse::NotModified().insert_header(("ETag", etag)).finish());
    }
    Ok(HttpResponse::Ok()
        .insert_header(("ETag", etag))
        .content_type("application/json")
        .body(body))
}

fn validate_target(pr_number: Option<i64>, branch: Option<&str>) -> Result<(), AppError> {
    match (pr_number, branch) {
        (Some(_), None) | (None, Some(_)) => Ok(()),
        _ => Err(AppError::validation("Exactly one of `prNumber` / `branch` must be set.")),
    }
}

#[utoipa::path(
    get, path = "/api/me/hub/board", operation_id = "hubBoard", tag = "hub",
    security(("bearer" = [])),
    responses((status = 200, body = crate::hub::types::HubBoard))
)]
/// GET /me/hub/board — the composed kanban board + orphan tray.
pub(crate) async fn board_route(
    state: web::Data<AppState>,
    req: HttpRequest,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let board = board::board(&state, user_id(&user)?).await?;
    json_or_304(&req, &board)
}

#[utoipa::path(
    post, path = "/api/me/hub/links", operation_id = "hubLink", tag = "hub",
    security(("bearer" = [])), request_body = HubLinkBody,
    responses((status = 200, body = crate::dto::OkResponse))
)]
/// POST /me/hub/links — manually link a PR or branch to a ticket.
pub(crate) async fn link_route(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<HubLinkBody>,
) -> Result<HttpResponse, AppError> {
    let uid = user_id(&user)?;
    validate_target(body.pr_number, body.branch.as_deref())?;
    ticket_links::insert(
        &state.db,
        uid,
        &body.ticket_key,
        &body.repo,
        body.pr_number,
        body.branch.as_deref(),
    )
    .await?;
    cache::invalidate_board(uid);
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnlinkQuery {
    #[allow(dead_code)]
    ticket_key: Option<String>,
    repo: String,
    pr_number: Option<i64>,
    branch: Option<String>,
}

#[utoipa::path(
    delete, path = "/api/me/hub/links", operation_id = "hubUnlink", tag = "hub",
    security(("bearer" = [])),
    params(
        ("ticketKey" = Option<String>, Query, description = "Unused; accepted for symmetry"),
        ("repo" = String, Query, description = "owner/name"),
        ("prNumber" = Option<i64>, Query, description = "PR target"),
        ("branch" = Option<String>, Query, description = "Branch target")
    ),
    responses((status = 200, body = crate::dto::OkResponse))
)]
/// DELETE /me/hub/links — remove a manual link.
pub(crate) async fn unlink_route(
    state: web::Data<AppState>,
    user: AuthUser,
    query: web::Query<UnlinkQuery>,
) -> Result<HttpResponse, AppError> {
    let uid = user_id(&user)?;
    validate_target(query.pr_number, query.branch.as_deref())?;
    ticket_links::delete_target(
        &state.db,
        uid,
        &query.repo,
        query.pr_number,
        query.branch.as_deref(),
    )
    .await?;
    cache::invalidate_board(uid);
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    get, path = "/api/me/hub/inbox", operation_id = "hubInbox", tag = "hub",
    security(("bearer" = [])),
    responses((status = 200, body = crate::hub::types::HubInbox))
)]
/// GET /me/hub/inbox — the ranked Needs-you feed (+ morning brief when on).
pub(crate) async fn inbox_route(
    state: web::Data<AppState>,
    req: HttpRequest,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let inbox = inbox::inbox(&state, user_id(&user)?).await?;
    json_or_304(&req, &inbox)
}

#[utoipa::path(
    get, path = "/api/me/hub/runs", operation_id = "hubRuns", tag = "hub",
    security(("bearer" = [])),
    responses((status = 200, body = crate::hub::types::HubRuns))
)]
/// GET /me/hub/runs — actions-strip run pills (favorites first-class).
pub(crate) async fn runs_route(
    state: web::Data<AppState>,
    req: HttpRequest,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let runs = runs::runs(&state, user_id(&user)?).await?;
    json_or_304(&req, &runs)
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/me/hub/board", web::get().to(board_route))
        .route("/me/hub/links", web::post().to(link_route))
        .route("/me/hub/links", web::delete().to(unlink_route))
        .route("/me/hub/inbox", web::get().to(inbox_route))
        .route("/me/hub/runs", web::get().to(runs_route));
}

#[cfg(test)]
mod tests {
    use super::etag_for;

    #[test]
    fn etag_is_stable_and_content_sensitive() {
        assert_eq!(etag_for(b"hello"), etag_for(b"hello"));
        assert_ne!(etag_for(b"hello"), etag_for(b"hellp"));
        assert!(etag_for(b"hello").starts_with("W/\""));
    }
}
