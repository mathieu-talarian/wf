//! `slack_connections` repository: one row per user holding the sealed bot
//! token, workspace identity, validation state, and the watched-channel list.

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};

use super::entity as slack;

pub struct UpsertSlackConnectionInput {
    pub user_id: Uuid,
    pub bot_token_ciphertext: String,
    pub bot_token_iv: String,
    pub bot_token_auth_tag: String,
    pub team_name: Option<String>,
    pub bot_user_id: Option<String>,
    pub validation_status: String,
}

/// Builds the row for an upsert. `watched_channels` is `NotSet` so an existing
/// channel list survives a token re-validation (only the listed columns update).
fn build_active_model(
    input: UpsertSlackConnectionInput,
    now: DateTimeWithTimeZone,
) -> slack::ActiveModel {
    slack::ActiveModel {
        user_id: Set(input.user_id),
        bot_token_ciphertext: Set(input.bot_token_ciphertext),
        bot_token_iv: Set(input.bot_token_iv),
        bot_token_auth_tag: Set(input.bot_token_auth_tag),
        team_name: Set(input.team_name),
        bot_user_id: Set(input.bot_user_id),
        watched_channels: NotSet,
        last_validated_at: Set(Some(now)),
        validation_status: Set(input.validation_status),
        validation_error: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
}

pub async fn upsert_connection(
    db: &DatabaseConnection,
    input: UpsertSlackConnectionInput,
) -> Result<slack::Model, DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let model = build_active_model(input, now);
    slack::Entity::insert(model)
        .on_conflict(
            OnConflict::column(slack::Column::UserId)
                .update_columns([
                    slack::Column::BotTokenCiphertext,
                    slack::Column::BotTokenIv,
                    slack::Column::BotTokenAuthTag,
                    slack::Column::TeamName,
                    slack::Column::BotUserId,
                    slack::Column::LastValidatedAt,
                    slack::Column::ValidationStatus,
                    slack::Column::ValidationError,
                    slack::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec_with_returning(db)
        .await
}

pub async fn select_row(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<Option<slack::Model>, DbErr> {
    slack::Entity::find_by_id(user_id).one(db).await
}

/// All connections — the tick reconciles valid and invalid scopes alike.
pub async fn list_all(db: &DatabaseConnection) -> Result<Vec<slack::Model>, DbErr> {
    slack::Entity::find().all(db).await
}

pub async fn set_validation(
    db: &DatabaseConnection,
    user_id: Uuid,
    status: &str,
    error: Option<&str>,
) -> Result<(), DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    slack::Entity::update_many()
        .col_expr(slack::Column::ValidationStatus, Expr::value(status))
        .col_expr(slack::Column::ValidationError, Expr::value(error))
        .col_expr(slack::Column::LastValidatedAt, Expr::value(now))
        .col_expr(slack::Column::UpdatedAt, Expr::value(now))
        .filter(slack::Column::UserId.eq(user_id))
        .exec(db)
        .await
        .map(|_| ())
}

pub async fn set_channels(
    db: &DatabaseConnection,
    user_id: Uuid,
    channels: serde_json::Value,
) -> Result<(), DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    slack::Entity::update_many()
        .col_expr(slack::Column::WatchedChannels, Expr::value(channels))
        .col_expr(slack::Column::UpdatedAt, Expr::value(now))
        .filter(slack::Column::UserId.eq(user_id))
        .exec(db)
        .await
        .map(|_| ())
}

pub async fn delete_connection(db: &DatabaseConnection, user_id: Uuid) -> Result<(), DbErr> {
    slack::Entity::delete_by_id(user_id).exec(db).await.map(|_| ())
}
