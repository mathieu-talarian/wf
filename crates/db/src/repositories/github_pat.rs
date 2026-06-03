//! `github_pat_connections` repository (migration plan §6.4). Connection-flow
//! operations; dashboard/favorites/snapshot writes are added in later chunks.
//! Port of `github/pat/account.ts`.

use sea_orm::prelude::{DateTimeWithTimeZone, Expr, Uuid};
use sea_orm::sea_query::{OnConflict, Value as SqlValue};
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use wf_core::Sealed;

use crate::entities::github_pat_connections as gh;

/// Input for connect/re-connect (port of `UpsertPatInputT`).
pub struct UpsertPatInput {
    pub user_id: Uuid,
    pub github_user_id: i64,
    pub github_login: String,
    pub token_kind: String,
    pub sealed: Sealed,
    pub scopes: Option<Vec<String>>,
    pub expires_at: Option<DateTimeWithTimeZone>,
    pub last_four: String,
    pub validation_status: String,
}

pub async fn select_row(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<Option<gh::Model>, DbErr> {
    gh::Entity::find_by_id(user_id).one(db).await
}

/// Insert-or-update on `user_id`, preserving `created_at`, `last_used_at`, and
/// the selection/favorites/snapshot jsonb columns (those are never touched
/// here) — matches `account.ts#upsert`/`buildUpsertRow`.
pub async fn upsert_pat(db: &DatabaseConnection, input: UpsertPatInput) -> Result<(), DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let scope = input.scopes.as_ref().map(|s| s.join(","));

    let model = gh::ActiveModel {
        user_id: Set(input.user_id),
        github_user_id: Set(input.github_user_id),
        github_login: Set(input.github_login),
        access_token_ciphertext: Set(input.sealed.ciphertext),
        access_token_iv: Set(input.sealed.iv),
        access_token_auth_tag: Set(input.sealed.auth_tag),
        token_kind: Set(input.token_kind),
        scope: Set(scope),
        permissions: Set(None),
        last_four: Set(Some(input.last_four)),
        expires_at: Set(input.expires_at),
        last_validated_at: Set(Some(now)),
        last_used_at: Set(None),
        validation_status: Set(input.validation_status),
        validation_error: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        // Preserve selection/favorites/snapshot across re-connect.
        selected_repos: NotSet,
        favorite_workflows: NotSet,
        dashboard_snapshot: NotSet,
    };

    gh::Entity::insert(model)
        .on_conflict(
            OnConflict::column(gh::Column::UserId)
                .update_columns([
                    gh::Column::GithubUserId,
                    gh::Column::GithubLogin,
                    gh::Column::TokenKind,
                    gh::Column::AccessTokenCiphertext,
                    gh::Column::AccessTokenIv,
                    gh::Column::AccessTokenAuthTag,
                    gh::Column::Scope,
                    gh::Column::Permissions,
                    gh::Column::LastFour,
                    gh::Column::ExpiresAt,
                    gh::Column::LastValidatedAt,
                    gh::Column::ValidationStatus,
                    gh::Column::ValidationError,
                    gh::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(())
}

/// Persists a validation outcome (re-validation failure path).
pub async fn mark_validation(
    db: &DatabaseConnection,
    user_id: Uuid,
    status: &str,
    error: &str,
) -> Result<(), DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    gh::Entity::update_many()
        .col_expr(gh::Column::ValidationStatus, Expr::value(status))
        .col_expr(gh::Column::ValidationError, Expr::value(error))
        .col_expr(gh::Column::LastValidatedAt, Expr::value(now))
        .filter(gh::Column::UserId.eq(user_id))
        .exec(db)
        .await?;
    Ok(())
}

pub async fn disconnect(db: &DatabaseConnection, user_id: Uuid) -> Result<(), DbErr> {
    gh::Entity::delete_by_id(user_id).exec(db).await?;
    Ok(())
}

/// Sets the selected repos and clears the durable dashboard snapshot (a repo
/// change invalidates it). Port of `runSetSelectedRepos`.
pub async fn set_selected_repos(
    db: &DatabaseConnection,
    user_id: Uuid,
    repos: &[String],
) -> Result<(), DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let arr = serde_json::Value::Array(
        repos.iter().map(|r| serde_json::Value::String(r.clone())).collect(),
    );
    gh::Entity::update_many()
        .col_expr(gh::Column::SelectedRepos, Expr::value(arr))
        .col_expr(gh::Column::DashboardSnapshot, Expr::value(SqlValue::Json(None)))
        .col_expr(gh::Column::UpdatedAt, Expr::value(now))
        .filter(gh::Column::UserId.eq(user_id))
        .exec(db)
        .await?;
    Ok(())
}

/// Persists `{ tab, data }` to `dashboard_snapshot` (cold-start SWR).
pub async fn set_dashboard_snapshot(
    db: &DatabaseConnection,
    user_id: Uuid,
    tab: &str,
    data: serde_json::Value,
) -> Result<(), DbErr> {
    let snap = serde_json::json!({ "tab": tab, "data": data });
    gh::Entity::update_many()
        .col_expr(gh::Column::DashboardSnapshot, Expr::value(snap))
        .filter(gh::Column::UserId.eq(user_id))
        .exec(db)
        .await?;
    Ok(())
}

/// Bumps `last_used_at` (best-effort, fire-and-forget from the dashboard path).
pub async fn touch_last_used(db: &DatabaseConnection, user_id: Uuid) -> Result<(), DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    gh::Entity::update_many()
        .col_expr(gh::Column::LastUsedAt, Expr::value(now))
        .filter(gh::Column::UserId.eq(user_id))
        .exec(db)
        .await?;
    Ok(())
}
