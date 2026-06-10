//! `events` repository (A1 spec §3.1/§6): batch insert with per-user dedup via
//! `ON CONFLICT (user_id, source, external_id) DO NOTHING`, plus the latest-
//! status lookup the Jira normalizer needs (spec §5.1), and the paged feed
//! read (B-lite; activity-feed UI spec §2).

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::OnConflict;
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Select, Statement,
};

use super::entity as events;

/// Typed input for one event row (the only `ActiveModel` construction site).
#[derive(Debug, Clone)]
pub struct InsertEventInput {
    pub user_id: Uuid,
    pub source: String,
    pub event_type: String,
    pub external_id: String,
    pub scope_key: String,
    pub actor: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub occurred_at: DateTimeWithTimeZone,
    pub payload: serde_json::Value,
}

/// Inserts a batch, silently skipping rows whose `(user_id, source,
/// external_id)` already exist. Returns the number of rows actually written.
pub async fn insert_ignore_dups(
    db: &DatabaseConnection,
    inputs: Vec<InsertEventInput>,
) -> Result<u64, DbErr> {
    if inputs.is_empty() {
        return Ok(0);
    }
    let models = inputs.into_iter().map(active_model);
    events::Entity::insert_many(models)
        .on_conflict(
            OnConflict::columns([
                events::Column::UserId,
                events::Column::Source,
                events::Column::ExternalId,
            ])
            .do_nothing()
            .to_owned(),
        )
        .exec_without_returning(db)
        .await
}

/// Latest ingested Jira `statusId` for an issue (same user, same project
/// scope) — the Jira normalizer's "previous status" input (spec §5.1).
pub async fn latest_jira_status(
    db: &DatabaseConnection,
    user_id: Uuid,
    scope_key: &str,
    issue_key: &str,
) -> Result<Option<String>, DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT payload->>'statusId' AS status_id FROM events \
         WHERE user_id = $1 AND source = 'jira' AND scope_key = $2 \
           AND payload->>'issueKey' = $3 ORDER BY id DESC LIMIT 1",
        [user_id.into(), scope_key.into(), issue_key.into()],
    );
    let row = db.query_one_raw(stmt).await?;
    match row {
        None => Ok(None),
        Some(r) => r.try_get::<Option<String>>("", "status_id"),
    }
}

/// Filter parameters for [`list_events_page`].
pub struct ListEventsFilter {
    /// Return only events with `id < before` (cursor).
    pub before: Option<i64>,
    /// Max rows to return (caller must clamp; DB adds one to detect next page).
    pub limit: u64,
    /// Exact match on `source`.
    pub source: Option<String>,
    /// `event_type LIKE '<prefix>%'` with the prefix matched **literally**:
    /// LIKE wildcards (`%`, `_`, `\`) are escaped here. Real event types
    /// contain `_` (`github.pull_request.`), so rejecting them upstream is
    /// not an option.
    pub type_prefix: Option<String>,
    /// Exact match on `scope_key`.
    pub scope_key: Option<String>,
}

/// Escapes LIKE wildcards so a prefix matches literally. Postgres's default
/// LIKE escape character is `\` (no `ESCAPE` clause needed).
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// Builds the [`list_events_page`] query (shared with the SQL-shape tests so
/// they exercise the production builder, not a copy).
fn list_events_query(user_id: Uuid, filter: &ListEventsFilter) -> Select<events::Entity> {
    let mut q = events::Entity::find().filter(events::Column::UserId.eq(user_id));

    if let Some(before) = filter.before {
        q = q.filter(events::Column::Id.lt(before));
    }
    if let Some(source) = &filter.source {
        q = q.filter(events::Column::Source.eq(source.clone()));
    }
    if let Some(prefix) = &filter.type_prefix {
        q = q.filter(events::Column::EventType.starts_with(escape_like(prefix)));
    }
    if let Some(scope) = &filter.scope_key {
        q = q.filter(events::Column::ScopeKey.eq(scope.clone()));
    }

    q.order_by_desc(events::Column::Id).limit(filter.limit + 1)
}

