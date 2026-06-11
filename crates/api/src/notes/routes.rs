//! Notes + reminders routes (`notes` tag). On every save the body is re-parsed
//! and the derived `reminders` / `note_links` rows are re-synced.

use actix_web::{web, HttpResponse};
use sea_orm::prelude::Uuid;
use serde::{Deserialize, Serialize};
use wf_db::tables::{note_links, notes, reminders};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::notes::parse;
use crate::state::AppState;

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NoteDetail {
    ticket_key: String,
    body: String,
    updated_at: Option<String>,
    backlinks: Vec<NoteBacklink>,
    reminders: Vec<Reminder>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NoteBacklink {
    ticket_key: String,
    snippet: String,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Reminder {
    id: String,
    ticket_key: String,
    text: String,
    due_at: String,
    /// `pending` | `done`
    state: String,
    snoozed_until: Option<String>,
    created_at: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PutNoteBody {
    ticket_key: String,
    body: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct ReminderRefBody {
    id: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct ReminderSnoozeBody {
    id: String,
    /// ISO 8601 instant.
    until: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NoteQuery {
    ticket_key: String,
}

#[derive(Deserialize)]
pub(crate) struct PutNoteQuery {
    /// IANA timezone for `⏰` parsing; UTC when absent/unknown.
    tz: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct RemindersQuery {
    state: Option<String>,
}

fn user_id(user: &AuthUser) -> Result<Uuid, AppError> {
    Uuid::parse_str(&user.0.id).map_err(|e| AppError::internal(anyhow::anyhow!(e)))
}

pub(crate) fn reminder_of(row: reminders::Model) -> Reminder {
    Reminder {
        id: row.id,
        ticket_key: row.ticket_key,
        text: row.body,
        due_at: row.due_at.to_rfc3339(),
        state: row.state,
        snoozed_until: row.snoozed_until.map(|t| t.to_rfc3339()),
        created_at: row.created_at.to_rfc3339(),
    }
}

async fn note_detail(
    state: &AppState,
    user_id: Uuid,
    ticket_key: &str,
) -> Result<NoteDetail, AppError> {
    let note = notes::get(&state.db, user_id, ticket_key).await?;
    let backlinks = note_links::backlinks(&state.db, user_id, ticket_key)
        .await?
        .into_iter()
        .map(|l| NoteBacklink { ticket_key: l.from_ticket, snippet: l.snippet })
        .collect();
    let reminder_rows = reminders::for_ticket(&state.db, user_id, ticket_key).await?;
    Ok(NoteDetail {
        ticket_key: ticket_key.to_string(),
        body: note.as_ref().map(|n| n.body.clone()).unwrap_or_default(),
        updated_at: note.map(|n| n.updated_at.to_rfc3339()),
        backlinks,
        reminders: reminder_rows.into_iter().map(reminder_of).collect(),
    })
}

#[utoipa::path(
    get, path = "/api/me/notes", operation_id = "getNote", tag = "notes",
    security(("bearer" = [])),
    params(("ticketKey" = String, Query, description = "Jira ticket key")),
    responses((status = 200, body = NoteDetail))
)]
/// GET /me/notes?ticketKey= — the ticket's note (empty body when none).
pub(crate) async fn get_note(
    state: web::Data<AppState>,
    user: AuthUser,
    query: web::Query<NoteQuery>,
) -> Result<HttpResponse, AppError> {
    let detail = note_detail(&state, user_id(&user)?, &query.ticket_key).await?;
    Ok(HttpResponse::Ok().json(detail))
}

#[utoipa::path(
    put, path = "/api/me/notes", operation_id = "putNote", tag = "notes",
    security(("bearer" = [])), request_body = PutNoteBody,
    params(("tz" = Option<String>, Query, description = "IANA timezone for ⏰ parsing")),
    responses((status = 200, body = NoteDetail))
)]
/// PUT /me/notes?tz= — upsert the note and re-sync derived reminders/links.
pub(crate) async fn put_note(
    state: web::Data<AppState>,
    user: AuthUser,
    query: web::Query<PutNoteQuery>,
    body: web::Json<PutNoteBody>,
) -> Result<HttpResponse, AppError> {
    let user_id = user_id(&user)?;
    let tz: chrono_tz::Tz =
        query.tz.as_deref().and_then(|t| t.parse().ok()).unwrap_or(chrono_tz::UTC);

    notes::upsert(&state.db, user_id, &body.ticket_key, &body.body).await?;

    let links = parse::parse_links(&body.ticket_key, &body.body);
    note_links::replace_for_ticket(&state.db, user_id, &body.ticket_key, links).await?;

    let parsed = parse::parse_reminders(&body.ticket_key, &body.body, tz, chrono::Utc::now());
    let inputs = parsed
        .into_iter()
        .map(|p| reminders::ParsedReminderInput {
            id: p.id,
            body: p.body,
            due_at: p.due_at.into(),
        })
        .collect();
    reminders::sync_for_ticket(&state.db, user_id, &body.ticket_key, inputs).await?;

    let detail = note_detail(&state, user_id, &body.ticket_key).await?;
    Ok(HttpResponse::Ok().json(detail))
}

#[utoipa::path(
    get, path = "/api/me/reminders", operation_id = "listReminders", tag = "notes",
    security(("bearer" = [])),
    params(("state" = Option<String>, Query, description = "pending | done")),
    responses((status = 200, body = [Reminder]))
)]
/// GET /me/reminders?state= — the user's reminders, due-date ascending.
pub(crate) async fn list_reminders(
    state: web::Data<AppState>,
    user: AuthUser,
    query: web::Query<RemindersQuery>,
) -> Result<HttpResponse, AppError> {
    let rows = reminders::list(&state.db, user_id(&user)?, query.state.as_deref()).await?;
    let out: Vec<Reminder> = rows.into_iter().map(reminder_of).collect();
    Ok(HttpResponse::Ok().json(out))
}

#[utoipa::path(
    post, path = "/api/me/reminders/done", operation_id = "reminderDone", tag = "notes",
    security(("bearer" = [])), request_body = ReminderRefBody,
    responses((status = 200, body = Reminder))
)]
/// POST /me/reminders/done — mark a reminder done.
pub(crate) async fn reminder_done(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<ReminderRefBody>,
) -> Result<HttpResponse, AppError> {
    let row = reminders::set_done(&state.db, user_id(&user)?, &body.id)
        .await?
        .ok_or_else(|| AppError::not_found("Unknown reminder id."))?;
    Ok(HttpResponse::Ok().json(reminder_of(row)))
}

#[utoipa::path(
    post, path = "/api/me/reminders/snooze", operation_id = "reminderSnooze", tag = "notes",
    security(("bearer" = [])), request_body = ReminderSnoozeBody,
    responses((status = 200, body = Reminder))
)]
/// POST /me/reminders/snooze — suppress a reminder until the given instant.
pub(crate) async fn reminder_snooze(
    state: web::Data<AppState>,
    user: AuthUser,
    body: web::Json<ReminderSnoozeBody>,
) -> Result<HttpResponse, AppError> {
    let until = chrono::DateTime::parse_from_rfc3339(&body.until)
        .map_err(|_| AppError::validation("`until` must be an ISO 8601 instant."))?;
    let row = reminders::snooze(&state.db, user_id(&user)?, &body.id, until.into())
        .await?
        .ok_or_else(|| AppError::not_found("Unknown reminder id."))?;
    Ok(HttpResponse::Ok().json(reminder_of(row)))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/me/notes", web::get().to(get_note))
        .route("/me/notes", web::put().to(put_note))
        .route("/me/reminders", web::get().to(list_reminders))
        .route("/me/reminders/done", web::post().to(reminder_done))
        .route("/me/reminders/snooze", web::post().to(reminder_snooze));
}
