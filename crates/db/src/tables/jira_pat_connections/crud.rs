//! `jira_pat_connections` repository (port of `jira/pat/account.ts`).
//! Connection-flow operations; `selected_projects` is preserved across
//! re-connect.

use sea_orm::prelude::{DateTimeWithTimeZone, Expr, Uuid};
use sea_orm::sea_query::OnConflict;
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use wf_core::Sealed;

use super::entity as jira;

/// Input for connect/re-connect (port of `UpsertJiraInputT`).
pub struct UpsertJiraInput {
    pub user_id: Uuid,
    pub site_url: String,
    pub account_id: String,
    pub email: String,
    pub display_name: String,
    pub sealed: Sealed,
    pub last_four: String,
}

pub async fn select_row(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<Option<jira::Model>, DbErr> {
    jira::Entity::find_by_id(user_id).one(db).await
}

/// Insert-or-update on `user_id`. Re-connect preserves `created_at`,
/// `last_used_at`, and `selected_projects` (none are in the update set), matching
/// `account.ts#upsert`.
pub async fn upsert_jira(db: &DatabaseConnection, input: UpsertJiraInput) -> Result<(), DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    jira::Entity::insert(upsert_model(input, now))
        .on_conflict(upsert_on_conflict())
        .exec(db)
        .await?;
    Ok(())
}

/// Builds the `ActiveModel` for [`upsert_jira`]. `selected_projects` and
/// `last_used_at` are left `NotSet` so they survive a re-connect.
fn upsert_model(input: UpsertJiraInput, now: DateTimeWithTimeZone) -> jira::ActiveModel {
    jira::ActiveModel {
        user_id: Set(input.user_id),
        site_url: Set(input.site_url),
        cloud_id: Set(None),
        account_id: Set(input.account_id),
        email: Set(input.email),
        display_name: Set(input.display_name),
        api_token_ciphertext: Set(input.sealed.ciphertext),
        api_token_iv: Set(input.sealed.iv),
        api_token_auth_tag: Set(input.sealed.auth_tag),
        last_four: Set(Some(input.last_four)),
        last_validated_at: Set(Some(now)),
        validation_status: Set("valid".to_string()),
        validation_error: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        // Preserved across re-connect.
        selected_projects: NotSet,
        last_used_at: NotSet,
    }
}

/// The `ON CONFLICT (user_id) DO UPDATE` clause for [`upsert_jira`].
fn upsert_on_conflict() -> OnConflict {
    OnConflict::column(jira::Column::UserId)
        .update_columns([
            jira::Column::SiteUrl,
            jira::Column::CloudId,
            jira::Column::AccountId,
            jira::Column::Email,
            jira::Column::DisplayName,
            jira::Column::ApiTokenCiphertext,
            jira::Column::ApiTokenIv,
            jira::Column::ApiTokenAuthTag,
            jira::Column::LastFour,
            jira::Column::LastValidatedAt,
            jira::Column::ValidationStatus,
            jira::Column::ValidationError,
            jira::Column::UpdatedAt,
        ])
        .to_owned()
}

/// Persists a validation outcome (re-validation failure path).
pub async fn mark_validation(
    db: &DatabaseConnection,
    user_id: Uuid,
    status: &str,
    error: &str,
) -> Result<(), DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    jira::Entity::update_many()
        .col_expr(jira::Column::ValidationStatus, Expr::value(status))
        .col_expr(jira::Column::ValidationError, Expr::value(error))
        .col_expr(jira::Column::LastValidatedAt, Expr::value(now))
        .filter(jira::Column::UserId.eq(user_id))
        .exec(db)
        .await?;
    Ok(())
}

/// Sets `selected_projects` (port of `runSetSelectedProjects`).
pub async fn set_selected_projects(
    db: &DatabaseConnection,
    user_id: Uuid,
    projects: &[String],
) -> Result<(), DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let arr = serde_json::Value::Array(
        projects.iter().map(|p| serde_json::Value::String(p.clone())).collect(),
    );
    jira::Entity::update_many()
        .col_expr(jira::Column::SelectedProjects, Expr::value(arr))
        .col_expr(jira::Column::UpdatedAt, Expr::value(now))
        .filter(jira::Column::UserId.eq(user_id))
        .exec(db)
        .await?;
    Ok(())
}

/// Bumps `last_used_at` (best-effort from the data paths).
pub async fn touch_last_used(db: &DatabaseConnection, user_id: Uuid) -> Result<(), DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    jira::Entity::update_many()
        .col_expr(jira::Column::LastUsedAt, Expr::value(now))
        .filter(jira::Column::UserId.eq(user_id))
        .exec(db)
        .await?;
    Ok(())
}

pub async fn disconnect(db: &DatabaseConnection, user_id: Uuid) -> Result<(), DbErr> {
    jira::Entity::delete_by_id(user_id).exec(db).await?;
    Ok(())
}
