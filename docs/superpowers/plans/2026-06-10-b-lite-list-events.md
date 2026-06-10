# B-lite: `GET /api/me/events` (listEvents) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Implement the paged/filtered events read endpoint whose contract is pinned in `docs/superpowers/specs/2026-06-10-activity-feed-ui-design.md` §2 — unblocking the activity-feed UI plan. This is the read slice of sub-project B (the `?since=` catch-up and SSE remain for B proper).

**Architecture:** One query-builder read in `wf-db` (`events::list_events_page`), one authed route in `wf-api` (`routes/events.rs`, utoipa-annotated, operationId `listEvents`, tag `events`), regenerated `openapi.json` artifact (CI has a spec-drift check).

**Contract (authoritative, from the feed-UI spec §2):** `GET /api/me/events?before=<i64>&limit=<u32 def 50 max 100>&source=&typePrefix=&scopeKey=` → `200 {"events":[{id, source, type, scopeKey, actor, title, url, occurredAt, payload}], "nextBefore": <i64|null>}` — newest-first (`ORDER BY id DESC`); `nextBefore` = last event's id when the page is full, else `null`; `occurredAt` ISO8601 millis+Z (house convention); camelCase.

**House rules:** clippy gate `cargo clippy --all --all-targets --locked -- -D warnings` (fns ≤25 lines); `#[cfg(test)] mod tests` last item; DTOs `#[serde(rename_all = "camelCase")]`; timestamps `to_rfc3339_opts(SecondsFormat::Millis, true)`; errors via `AppError`; ActiveModel never leaves crud.rs (read-only here, no ActiveModel involved).

---

### Task 1: `list_events_page` in wf-db + the route in wf-api + spec regen

**Files:** Modify: `crates/db/src/tables/events/crud.rs`; Create: `crates/api/src/routes/events.rs`; Modify: `crates/api/src/routes/mod.rs`, `crates/api/src/openapi.rs`; Regenerate: `crates/api/openapi.json` (or wherever the committed artifact lives).

- [ ] **Step 1: wf-db read** — append to `crates/db/src/tables/events/crud.rs` (add `ColumnTrait, QueryFilter, QueryOrder, QuerySelect` to the existing sea_orm imports as needed):

```rust
/// Filters for the paged feed read (B-lite; activity-feed UI spec §2).
#[derive(Debug, Clone, Default)]
pub struct ListEventsFilter {
    pub before: Option<i64>,
    pub limit: u64,
    pub source: Option<String>,
    pub type_prefix: Option<String>,
    pub scope_key: Option<String>,
}

/// Newest-first page of a user's events (`ORDER BY id DESC LIMIT n`), with
/// optional source / type-prefix / scope filters (all indexed access paths).
pub async fn list_events_page(
    db: &DatabaseConnection,
    user_id: Uuid,
    filter: &ListEventsFilter,
) -> Result<Vec<events::Model>, DbErr> {
    let mut q = events::Entity::find().filter(events::Column::UserId.eq(user_id));
    if let Some(before) = filter.before {
        q = q.filter(events::Column::Id.lt(before));
    }
    if let Some(source) = &filter.source {
        q = q.filter(events::Column::Source.eq(source.clone()));
    }
    if let Some(prefix) = &filter.type_prefix {
        q = q.filter(events::Column::EventType.starts_with(prefix.clone()));
    }
    if let Some(scope) = &filter.scope_key {
        q = q.filter(events::Column::ScopeKey.eq(scope.clone()));
    }
    q.order_by_desc(events::Column::Id).limit(filter.limit).all(db).await
}
```

