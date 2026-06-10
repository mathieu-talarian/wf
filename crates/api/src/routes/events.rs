//! `GET /api/me/events` (B-lite activity-feed UI spec §2): paged, filtered
//! read of the authenticated user's events, newest-first.

use actix_web::{web, HttpResponse};
use chrono::SecondsFormat;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wf_db::tables::events;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 100;

/// Query parameters for `GET /api/me/events`.
///
/// All fields are optional; omitted fields mean "no filter on that column".
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(rename_all = "camelCase")]
pub(crate) struct ListEventsQuery {
    /// Return only events with `id < before` (cursor for the next page).
    pub before: Option<i64>,
    /// Max events to return (1–100, default 50).
    pub limit: Option<u32>,
    /// Filter by exact `source` value (e.g. `"github"`, `"jira"`).
    pub source: Option<String>,
    /// Filter by `event_type` prefix, matched literally (e.g.
    /// `"github.pull_request."`). LIKE wildcards are escaped in the DB layer.
    pub type_prefix: Option<String>,
    /// Filter by exact `scope_key` value (e.g. a Jira project key).
    pub scope_key: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventDto {
    pub id: i64,
    pub source: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub scope_key: String,
    pub actor: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub occurred_at: String,
    pub payload: serde_json::Value,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventsPage {
    pub events: Vec<EventDto>,
    pub next_before: Option<i64>,
}

fn to_dto(m: events::Model) -> EventDto {
    EventDto {
        id: m.id,
        source: m.source,
        r#type: m.event_type,
        scope_key: m.scope_key,
        actor: m.actor,
        title: m.title,
        url: m.url,
        occurred_at: m
            .occurred_at
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        payload: m.payload,
    }
}

/// Extract the page slice and cursor from a limit+1 probe result.
///
/// `rows` contains at most `limit + 1` rows (fetched by the DB query).
/// Returns `(page_rows, next_before)` where `next_before` is the `id` of the
/// last row kept when a further page exists, or `None` at end-of-feed.
fn page_and_cursor(
    mut rows: Vec<events::Model>,
    limit: usize,
) -> (Vec<events::Model>, Option<i64>) {
    if rows.len() > limit {
        let cursor = rows[limit - 1].id;
        rows.truncate(limit);
        (rows, Some(cursor))
    } else {
        (rows, None)
    }
}

fn to_filter(q: &ListEventsQuery) -> events::ListEventsFilter {
    events::ListEventsFilter {
        before: q.before,
        limit: u64::from(q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)),
        source: q.source.clone(),
        type_prefix: q.type_prefix.clone(),
        scope_key: q.scope_key.clone(),
    }
}

fn user_id(user: &AuthUser) -> Result<Uuid, AppError> {
    Uuid::parse_str(&user.0.id).map_err(|e| AppError::internal(anyhow::anyhow!(e)))
}

#[utoipa::path(
    get,
    path = "/api/me/events",
    operation_id = "listEvents",
    tag = "events",
    security(("bearer" = [])),
    params(ListEventsQuery),
    responses((status = 200, body = EventsPage))
)]
pub(crate) async fn list_events(
    state: web::Data<AppState>,
    user: AuthUser,
    query: web::Query<ListEventsQuery>,
) -> Result<HttpResponse, AppError> {
    let uid = user_id(&user)?;
    let filter = to_filter(&query);
    let limit = filter.limit as usize;
    let rows = events::list_events_page(&state.db, uid, &filter).await?;

    let (rows, next_before) = page_and_cursor(rows, limit);
    let page = EventsPage {
        events: rows.into_iter().map(to_dto).collect(),
        next_before,
    };
    Ok(HttpResponse::Ok().json(page))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/me/events", web::get().to(list_events));
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::prelude::DateTimeWithTimeZone;
    use uuid::Uuid;

    fn make_model(id: i64) -> events::Model {
        let ts: DateTimeWithTimeZone =
            chrono::DateTime::parse_from_rfc3339("2026-06-10T12:00:00.000Z")
                .unwrap();
        events::Model {
            id,
            user_id: Uuid::nil(),
            source: "github".to_string(),
            event_type: "pr.opened".to_string(),
            external_id: "ext-1".to_string(),
            scope_key: "myrepo".to_string(),
            actor: None,
            title: None,
            url: None,
            occurred_at: ts,
            payload: serde_json::Value::Null,
            ingested_at: ts,
        }
    }

    // ── wire-key contract ──────────────────────────────────────────────────

    #[test]
    fn wire_keys_event_dto() {
        let dto = to_dto(make_model(1));
        let v = serde_json::to_value(&dto).unwrap();
        // `event_type` must be serialised as "type" (renamed field)
        assert!(v.get("type").is_some(), "key 'type' missing");
        assert!(v.get("eventType").is_none(), "key 'eventType' must not exist");
        // `scope_key` camelCase rename via #[serde(rename_all = "camelCase")]
        assert!(v.get("scopeKey").is_some(), "key 'scopeKey' missing");
        assert!(v.get("scope_key").is_none(), "key 'scope_key' must not exist");
        // occurred_at: millis + Z suffix
        let occ = v["occurredAt"].as_str().unwrap();
        assert!(occ.ends_with('Z'), "occurredAt must end with Z");
        assert!(occ.contains(".000"), "occurredAt must contain millis '.000'");
    }

    #[test]
    fn wire_keys_events_page_next_before() {
        let page = EventsPage { events: vec![], next_before: Some(5) };
        let v = serde_json::to_value(&page).unwrap();
        assert!(v.get("nextBefore").is_some(), "key 'nextBefore' missing");
        assert!(v.get("next_before").is_none(), "key 'next_before' must not exist");
    }

    // ── page_and_cursor ────────────────────────────────────────────────────

    #[test]
    fn next_before_logic_full_page() {
        // limit=2, probe returned 3 rows → cursor = id of the 2nd row (row[1])
        let rows = vec![make_model(10), make_model(9), make_model(8)];
        let (page, cursor) = page_and_cursor(rows, 2);
        assert_eq!(cursor, Some(9));
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].id, 10);
        assert_eq!(page[1].id, 9);
    }

    #[test]
    fn next_before_logic_partial_page() {
        // limit=2, only 1 row returned → no further page
        let rows = vec![make_model(42)];
        let (page, cursor) = page_and_cursor(rows, 2);
        assert_eq!(cursor, None);
        assert_eq!(page.len(), 1);
    }

    #[test]
    fn next_before_logic_exact_limit() {
        // limit=3, exactly 3 rows → no probe row, no cursor
        let rows = vec![make_model(5), make_model(4), make_model(3)];
        let (page, cursor) = page_and_cursor(rows, 3);
        assert_eq!(cursor, None);
        assert_eq!(page.len(), 3);
    }

    // ── to_filter ──────────────────────────────────────────────────────────

    #[test]
    fn to_filter_defaults_and_clamps() {
        let q = ListEventsQuery {
            before: None,
            limit: None,
            source: None,
            type_prefix: None,
            scope_key: None,
        };
        let f = to_filter(&q);
        assert_eq!(f.limit, 50);

        let q_over = ListEventsQuery { limit: Some(999), ..q };
        assert_eq!(to_filter(&q_over).limit, 100);

        let q_zero = ListEventsQuery {
            limit: Some(0),
            ..ListEventsQuery {
                before: None,
                limit: None,
                source: None,
                type_prefix: None,
                scope_key: None,
            }
        };
        assert_eq!(to_filter(&q_zero).limit, 1);
    }
}
