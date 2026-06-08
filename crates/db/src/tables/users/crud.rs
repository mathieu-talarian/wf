//! `users` repository: upsert-from-auth (migration plan §6.4).
//!
//! Port of `domain/users.ts#runUpsert`: insert `(id, email, name, avatar_url,
//! created_at, updated_at)` and, on conflict on `id`, update everything except
//! `id` and `created_at`. Returns the resulting row (the `/me` response).

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::OnConflict;
use sea_orm::ActiveValue::Set;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};
use wf_core::AuthedUser;

use super::entity as users;

pub async fn upsert_from_auth(
    db: &DatabaseConnection,
    authed: &AuthedUser,
) -> Result<users::Model, DbErr> {
    // Supabase `sub` is a uuid; an invalid value is treated as a DB-layer error
    // (the TS path lets Postgres reject the cast → 500).
    let id = Uuid::parse_str(&authed.id)
        .map_err(|e| DbErr::Custom(format!("invalid user id {:?}: {e}", authed.id)))?;

    users::Entity::insert(active_model(id, authed))
        .on_conflict(
            OnConflict::column(users::Column::Id)
                .update_columns([
                    users::Column::Email,
                    users::Column::Name,
                    users::Column::AvatarUrl,
                    users::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec_with_returning(db)
        .await
}

/// Builds the `ActiveModel` for [`upsert_from_auth`], stamping `created_at`/
/// `updated_at` with the current time (the conflict clause keeps `created_at`).
fn active_model(id: Uuid, authed: &AuthedUser) -> users::ActiveModel {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    users::ActiveModel {
        id: Set(id),
        email: Set(authed.email.clone()),
        name: Set(authed.name.clone()),
        avatar_url: Set(authed.avatar_url.clone()),
        created_at: Set(now),
        updated_at: Set(now),
    }
}