/// Returns up to `filter.limit` events for `user_id`, newest-first.
///
/// Fetches `limit + 1` rows; if a full extra row comes back the caller
/// should set `next_before` to the last normal row's `id` and drop the extra.
pub async fn list_events_page(
    db: &DatabaseConnection,
    user_id: Uuid,
    filter: &ListEventsFilter,
) -> Result<Vec<events::Model>, DbErr> {
    list_events_query(user_id, filter).all(db).await
}

/// Builds the `ActiveModel` for [`insert_ignore_dups`]; `id`/`ingested_at`
/// stay `NotSet` (identity + DB default).
fn active_model(input: InsertEventInput) -> events::ActiveModel {
    events::ActiveModel {
        id: NotSet,
        user_id: Set(input.user_id),
        source: Set(input.source),
        event_type: Set(input.event_type),
        external_id: Set(input.external_id),
        scope_key: Set(input.scope_key),
        actor: Set(input.actor),
        title: Set(input.title),
        url: Set(input.url),
        occurred_at: Set(input.occurred_at),
        payload: Set(input.payload),
        ingested_at: NotSet,
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{DbBackend, QueryTrait};

    use super::*;

    fn base_filter() -> ListEventsFilter {
        ListEventsFilter {
            before: None,
            limit: 50,
            source: None,
            type_prefix: None,
            scope_key: None,
        }
    }

    /// Renders the production query builder to SQL without executing it,
    /// so the SQL-shape tests need no DB.
    fn sql_for(user_id: Uuid, filter: &ListEventsFilter) -> String {
        list_events_query(user_id, filter).build(DbBackend::Postgres).to_string()
    }

    #[test]
    fn no_filters_orders_desc_and_fetches_limit_plus_one() {
        let uid = Uuid::new_v4();
        let sql = sql_for(uid, &base_filter());
        assert!(sql.contains("ORDER BY \"events\".\"id\" DESC"));
        assert!(sql.contains("LIMIT 51"));
        assert!(sql.contains(&uid.to_string()));
    }

    #[test]
    fn before_filter_adds_lt_clause() {
        let uid = Uuid::new_v4();
        let filter = ListEventsFilter { before: Some(999), ..base_filter() };
        let sql = sql_for(uid, &filter);
        assert!(sql.contains("\"id\" < 999"));
    }

    #[test]
    fn source_filter_adds_eq_clause() {
        let uid = Uuid::new_v4();
        let filter = ListEventsFilter { source: Some("github".into()), ..base_filter() };
        let sql = sql_for(uid, &filter);
        assert!(sql.contains("\"source\" = 'github'"));
    }

    #[test]
    fn type_prefix_produces_like_pattern() {
        let uid = Uuid::new_v4();
        let filter = ListEventsFilter { type_prefix: Some("pr.".into()), ..base_filter() };
        let sql = sql_for(uid, &filter);
        assert!(sql.contains("LIKE 'pr.%'"));
    }

    #[test]
    fn type_prefix_escapes_like_wildcards() {
        // Real event types contain `_` (github.pull_request.*); the pattern
        // must match it literally, not as a single-char wildcard.
        let uid = Uuid::new_v4();
        let filter =
            ListEventsFilter { type_prefix: Some("github.pull_request.".into()), ..base_filter() };
        // sea_query renders strings containing `\` as Postgres E-strings,
        // doubling each backslash — `E'...\\_...'` reaches LIKE as `\_`.
        let sql = sql_for(uid, &filter);
        assert!(sql.contains(r"LIKE E'github.pull\\_request.%'"), "got: {sql}");

        let hostile = ListEventsFilter { type_prefix: Some("%_\\".into()), ..base_filter() };
        let sql = sql_for(uid, &hostile);
        assert!(sql.contains(r"LIKE E'\\%\\_\\\\%'"), "got: {sql}");
    }

    #[test]
    fn scope_key_filter_adds_eq_clause() {
        let uid = Uuid::new_v4();
        let filter = ListEventsFilter { scope_key: Some("PROJ".into()), ..base_filter() };
        let sql = sql_for(uid, &filter);
        assert!(sql.contains("\"scope_key\" = 'PROJ'"));
    }
}
