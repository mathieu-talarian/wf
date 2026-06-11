//! SeaORM entity for `ticket_links` — matches `migration::m0003`. Manual
//! ticket↔code links (the orphan tray's `+ link manually`); exactly one of
//! `pr_number` / `branch` is set (CHECK constraint). Manual links always win
//! over the hub's auto-matching.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "ticket_links")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: Uuid,
    pub ticket_key: String,
    pub repo: String,
    pub pr_number: Option<i64>,
    pub branch: Option<String>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
