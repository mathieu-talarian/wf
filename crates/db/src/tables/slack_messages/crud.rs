//! `slack_messages` repository: idempotent ingest (sync tick), per-ticket
//! thread reads, unread accounting for the hub board/inbox, and mark-read.

use std::collections::HashMap;

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
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
    pub posted_at: DateTimeWithTimeZone,
}

/// Inserts a batch of polled messages; re-polled rows update their mutable
/// fields (edits, late ticket-key matches) but never reset `is_read`.
/// Returns the number of rows written.
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
    let models = inputs.into_iter().map(|input| msg::ActiveModel {
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
        is_read: Set(false),
        posted_at: Set(input.posted_at),
        ingested_at: Set(now),
    });
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

/// Unread (non-bot) message counts per ticket key — the board's `💬 n unread`.
pub async fn unread_counts(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<HashMap<String, i64>, DbErr> {
    let keys: Vec<Option<String>> = msg::Entity::find()
        .select_only()
        .column(msg::Column::TicketKey)
        .filter(msg::Column::UserId.eq(user_id))
        .filter(msg::Column::IsRead.eq(false))
        .filter(msg::Column::IsBot.eq(false))
        .filter(msg::Column::TicketKey.is_not_null())
        .into_tuple()
        .all(db)
        .await?;
    let mut counts = HashMap::new();
    for key in keys.into_iter().flatten() {
        *counts.entry(key).or_insert(0) += 1;
    }
    Ok(counts)
}

/// Latest unread non-bot messages (newest first) — the inbox's QA items.
pub async fn list_unread(
    db: &DatabaseConnection,
    user_id: Uuid,
    limit: u64,
) -> Result<Vec<msg::Model>, DbErr> {
    msg::Entity::find()
        .filter(msg::Column::UserId.eq(user_id))
        .filter(msg::Column::IsRead.eq(false))
        .filter(msg::Column::IsBot.eq(false))
        .filter(msg::Column::TicketKey.is_not_null())
        .order_by_desc(msg::Column::PostedAt)
        .limit(limit)
        .all(db)
        .await
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
