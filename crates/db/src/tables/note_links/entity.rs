//! SeaORM entity for `note_links` — matches `migration::m0003`. One row per
//! `[[to_ticket]]` wiki-link found in `from_ticket`'s note; `snippet` is the
//! line containing the link (shown under BACKLINKS in the notes editor).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "note_links")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub from_ticket: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub to_ticket: String,
    pub snippet: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
