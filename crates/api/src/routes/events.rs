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
    /// Filter by `event_type` prefix (e.g. `"pr."` matches `"pr.opened"`).
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
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        payload: m.payload,
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

/// Validate that a `type_prefix` value does not contain unescaped LIKE
/// wildcards. `starts_with` in sea-orm 2.0.0-rc.40 builds `LIKE '<prefix>%'`
/// without escaping input, so a `%` or `_` in the prefix would widen the
/// match silently. Callers supply these from a fixed vocabulary so this only
/// fires on unexpected / adversarial input.
fn validate_prefix(s: &str) -> Result<(), AppError> {
    if s.contains('%') || s.contains('_') {
        return Err(AppError::validation(
            "typePrefix must not contain '%' or '_'",
        ));
    }
    Ok(())
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
    if let Some(prefix) = &query.type_prefix {
        validate_prefix(prefix)?;
    }

    let uid = user_id(&user)?;
    let filter = to_filter(&query);
    let limit = filter.limit as usize;
    let mut rows = events::list_events_page(&state.db, uid, &filter).await?;

    let next_before = if rows.len() > limit {
        let last_id = rows[limit - 1].id;
        rows.truncate(limit);
        Some(last_id)
    } else {
        None
    };

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

    #[test]
    fn validate_prefix_rejects_percent() {
        assert!(validate_prefix("pr.%").is_err());
    }

    #[test]
    fn validate_prefix_rejects_underscore() {
        assert!(validate_prefix("pr._opened").is_err());
    }

    #[test]
    fn validate_prefix_accepts_clean_value() {
        assert!(validate_prefix("pr.").is_ok());
        assert!(validate_prefix("github.").is_ok());
        assert!(validate_prefix("").is_ok());
    }

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

        let q_zero = ListEventsQuery { limit: Some(0), ..ListEventsQuery {
            before: None, limit: None, source: None,
            type_prefix: None, scope_key: None,
        }};
        assert_eq!(to_filter(&q_zero).limit, 1);
    }

    #[test]
    fn next_before_logic_full_page() {
        // Simulate: limit=2, DB returned 3 rows → next_before = id of row[1]
        let limit: usize = 2;
        let ids = [10i64, 9, 8];
        let next_before = if ids.len() > limit {
            Some(ids[limit - 1])
        } else {
            None
        };
        assert_eq!(next_before, Some(9));
    }

    #[test]
    fn next_before_logic_partial_page() {
        // Simulate: limit=2, DB returned 1 row → next_before = None (fewer
        // rows than limit means no further page).
        let limit: usize = 2;
        let row_count: usize = 1;
        let next_before: Option<i64> = if row_count > limit { Some(99) } else { None };
        assert_eq!(next_before, None);
    }
}
