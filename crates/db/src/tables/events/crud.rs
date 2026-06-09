//! `events` repository (A1 spec §3.1/§6): batch insert with per-user dedup via
//! `ON CONFLICT (user_id, source, external_id) DO NOTHING`, plus the latest-
//! status lookup the Jira normalizer needs (spec §5.1).

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::OnConflict;
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait, Statement};

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
