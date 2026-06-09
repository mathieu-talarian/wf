//! SeaORM entity for `sync_state` (A1 spec §3.2) — one row per pollable scope,
//! composite PK. Matches `migration::m0002` (note: the `cursor` column is a
//! reserved word in Postgres; SeaORM quotes it, raw SQL must too).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "sync_state")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub source: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub scope_key: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub entity_kind: String,
    pub cursor: Option<String>,
    pub last_polled_at: Option<DateTimeWithTimeZone>,
    pub next_poll_at: DateTimeWithTimeZone,
    pub consecutive_errors: i32,
    pub last_error: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_until: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