(`starts_with` is sea-orm's LIKE-prefix helper; verify it exists on this rc and that it escapes `%`/`_` in the input — if it does not escape, sanitize the prefix by rejecting `%` and `_` in the handler instead, noting it. The prefix values sent by the UI are from a fixed vocabulary, but the param is public input.)

- [ ] **Step 2: the route** — `crates/api/src/routes/events.rs`. Model the auth-extractor/handler/DTO/utoipa style on `crates/api/src/routes/me.rs` and one GitHub route file (read them first; reuse the exact `AuthUser` usage and error conventions):

```rust
//! `GET /me/events` — paged, filtered read over the unified events table
//! (B-lite; contract pinned by the activity-feed UI spec §2). The `?since=`
//! ascending catch-up and SSE land with sub-project B proper.

use actix_web::{web, HttpResponse};
use chrono::SecondsFormat;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use wf_db::tables::events;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

const DEFAULT_LIMIT: u64 = 50;
const MAX_LIMIT: u64 = 100;

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub(crate) struct ListEventsQuery {
    /// Return events with `id < before` (omit for the newest page).
    before: Option<i64>,
    /// Page size (default 50, max 100).
    limit: Option<u64>,
    /// `github` | `jira`.
    source: Option<String>,
    /// Prefix match on the event type, e.g. `github.pull_request.`.
    type_prefix: Option<String>,
    /// Exact repo full-name or Jira project key.
    scope_key: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventDto {
    id: i64,
    source: String,
    r#type: String,
    scope_key: String,
    actor: Option<String>,
    title: Option<String>,
    url: Option<String>,
    occurred_at: String,
    payload: serde_json::Value,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventsPage {
    events: Vec<EventDto>,
    next_before: Option<i64>,
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
        occurred_at: m.occurred_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        payload: m.payload,
    }
}

fn to_filter(q: &ListEventsQuery) -> events::ListEventsFilter {
    events::ListEventsFilter {
        before: q.before,
        limit: q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        source: q.source.clone(),
        type_prefix: q.type_prefix.clone(),
        scope_key: q.scope_key.clone(),
    }
}

/// `nextBefore` = last id when the page is full (more may exist), else null.
fn next_before(events: &[EventDto], limit: u64) -> Option<i64> {
    (events.len() as u64 == limit).then(|| events.last().map(|e| e.id)).flatten()
}

#[utoipa::path(
    get,
    path = "/api/me/events",
    operation_id = "listEvents",
    tag = "events",
    params(ListEventsQuery),
    responses((status = 200, body = EventsPage)),
    security(("bearer" = []))
)]
pub(crate) async fn list_events(
    user: AuthUser,
    state: web::Data<AppState>,
    query: web::Query<ListEventsQuery>,
) -> Result<HttpResponse, AppError> {
    let filter = to_filter(&query);
    let limit = filter.limit;
    let rows = events::list_events_page(&state.db, user_id_of(&user), &filter).await?;
    let events: Vec<EventDto> = rows.into_iter().map(to_dto).collect();
    let next_before = next_before(&events, limit);
    Ok(HttpResponse::Ok().json(EventsPage { events, next_before }))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/me/events", web::get().to(list_events));
}
```

Adapt: (a) how the user id comes out of `AuthUser` (`user_id_of(&user)` is a placeholder for whatever the other /me routes do — e.g. `user.user_id` or a helper; copy their idiom; if DbErr→AppError needs an explicit map, copy that too); (b) the exact utoipa security scheme name from existing annotated routes (`bearer` may be named differently — grep `security((` in crates/api); (c) `IntoParams` query-casing: verify generated param names come out camelCase (`typePrefix`, `scopeKey`) — `#[serde(rename_all = "camelCase")]` governs actix's deserialization, and utoipa's IntoParams respects serde renames on current versions; confirm in the regenerated spec, and if the spec shows snake_case params, add `#[param(...)]`/`#[into_params(rename_all = "camelCase")]` as needed so BOTH the runtime and the spec agree on camelCase.

- [ ] **Step 3: wire** — `routes/mod.rs`: `pub mod events;` + call `events::configure(cfg)` inside the /api-scoped `configure` (this endpoint IS part of the public API, unlike `internal`). `openapi.rs`: add the path fn + the two schemas to the utoipa derive lists, following how existing routes register.
- [ ] **Step 4: unit tests** (last item of events.rs): `next_before` full-page vs short-page vs empty; `to_filter` clamps limit 0→1, 1000→100, default 50; `to_dto` maps `type`/`occurredAt` format (construct a Model literal; assert serialized JSON keys `type`, `scopeKey`, `occurredAt` ends with `Z`).
- [ ] **Step 5: regenerate the openapi.json artifact** — find the existing regeneration flow (commit ec2cdec added the artifact + CI drift check; look for a script, test, or bin that writes/compares it — e.g. in scripts/ or an api test). Regenerate; verify the diff adds exactly one path (`/api/me/events`, operationId `listEvents`, camelCase params) and two schemas. The CI drift check must pass.
- [ ] **Step 6: gates** — `cargo test --workspace` and `cargo clippy --all --all-targets --locked -- -D warnings`.
- [ ] **Step 7: commit** — `git add crates/db crates/api && git commit -m "B-lite: GET /api/me/events (listEvents) — paged, filtered feed read"` (include the openapi.json artifact path in the add; never `git add -A`).

### Task 2: Live verification

- [ ] Boot `cargo run -p wf-api`; without a JWT expect 401 problem+json from `/api/me/events`. (A full authed smoke needs a real Supabase token — the feed UI's Task 8 manual pass is the end-to-end verification; don't block on it here.)
- [ ] In `../workflow`: `yarn api:spec && grep -c listEvents openapi.json` → ≥1 (the FE plan's Task 1 gate now passes). Do not run `yarn api:gen` or change the FE repo — that belongs to the FE plan's execution.
