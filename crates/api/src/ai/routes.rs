//! AI routes (`ai` tag): settings + the two draft endpoints. Guards per the
//! contract: 409 `ai-disabled` when the toggle is off, 503 `ai-unconfigured`
//! when `ANTHROPIC_API_KEY` is absent.

use actix_web::{web, HttpResponse};
use sea_orm::prelude::Uuid;
use serde::{Deserialize, Serialize};
use wf_db::tables::slack_messages;

use crate::ai::settings::{self, AiSettings};
use crate::ai::anthropic;
use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiDraft {
    text: String,
    generated_at: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiDraftReplyBody {
    ticket_key: String,
    channel_id: String,
    thread_ts: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiHandoffBody {
    ticket_key: String,
}

fn user_id(user: &AuthUser) -> Result<Uuid, AppError> {
    Uuid::parse_str(&user.0.id).map_err(|e| AppError::internal(anyhow::anyhow!(e)))
}

fn draft(text: String) -> AiDraft {
    AiDraft { text, generated_at: chrono::Utc::now().to_rfc3339() }
}

#[utoipa::path(
    get, path = "/api/me/ai/settings", operation_id = "aiSettings", tag = "ai",
    security(("bearer" = [])),
    responses((status = 200, body = AiSettings))
)]
/// GET /me/ai/settings — the AI-assist toggles (all off by default).
pub(crate) async fn get_settings(
    state: web::Data<AppState>,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let settings = settings::get(&state, user_id(&user)?).await?;
    Ok(HttpResponse::Ok().json(settings))
}

#[utoipa::path(
    put, path = "/api/me/ai/settings", operation_id = "aiSetSettings", tag = "ai",
    security(("bearer" = [])), request_body = AiSettings,
    responses((status = 200, body = AiSettings))
)]
/// PUT /me/ai/settings — replace the toggles.
pub(crate) async fn set_settings(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<AiSettings>,
) -> Result<HttpResponse, AppError> {
    let saved = settings::set(&state, user_id(&user)?, *body).await?;
    Ok(HttpResponse::Ok().json(saved))
}

#[utoipa::path(
    post, path = "/api/me/ai/draft-reply", operation_id = "aiDraftReply", tag = "ai",
    security(("bearer" = [])), request_body = AiDraftReplyBody,
    responses((status = 200, body = AiDraft))
)]
/// POST /me/ai/draft-reply — suggested reply for a ticket's QA thread.
pub(crate) async fn draft_reply(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<AiDraftReplyBody>,
) -> Result<HttpResponse, AppError> {
    let uid = user_id(&user)?;
    settings::require_enabled(settings::get(&state, uid).await?.qa_draft_replies)?;

    let issue = crate::jira::data::issue(&state, uid, &body.ticket_key).await.ok();
    let thread =
        slack_messages::list_for_ticket_thread(&state.db, uid, &body.channel_id, &body.thread_ts)
            .await?;
    if thread.is_empty() {
        return Err(AppError::not_found("Unknown Slack thread for this ticket."));
    }
    let transcript: String = thread
        .iter()
        .map(|m| format!("{}: {}", m.author_name, m.body))
        .collect::<Vec<_>>()
        .join("\n");
    let context = issue
        .map(|i| format!("Ticket {} — {}\n{}", i.summary.key, i.summary.summary, i.description))
        .unwrap_or_else(|| format!("Ticket {}", body.ticket_key));

    let system = "You draft Slack replies for a developer answering their QA team. \
        Reply as the developer, in their voice: concise, friendly, concrete. \
        Answer the latest question(s) using the ticket context. If information \
        is missing, say what you'll check. Output ONLY the reply text — no \
        preamble, no quotes.";
    let prompt = format!("## Ticket context\n{context}\n\n## Thread\n{transcript}");
    let text = anthropic::complete(&state, system, &prompt).await?;
    Ok(HttpResponse::Ok().json(draft(text)))
}

#[utoipa::path(
    post, path = "/api/me/ai/handoff", operation_id = "aiHandoff", tag = "ai",
    security(("bearer" = [])), request_body = AiHandoffBody,
    responses((status = 200, body = AiDraft))
)]
/// POST /me/ai/handoff — QA handoff draft for a ticket moving to Testing.
pub(crate) async fn handoff(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<AiHandoffBody>,
) -> Result<HttpResponse, AppError> {
    let uid = user_id(&user)?;
    settings::require_enabled(settings::get(&state, uid).await?.qa_handoff_drafts)?;

    let issue = crate::jira::data::issue(&state, uid, &body.ticket_key).await?;
    let board = crate::hub::board::board(&state, uid).await.ok();
    let card = board.as_ref().and_then(|b| {
        b.columns.iter().flat_map(|c| &c.tickets).find(|t| t.key == body.ticket_key).cloned()
    });
    let code = card
        .map(|c| {
            let prs = c
                .prs
                .iter()
                .map(|p| format!("PR #{} ({}, {}) — {}", p.number, p.repo, p.state, p.title))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "Branch: {}\n{}\nChecks: {}\nDeployed: {}",
                c.branch.unwrap_or_else(|| "—".to_string()),
                prs,
                c.check_state,
                c.deployment.map(|d| d.environment).unwrap_or_else(|| "unknown".to_string()),
            )
        })
        .unwrap_or_else(|| "No linked code found.".to_string());

    let system = "You draft the Slack message a developer posts in the QA channel \
        when handing a ticket to testing. Include: what changed (from the ticket), \
        where to test it (environment/branch), and what to focus on. Use short \
        lines, a few emoji at most. Output ONLY the message text.";
    let prompt = format!(
        "## Ticket {} — {}\n{}\n\n## Code state\n{}",
        issue.summary.key, issue.summary.summary, issue.description, code
    );
    let text = anthropic::complete(&state, system, &prompt).await?;
    Ok(HttpResponse::Ok().json(draft(text)))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/me/ai/settings", web::get().to(get_settings))
        .route("/me/ai/settings", web::put().to(set_settings))
        .route("/me/ai/draft-reply", web::post().to(draft_reply))
        .route("/me/ai/handoff", web::post().to(handoff));
}
