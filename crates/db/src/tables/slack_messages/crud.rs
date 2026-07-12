//! `slack_messages` repository: idempotent ingest (sync tick), per-ticket
//! thread reads, unread accounting for the hub board/inbox, and mark-read.

use std::collections::HashMap;

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait, QueryFilter,
    QueryOrder, QueryResult, Statement,
};

use super::entity as msg;

pub struct UpsertSlackMessageInput {
    pub channel_id: String,
    pub channel_name: String,
    pub ts: String,
    pub thread_ts: String,
    pub author_id: String,
    pub author_name: String,
    pub author_avatar_url: Option<String>,
    pub is_bot: bool,
    pub body: String,
    pub ticket_key: Option<String>,
    /// Baseline backfills insert as read (no inbox flood); live polls as unread.
    pub is_read: bool,
    pub posted_at: DateTimeWithTimeZone,
}

#[derive(Debug, Clone)]
pub struct UnreadTicketSummary {
    pub ticket_key: String,
    pub channel_id: String,
    pub channel_name: String,
    pub ts: String,
    pub thread_ts: String,
    pub body: String,
    pub posted_at: DateTimeWithTimeZone,
    pub unread: i64,
}

/// Inserts a batch of polled messages; re-polled rows update their mutable
/// fields (edits, late ticket-key matches) but never reset `is_read`.
/// Returns the number of rows written.
/// Builds one row from a polled message. `id` is `NotSet` (DB-assigned); the
/// caller's `on_conflict` decides which columns a re-poll overwrites.
fn build_active_model(
    user_id: Uuid,
    input: UpsertSlackMessageInput,
    now: DateTimeWithTimeZone,
) -> msg::ActiveModel {
    msg::ActiveModel {
        id: NotSet,
        user_id: Set(user_id),
        channel_id: Set(input.channel_id),
        channel_name: Set(input.channel_name),
        ts: Set(input.ts),
        thread_ts: Set(input.thread_ts),
        author_id: Set(input.author_id),
        author_name: Set(input.author_name),
        author_avatar_url: Set(input.author_avatar_url),
        is_bot: Set(input.is_bot),
        body: Set(input.body),
        ticket_key: Set(input.ticket_key),
        is_read: Set(input.is_read),
        posted_at: Set(input.posted_at),
        ingested_at: Set(now),
    }
}

pub async fn upsert_many(
    db: &DatabaseConnection,
    user_id: Uuid,
    inputs: Vec<UpsertSlackMessageInput>,
) -> Result<u64, DbErr> {
    if inputs.is_empty() {
        return Ok(0);
    }
    let count = inputs.len() as u64;
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let models = inputs
        .into_iter()
        .map(|input| build_active_model(user_id, input, now));
    msg::Entity::insert_many(models)
        .on_conflict(
            OnConflict::columns([msg::Column::UserId, msg::Column::ChannelId, msg::Column::Ts])
                .update_columns([
                    msg::Column::ChannelName,
                    msg::Column::AuthorName,
                    msg::Column::AuthorAvatarUrl,
                    msg::Column::Body,
                    msg::Column::TicketKey,
                ])
                .to_owned(),
        )
        .exec(db)
        .await
        .map(|_| count)
}

/// All messages matched to a ticket, oldest first (the API layer groups them
/// into threads by `thread_ts`).
pub async fn list_for_ticket(
    db: &DatabaseConnection,
    user_id: Uuid,
    ticket_key: &str,
) -> Result<Vec<msg::Model>, DbErr> {
    msg::Entity::find()
        .filter(msg::Column::UserId.eq(user_id))
        .filter(msg::Column::TicketKey.eq(ticket_key))
        .order_by_asc(msg::Column::PostedAt)
        .all(db)
        .await
}

