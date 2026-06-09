//! SeaORM entity for `events` (A1 spec §3.1) — matches `migration::m0001`.
//! `id` is a Postgres identity column (the feed cursor); `type` is mapped to
//! `event_type` because `type` is a Rust keyword.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "events")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: Uuid,
    pub source: String,
    #[sea_orm(column_name = "type")]
    pub event_type: String,
    pub external_id: String,
    pub scope_key: String,
    pub actor: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub occurred_at: DateTimeWithTimeZone,
    pub payload: Json,
    pub ingested_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
