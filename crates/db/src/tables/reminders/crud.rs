//! `reminders` repository: per-note re-sync (insert new hashes, drop removed
//! ones, never touch surviving rows so done/snooze state persists), state
//! mutations, and the due-set reads the hub board/inbox use.

use std::collections::HashMap;

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::ActiveValue::Set;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait, ExprTrait,
    QueryFilter, QueryOrder, QueryResult, QuerySelect, Statement,
};

use super::entity as rem;

pub struct ParsedReminderInput {
    /// Stable content hash of `(ticket_key, body, due_at)`.
    pub id: String,
    pub body: String,
    pub due_at: DateTimeWithTimeZone,
}

/// Re-syncs a ticket's reminders to the freshly parsed set: rows whose hash
/// disappeared are deleted, new hashes are inserted as `pending`, surviving
/// hashes are left untouched (preserving done/snoozed state).
pub async fn sync_for_ticket(
    db: &DatabaseConnection,
    user_id: Uuid,
    ticket_key: &str,
    parsed: Vec<ParsedReminderInput>,
) -> Result<Vec<rem::Model>, DbErr> {
    let keep: Vec<String> = parsed.iter().map(|p| p.id.clone()).collect();
    let mut delete = rem::Entity::delete_many()
        .filter(rem::Column::UserId.eq(user_id))
        .filter(rem::Column::TicketKey.eq(ticket_key));
    if !keep.is_empty() {
        delete = delete.filter(rem::Column::Id.is_not_in(keep));
    }
    delete.exec(db).await?;

    insert_new_reminders(db, user_id, ticket_key, parsed).await?;
    for_ticket(db, user_id, ticket_key).await
}

/// Inserts the freshly parsed reminders as `pending`. Hashes that already exist
/// are skipped via `ON CONFLICT DO NOTHING`, which surfaces as
/// `RecordNotInserted` — a no-op here, not an error.
async fn insert_new_reminders(
    db: &DatabaseConnection,
    user_id: Uuid,
    ticket_key: &str,
    parsed: Vec<ParsedReminderInput>,
) -> Result<(), DbErr> {
    if parsed.is_empty() {
        return Ok(());
    }
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let models = parsed.into_iter().map(|p| rem::ActiveModel {
        user_id: Set(user_id),
        id: Set(p.id),
        ticket_key: Set(ticket_key.to_string()),
        body: Set(p.body),
        due_at: Set(p.due_at),
        state: Set("pending".to_string()),
        snoozed_until: Set(None),
        created_at: Set(now),
    });
    let insert = rem::Entity::insert_many(models).on_conflict(
        OnConflict::columns([rem::Column::UserId, rem::Column::Id])
            .do_nothing()
            .to_owned(),
    );
    match insert.exec(db).await {
        Err(DbErr::RecordNotInserted) => Ok(()),
        other => other.map(|_| ()),
    }
}

pub async fn for_ticket(
    db: &DatabaseConnection,
    user_id: Uuid,
    ticket_key: &str,
) -> Result<Vec<rem::Model>, DbErr> {
    rem::Entity::find()
        .filter(rem::Column::UserId.eq(user_id))
        .filter(rem::Column::TicketKey.eq(ticket_key))
        .order_by_asc(rem::Column::DueAt)
        .all(db)
        .await
}

pub async fn list(
    db: &DatabaseConnection,
    user_id: Uuid,
    state: Option<&str>,
) -> Result<Vec<rem::Model>, DbErr> {
    let mut query = rem::Entity::find().filter(rem::Column::UserId.eq(user_id));
    if let Some(state) = state {
        query = query.filter(rem::Column::State.eq(state));
    }
    query.order_by_asc(rem::Column::DueAt).all(db).await
}

/// Pending reminders that are due now and not snoozed into the future —
/// the inbox's `⏰` items.
pub async fn list_due(
    db: &DatabaseConnection,
    user_id: Uuid,
    limit: u64,
) -> Result<Vec<rem::Model>, DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    rem::Entity::find()
        .filter(rem::Column::UserId.eq(user_id))
        .filter(rem::Column::State.eq("pending"))
        .filter(rem::Column::DueAt.lte(now))
        .filter(
            rem::Column::SnoozedUntil
                .is_null()
                .or(rem::Column::SnoozedUntil.lte(now)),
        )
        .order_by_asc(rem::Column::DueAt)
        .limit(std::cmp::min(limit, 100))
        .all(db)
        .await
}

/// Due-reminder counts per ticket key — the board's `⏰ n` chips.
pub async fn due_counts(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<HashMap<String, i64>, DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        DUE_COUNTS_SQL,
        [user_id.into()],
    );
    db.query_all_raw(stmt)
        .await?
        .into_iter()
        .map(due_count)
        .collect()
}

const DUE_COUNTS_SQL: &str = r#"
SELECT ticket_key, COUNT(*)::bigint AS due
FROM reminders
WHERE user_id = $1 AND state = 'pending' AND due_at <= now()
  AND (snoozed_until IS NULL OR snoozed_until <= now())
GROUP BY ticket_key
"#;

fn due_count(row: QueryResult) -> Result<(String, i64), DbErr> {
    Ok((row.try_get("", "ticket_key")?, row.try_get("", "due")?))
}

pub async fn set_done(
    db: &DatabaseConnection,
    user_id: Uuid,
    id: &str,
) -> Result<Option<rem::Model>, DbErr> {
    rem::Entity::update_many()
        .col_expr(rem::Column::State, Expr::value("done"))
        .filter(rem::Column::UserId.eq(user_id))
        .filter(rem::Column::Id.eq(id))
        .exec(db)
        .await?;
    rem::Entity::find_by_id((user_id, id.to_string())).one(db).await
}

pub async fn snooze(
    db: &DatabaseConnection,
    user_id: Uuid,
    id: &str,
    until: DateTimeWithTimeZone,
) -> Result<Option<rem::Model>, DbErr> {
    rem::Entity::update_many()
        .col_expr(rem::Column::SnoozedUntil, Expr::value(until))
        .filter(rem::Column::UserId.eq(user_id))
        .filter(rem::Column::Id.eq(id))
        .exec(db)
        .await?;
    rem::Entity::find_by_id((user_id, id.to_string())).one(db).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_counts_are_grouped_in_postgres() {
        assert!(DUE_COUNTS_SQL.contains("COUNT(*)::bigint"));
        assert!(DUE_COUNTS_SQL.contains("GROUP BY ticket_key"));
        assert!(DUE_COUNTS_SQL.contains("snoozed_until <= now()"));
    }
}
