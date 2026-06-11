//! SeaORM entity for `slack_messages` — matches `migration::m0003`. One row
//! per (user, channel, ts); `ticket_key` is the Jira key matched in the text
//! by the sync normalizer (null when unmatched), `is_read` is our own per-user
//! read cursor (bot tokens have none).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "slack_messages")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: Uuid,
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
    pub is_read: bool,
    pub posted_at: DateTimeWithTimeZone,
    pub ingested_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
