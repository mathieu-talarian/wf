//! SeaORM entity for `reminders` — matches `migration::m0003`. Rows are
//! *derived* from `⏰` tokens in notes: `id` is a stable content hash of
//! `(ticket_key, body, due_at)` so done/snoozed state survives note edits that
//! don't touch the reminder line; the API layer re-syncs the set per save.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "reminders")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub ticket_key: String,
    pub body: String,
    pub due_at: DateTimeWithTimeZone,
    pub state: String,
    pub snoozed_until: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
