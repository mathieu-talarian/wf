//! `notes` repository: get/upsert one note per (user, ticket) and the
//! has-a-note key list the hub board uses for its `✎` chips.

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::OnConflict;
use sea_orm::ActiveValue::Set;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect};

use super::entity as notes;

pub async fn get(
    db: &DatabaseConnection,
    user_id: Uuid,
    ticket_key: &str,
) -> Result<Option<notes::Model>, DbErr> {
    notes::Entity::find_by_id((user_id, ticket_key.to_string())).one(db).await
}

pub async fn upsert(
    db: &DatabaseConnection,
    user_id: Uuid,
    ticket_key: &str,
    body: &str,
) -> Result<notes::Model, DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let model = notes::ActiveModel {
        user_id: Set(user_id),
        ticket_key: Set(ticket_key.to_string()),
        body: Set(body.to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    };
    notes::Entity::insert(model)
        .on_conflict(
            OnConflict::columns([notes::Column::UserId, notes::Column::TicketKey])
                .update_columns([notes::Column::Body, notes::Column::UpdatedAt])
                .to_owned(),
        )
        .exec_with_returning(db)
        .await
}

/// Ticket keys that have a non-empty note — the board's `✎` chip source.
pub async fn keys_with_notes(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<Vec<String>, DbErr> {
    notes::Entity::find()
        .select_only()
        .column(notes::Column::TicketKey)
        .filter(notes::Column::UserId.eq(user_id))
        .filter(notes::Column::Body.ne(""))
        .into_tuple()
        .all(db)
        .await
}
