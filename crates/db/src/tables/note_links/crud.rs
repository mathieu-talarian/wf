//! `note_links` repository: full re-sync per note save plus the backlink
//! lookup for the notes editor footer.

use sea_orm::prelude::Uuid;
use sea_orm::ActiveValue::Set;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};

use super::entity as links;

/// `(to_ticket, snippet)` pairs parsed from one note body.
pub type ParsedLink = (String, String);

/// Replaces every outgoing link of `from_ticket` with the freshly parsed set.
pub async fn replace_for_ticket(
    db: &DatabaseConnection,
    user_id: Uuid,
    from_ticket: &str,
    parsed: Vec<ParsedLink>,
) -> Result<(), DbErr> {
    links::Entity::delete_many()
        .filter(links::Column::UserId.eq(user_id))
        .filter(links::Column::FromTicket.eq(from_ticket))
        .exec(db)
        .await?;
    if parsed.is_empty() {
        return Ok(());
    }
    let models = parsed.into_iter().map(|(to_ticket, snippet)| links::ActiveModel {
        user_id: Set(user_id),
        from_ticket: Set(from_ticket.to_string()),
        to_ticket: Set(to_ticket),
        snippet: Set(snippet),
    });
    links::Entity::insert_many(models).exec(db).await.map(|_| ())
}

/// Notes that `[[link]]` to `to_ticket` — the BACKLINKS footer.
pub async fn backlinks(
    db: &DatabaseConnection,
    user_id: Uuid,
    to_ticket: &str,
) -> Result<Vec<links::Model>, DbErr> {
    links::Entity::find()
        .filter(links::Column::UserId.eq(user_id))
        .filter(links::Column::ToTicket.eq(to_ticket))
        .all(db)
        .await
}
