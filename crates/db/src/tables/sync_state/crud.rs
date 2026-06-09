//! `sync_state` repository (A1 spec §3.2/§4.2): scope reconciliation, atomic
//! lease-claim of due rows (`FOR UPDATE SKIP LOCKED`), owner-guarded
//! completion (a stale tick can't overwrite a re-claimed row), and the
//! opportunistic "mark overdue" hook (spec §4.3).

use sea_orm::prelude::Uuid;
use sea_orm::sea_query::OnConflict;
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, Statement,
};

use super::entity as sync_state;

/// One desired pollable scope: `(scope_key, entity_kind)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeKey {
    pub scope_key: String,
    pub entity_kind: String,
}

/// Makes `sync_state` for `(user, source)` hold exactly the `desired` scopes:
/// inserts missing rows (due immediately) and deletes de-selected ones.
pub async fn replace_scopes(
    db: &DatabaseConnection,
    user_id: Uuid,
    source: &str,
    desired: &[ScopeKey],
) -> Result<(), DbErr> {
    let existing = existing_scopes(db, user_id, source).await?;
    insert_missing(db, user_id, source, desired, &existing).await?;
    delete_deselected(db, user_id, source, desired, &existing).await
}

async fn existing_scopes(
    db: &DatabaseConnection,
    user_id: Uuid,
    source: &str,
) -> Result<Vec<ScopeKey>, DbErr> {
    let rows = sync_state::Entity::find()
        .filter(sync_state::Column::UserId.eq(user_id))
        .filter(sync_state::Column::Source.eq(source))
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| ScopeKey { scope_key: r.scope_key, entity_kind: r.entity_kind })
        .collect())
}

async fn insert_missing(
    db: &DatabaseConnection,
    user_id: Uuid,
    source: &str,
    desired: &[ScopeKey],
    existing: &[ScopeKey],
) -> Result<(), DbErr> {
    let missing: Vec<_> = desired
        .iter()
        .filter(|s| !existing.contains(s))
        .map(|s| new_scope_model(user_id, source, s))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    sync_state::Entity::insert_many(missing)
        .on_conflict(conflict_pk().do_nothing().to_owned())
        .exec_without_returning(db)
        .await
        .map(|_| ())
}

async fn delete_deselected(
    db: &DatabaseConnection,
    user_id: Uuid,
    source: &str,
    desired: &[ScopeKey],
    existing: &[ScopeKey],
) -> Result<(), DbErr> {
    for gone in existing.iter().filter(|s| !desired.contains(s)) {
        sync_state::Entity::delete_by_id((
            user_id,
            source.to_string(),
            gone.scope_key.clone(),
            gone.entity_kind.clone(),
        ))
        .exec(db)
        .await?;
    }
    Ok(())
}

fn conflict_pk() -> OnConflict {
    OnConflict::columns([
        sync_state::Column::UserId,
        sync_state::Column::Source,
        sync_state::Column::ScopeKey,
        sync_state::Column::EntityKind,
    ])
}

/// The only `ActiveModel` construction site: a fresh scope, due immediately.
fn new_scope_model(user_id: Uuid, source: &str, s: &ScopeKey) -> sync_state::ActiveModel {
    sync_state::ActiveModel {
        user_id: Set(user_id),
        source: Set(source.to_string()),
        scope_key: Set(s.scope_key.clone()),
        entity_kind: Set(s.entity_kind.clone()),
        cursor: Set(None),
        last_polled_at: Set(None),
        next_poll_at: NotSet, // DB default now()
        consecutive_errors: Set(0),
        last_error: Set(None),
        lease_owner: Set(None),
        lease_until: Set(None),
    }
}

/// Atomically claims up to `batch` due, unleased (or lease-expired) scopes for
/// `owner`, leasing them for `lease_secs` (spec §4.2 step 2).
pub async fn claim_due(
    db: &DatabaseConnection,
    batch: u64,
    owner: &str,
    lease_secs: u64,
) -> Result<Vec<sync_state::Model>, DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"WITH due AS (
             SELECT user_id, source, scope_key, entity_kind FROM sync_state
             WHERE next_poll_at <= now()
               AND (lease_until IS NULL OR lease_until < now())
             ORDER BY next_poll_at LIMIT $1
             FOR UPDATE SKIP LOCKED)
           UPDATE sync_state s
           SET lease_owner = $2, lease_until = now() + make_interval(secs => $3)
           FROM due d
           WHERE (s.user_id, s.source, s.scope_key, s.entity_kind)
               = (d.user_id, d.source, d.scope_key, d.entity_kind)
           RETURNING s.*"#,
        [(batch as i64).into(), owner.into(), (lease_secs as f64).into()],
    );
    let rows = db.query_all_raw(stmt).await?;
    rows.iter().map(|r| sync_state::Model::from_query_result(r, "")).collect()
}

/// Success completion: store the (possibly advanced) cursor, reset errors,
/// clear the lease, schedule the next poll. Owner-guarded (spec §4.2 step 4).
pub async fn complete_ok(
    db: &DatabaseConnection,
    row: &sync_state::Model,
    owner: &str,
    new_cursor: Option<String>,
    next_in_secs: u64,
) -> Result<(), DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"UPDATE sync_state
           SET "cursor" = $5, last_polled_at = now(),
               next_poll_at = now() + make_interval(secs => $6),
               consecutive_errors = 0, last_error = NULL,
               lease_owner = NULL, lease_until = NULL
           WHERE user_id = $1 AND source = $2 AND scope_key = $3
             AND entity_kind = $4 AND lease_owner = $7"#,
        [
            row.user_id.into(),
            row.source.clone().into(),
            row.scope_key.clone().into(),
            row.entity_kind.clone().into(),
            new_cursor.into(),
            (next_in_secs as f64).into(),
            owner.into(),
        ],
    );
    db.execute_raw(stmt).await.map(|_| ())
}

/// Failure completion: record the error, bump `consecutive_errors`, back off
/// by `next_in_secs` (caller computes the backoff), clear the lease.
pub async fn complete_err(
    db: &DatabaseConnection,
    row: &sync_state::Model,
    owner: &str,
    error: &str,
    next_in_secs: u64,
) -> Result<(), DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"UPDATE sync_state
           SET last_polled_at = now(),
               next_poll_at = now() + make_interval(secs => $6),
               consecutive_errors = consecutive_errors + 1, last_error = $5,
               lease_owner = NULL, lease_until = NULL
           WHERE user_id = $1 AND source = $2 AND scope_key = $3
             AND entity_kind = $4 AND lease_owner = $7"#,
        [
            row.user_id.into(),
            row.source.clone().into(),
            row.scope_key.clone().into(),
            row.entity_kind.clone().into(),
            error.into(),
            (next_in_secs as f64).into(),
            owner.into(),
        ],
    );
    db.execute_raw(stmt).await.map(|_| ())
}

/// Opportunistic hook (spec §4.3): pull every scope of this user forward to
/// "due now" so the next tick prioritizes them. No-op for already-due rows.
pub async fn mark_user_due(db: &DatabaseConnection, user_id: Uuid) -> Result<(), DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE sync_state SET next_poll_at = now() \
         WHERE user_id = $1 AND next_poll_at > now()",
        [user_id.into()],
    );
    db.execute_raw(stmt).await.map(|_| ())
}