/// One thread's rows (any ticket), oldest first — used to inherit channel
/// name / ticket key when echoing a posted reply.
pub async fn list_for_ticket_thread(
    db: &DatabaseConnection,
    user_id: Uuid,
    channel_id: &str,
    thread_ts: &str,
) -> Result<Vec<msg::Model>, DbErr> {
    msg::Entity::find()
        .filter(msg::Column::UserId.eq(user_id))
        .filter(msg::Column::ChannelId.eq(channel_id))
        .filter(msg::Column::ThreadTs.eq(thread_ts))
        .order_by_asc(msg::Column::PostedAt)
        .all(db)
        .await
}

/// Unread (non-bot) message counts per ticket key — the board's `💬 n unread`.
pub async fn unread_counts(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<HashMap<String, i64>, DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        UNREAD_COUNTS_SQL,
        [user_id.into()],
    );
    db.query_all_raw(stmt)
        .await?
        .into_iter()
        .map(unread_count)
        .collect()
}

const UNREAD_COUNTS_SQL: &str = r#"
SELECT ticket_key, COUNT(*)::bigint AS unread
FROM slack_messages
WHERE user_id = $1 AND is_read = false AND is_bot = false AND ticket_key IS NOT NULL
GROUP BY ticket_key
"#;

fn unread_count(row: QueryResult) -> Result<(String, i64), DbErr> {
    Ok((row.try_get("", "ticket_key")?, row.try_get("", "unread")?))
}

/// Latest unread non-bot message and count for each ticket.
pub async fn unread_ticket_summaries(
    db: &DatabaseConnection,
    user_id: Uuid,
    limit: u64,
) -> Result<Vec<UnreadTicketSummary>, DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        UNREAD_TICKETS_SQL,
        [user_id.into(), (limit.min(100) as i64).into()],
    );
    db.query_all_raw(stmt).await?.into_iter().map(unread_ticket).collect()
}

const UNREAD_TICKETS_SQL: &str = r#"
WITH ranked AS (
  SELECT ticket_key, channel_id, channel_name, ts, thread_ts, body, posted_at,
         COUNT(*) OVER (PARTITION BY ticket_key)::bigint AS unread,
         ROW_NUMBER() OVER (PARTITION BY ticket_key ORDER BY posted_at DESC, id DESC) AS rank
  FROM slack_messages
  WHERE user_id = $1 AND is_read = false AND is_bot = false AND ticket_key IS NOT NULL
)
SELECT ticket_key, channel_id, channel_name, ts, thread_ts, body, posted_at, unread
FROM ranked WHERE rank = 1 ORDER BY posted_at DESC LIMIT $2
"#;

fn unread_ticket(row: QueryResult) -> Result<UnreadTicketSummary, DbErr> {
    Ok(UnreadTicketSummary {
        ticket_key: row.try_get("", "ticket_key")?,
        channel_id: row.try_get("", "channel_id")?,
        channel_name: row.try_get("", "channel_name")?,
        ts: row.try_get("", "ts")?,
        thread_ts: row.try_get("", "thread_ts")?,
        body: row.try_get("", "body")?,
        posted_at: row.try_get("", "posted_at")?,
        unread: row.try_get("", "unread")?,
    })
}

pub async fn mark_read(
    db: &DatabaseConnection,
    user_id: Uuid,
    ticket_key: &str,
) -> Result<(), DbErr> {
    msg::Entity::update_many()
        .col_expr(msg::Column::IsRead, Expr::value(true))
        .filter(msg::Column::UserId.eq(user_id))
        .filter(msg::Column::TicketKey.eq(ticket_key))
        .exec(db)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_queries_aggregate_inside_postgres() {
        assert!(UNREAD_COUNTS_SQL.contains("GROUP BY ticket_key"));
        assert!(UNREAD_TICKETS_SQL.contains("COUNT(*) OVER (PARTITION BY ticket_key)"));
        assert!(UNREAD_TICKETS_SQL.contains("ROW_NUMBER() OVER"));
        assert!(UNREAD_TICKETS_SQL.contains("WHERE rank = 1"));
    }
}
