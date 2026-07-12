//! Slack routes (`slack` tag). All require a valid Supabase JWT.

use actix_web::{HttpResponse, web};
use sea_orm::prelude::Uuid;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::slack::{data, pat};
use crate::slack::summary::SlackChannelRef;
use crate::state::AppState;

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct SlackTokenBody {
    token: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackChannelsBody {
    channels: Vec<SlackChannelRef>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackChannelsQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackReplyBody {
    channel_id: String,
    thread_ts: String,
    text: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackMarkReadBody {
    ticket_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TicketKeyQuery {
    ticket_key: String,
}

fn user_id(user: &AuthUser) -> Result<Uuid, AppError> {
    Uuid::parse_str(&user.0.id).map_err(|e| AppError::internal(anyhow::anyhow!(e)))
}

#[utoipa::path(
    get, path = "/api/me/slack", operation_id = "slackStatus", tag = "slack",
    security(("bearer" = [])),
    responses((status = 200, body = crate::slack::summary::SlackConnectionSummary))
)]
/// GET /me/slack — connection status + watched channels.
pub(crate) async fn status(
    state: web::Data<AppState>,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let summary = pat::status(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(summary))
}

#[utoipa::path(
    post, path = "/api/me/slack/token", operation_id = "slackConnect", tag = "slack",
    security(("bearer" = [])), request_body = SlackTokenBody,
    responses((status = 202, body = crate::slack::summary::SlackConnectionSummary))
)]
/// POST /me/slack/token — store + validate a bot token.
pub(crate) async fn connect(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<SlackTokenBody>,
) -> Result<HttpResponse, AppError> {
    let uid = user_id(&user)?;
    let summary = pat::connect(&state, uid, &body.token).await?;
    crate::scheduler::trigger(state, uid, "slack");
    Ok(HttpResponse::Accepted().json(summary))
}

#[utoipa::path(
    post, path = "/api/me/slack/token/validate", operation_id = "slackValidate", tag = "slack",
    security(("bearer" = [])),
    responses((status = 200, body = crate::slack::summary::SlackConnectionSummary))
)]
/// POST /me/slack/token/validate — re-validate the stored token.
pub(crate) async fn validate(
    state: web::Data<AppState>,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let summary = pat::validate(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(summary))
}

#[utoipa::path(
    delete, path = "/api/me/slack", operation_id = "slackDisconnect", tag = "slack",
    security(("bearer" = [])),
    responses((status = 200, body = crate::dto::OkResponse))
)]
/// DELETE /me/slack — remove the connection (and its synced messages stay).
pub(crate) async fn disconnect(
    state: web::Data<AppState>,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let uid = user_id(&user)?;
    pat::disconnect(&state, uid).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    get, path = "/api/me/slack/channels", operation_id = "slackChannels", tag = "slack",
    security(("bearer" = [])),
    params(SlackChannelsQuery),
    responses((status = 200, body = crate::slack::summary::SlackChannelsPage))
)]
/// GET /me/slack/channels — channels visible to the bot (for the picker).
pub(crate) async fn channels(
    state: web::Data<AppState>,
    user: AuthUser,
    query: web::Query<SlackChannelsQuery>,
) -> Result<HttpResponse, AppError> {
    let channels = pat::list_channels(
        &state,
        user_id(&user)?,
        query.cursor.as_deref(),
        query.limit.unwrap_or(100),
    )
    .await?;
    Ok(HttpResponse::Ok().json(channels))
}

#[utoipa::path(
    put, path = "/api/me/slack/channels", operation_id = "slackSetChannels", tag = "slack",
    security(("bearer" = [])), request_body = SlackChannelsBody,
    responses((status = 202, body = crate::slack::summary::SlackConnectionSummary))
)]
/// PUT /me/slack/channels — set the watched channels.
pub(crate) async fn set_channels(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<SlackChannelsBody>,
) -> Result<HttpResponse, AppError> {
    let uid = user_id(&user)?;
    let summary = pat::set_channels(&state, uid, &body.channels).await?;
    crate::scheduler::trigger(state, uid, "slack");
    Ok(HttpResponse::Accepted().json(summary))
}

#[utoipa::path(
    get, path = "/api/me/slack/threads", operation_id = "slackThreads", tag = "slack",
    security(("bearer" = [])),
    params(("ticketKey" = String, Query, description = "Jira ticket key")),
    responses((status = 200, body = crate::slack::summary::SlackThreadsResult))
)]
/// GET /me/slack/threads?ticketKey= — QA threads matched to a ticket.
pub(crate) async fn threads(
    state: web::Data<AppState>,
    user: AuthUser,
    query: web::Query<TicketKeyQuery>,
) -> Result<HttpResponse, AppError> {
    let result = data::threads(&state, user_id(&user)?, &query.ticket_key).await?;
    Ok(HttpResponse::Ok().json(result))
}

#[utoipa::path(
    post, path = "/api/me/slack/reply", operation_id = "slackReply", tag = "slack",
    security(("bearer" = [])), request_body = SlackReplyBody,
    responses((status = 200, body = crate::slack::summary::SlackMessage))
)]
/// POST /me/slack/reply — post a reply into a thread.
pub(crate) async fn reply(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<SlackReplyBody>,
) -> Result<HttpResponse, AppError> {
    let message = data::reply(
        &state,
        user_id(&user)?,
        &body.channel_id,
        &body.thread_ts,
        &body.text,
    )
    .await?;
    Ok(HttpResponse::Ok().json(message))
}

#[utoipa::path(
    post, path = "/api/me/slack/read", operation_id = "slackMarkRead", tag = "slack",
    security(("bearer" = [])), request_body = SlackMarkReadBody,
    responses((status = 200, body = crate::dto::OkResponse))
)]
/// POST /me/slack/read — mark a ticket's messages read.
pub(crate) async fn mark_read(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<SlackMarkReadBody>,
) -> Result<HttpResponse, AppError> {
    let uid = user_id(&user)?;
    data::mark_read(&state, uid, &body.ticket_key).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/me/slack", web::get().to(status))
        .route("/me/slack/token", web::post().to(connect))
        .route("/me/slack/token/validate", web::post().to(validate))
        .route("/me/slack", web::delete().to(disconnect))
        .route("/me/slack/channels", web::get().to(channels))
        .route("/me/slack/channels", web::put().to(set_channels))
        .route("/me/slack/threads", web::get().to(threads))
        .route("/me/slack/reply", web::post().to(reply))
        .route("/me/slack/read", web::post().to(mark_read));
}
