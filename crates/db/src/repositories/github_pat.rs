//! `github_pat_connections` repository (migration plan §6.4). Connection-flow
//! operations; dashboard/favorites/snapshot writes are added in later chunks.
//! Port of `github/pat/account.ts`.

use std::collections::{HashMap, HashSet};

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
    gh::Entity::insert(upsert_model(input, now))
        .on_conflict(upsert_on_conflict())
        .exec(db)
        .await?;
    Ok(())
}

/// Builds the `ActiveModel` for [`upsert_pat`]. Selection/favorites/snapshot are
/// left `NotSet` so they survive a re-connect.
fn upsert_model(input: UpsertPatInput, now: DateTimeWithTimeZone) -> gh::ActiveModel {
    let scope = input.scopes.as_ref().map(|s| s.join(","));
    gh::ActiveModel {
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
    }
}

/// The `ON CONFLICT (user_id) DO UPDATE` clause for [`upsert_pat`]: refreshes the
/// token/validation columns but never the preserved jsonb/timestamp columns.
fn upsert_on_conflict() -> OnConflict {
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

/// Repo-full-name → workflow ids (port of `FavoritesMapT`).
pub type FavoritesMap = HashMap<String, Vec<i64>>;

/// Decode the `favorite_workflows` jsonb (port of `favoritesOf`): null or a
/// malformed shape both yield an empty map.
fn favorites_from_value(value: Option<&serde_json::Value>) -> FavoritesMap {
    value.cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

pub fn favorites_of(row: &gh::Model) -> FavoritesMap {
    favorites_from_value(row.favorite_workflows.as_ref())
}

/// Set one repo's favorites without disturbing the others (port of
/// `setRepoInFavorites`): de-dupe (first occurrence wins), and drop the repo
/// entirely when its list becomes empty.
pub fn set_repo_in_favorites(
    map: &FavoritesMap,
    repo_full_name: &str,
    workflow_ids: &[i64],
) -> FavoritesMap {
    let mut seen = HashSet::new();
    let deduped: Vec<i64> = workflow_ids.iter().copied().filter(|id| seen.insert(*id)).collect();
    let mut next = map.clone();
    next.remove(repo_full_name);
    if !deduped.is_empty() {
        next.insert(repo_full_name.to_string(), deduped);
    }
    next
}

/// `GET /me/github/favorites` (port of `runGetFavorites`): empty when no row.
pub async fn get_favorites(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<FavoritesMap, DbErr> {
    Ok(select_row(db, user_id).await?.map(|row| favorites_of(&row)).unwrap_or_default())
}

async fn persist_favorites(
    db: &DatabaseConnection,
    user_id: Uuid,
    favorites: &FavoritesMap,
) -> Result<(), DbErr> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let value = serde_json::to_value(favorites).unwrap_or(serde_json::Value::Null);
    gh::Entity::update_many()
        .col_expr(gh::Column::FavoriteWorkflows, Expr::value(value))
        .col_expr(gh::Column::UpdatedAt, Expr::value(now))
        .filter(gh::Column::UserId.eq(user_id))
        .exec(db)
        .await?;
    Ok(())
}

/// `PUT /me/github/favorites` (port of `runSetRepoFavorites`): merge one repo's
/// favorites and return the full map. Errors when no token is connected.
pub async fn set_repo_favorites(
    db: &DatabaseConnection,
    user_id: Uuid,
    repo_full_name: &str,
    workflow_ids: &[i64],
) -> Result<FavoritesMap, DbErr> {
    let row = select_row(db, user_id)
        .await?
        .ok_or_else(|| DbErr::Custom("No GitHub token connected".to_string()))?;
    let next = set_repo_in_favorites(&favorites_of(&row), repo_full_name, workflow_ids);
    persist_favorites(db, user_id, &next).await?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &[i64])]) -> FavoritesMap {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect()
    }

    #[test]
    fn favorites_of_null_is_empty() {
        assert!(favorites_from_value(None).is_empty());
        assert!(favorites_from_value(Some(&serde_json::Value::Null)).is_empty());
    }

    #[test]
    fn favorites_of_returns_stored_map() {
        let v = serde_json::json!({ "o/a": [1, 2] });
        assert_eq!(favorites_from_value(Some(&v)), map(&[("o/a", &[1, 2])]));
    }

    #[test]
    fn sets_one_repo_without_touching_others() {
        let next = set_repo_in_favorites(&map(&[("o/a", &[1])]), "o/b", &[3, 4]);
        assert_eq!(next, map(&[("o/a", &[1]), ("o/b", &[3, 4])]));
    }

    #[test]
    fn overwrites_and_dedupes_existing_repo() {
        let next = set_repo_in_favorites(&map(&[("o/a", &[1, 2])]), "o/a", &[2, 2, 5]);
        assert_eq!(next, map(&[("o/a", &[2, 5])]));
    }

    #[test]
    fn drops_repo_when_list_becomes_empty() {
        let next = set_repo_in_favorites(&map(&[("o/a", &[1]), ("o/b", &[2])]), "o/a", &[]);
        assert_eq!(next, map(&[("o/b", &[2])]));
    }
}
