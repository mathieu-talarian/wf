//! `ticket_links` repository: insert/delete one manual link and the full
//! per-user list the hub board folds into its matching pass.

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};

use super::entity as link;

/// Inserts a manual link; a conflicting link for the same PR/branch is
/// replaced (re-linking to a different ticket is the supported correction
/// flow, so delete-then-insert).
pub async fn insert(
    db: &DatabaseConnection,
    user_id: Uuid,
    ticket_key: &str,
    repo: &str,
    pr_number: Option<i64>,
    branch: Option<&str>,
) -> Result<(), DbErr> {
    delete_target(db, user_id, repo, pr_number, branch).await?;
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let model = link::ActiveModel {
        id: NotSet,
        user_id: Set(user_id),
        ticket_key: Set(ticket_key.to_string()),
        repo: Set(repo.to_string()),
        pr_number: Set(pr_number),
        branch: Set(branch.map(str::to_string)),
        created_at: Set(now),
    };
    link::Entity::insert(model).exec(db).await.map(|_| ())
}

/// Deletes whatever link points at the given PR or branch.
pub async fn delete_target(
    db: &DatabaseConnection,
    user_id: Uuid,
    repo: &str,
    pr_number: Option<i64>,
    branch: Option<&str>,
) -> Result<(), DbErr> {
    let mut query = link::Entity::delete_many()
        .filter(link::Column::UserId.eq(user_id))
        .filter(link::Column::Repo.eq(repo));
    match (pr_number, branch) {
        (Some(number), _) => query = query.filter(link::Column::PrNumber.eq(number)),
        (None, Some(name)) => query = query.filter(link::Column::Branch.eq(name)),
        (None, None) => return Ok(()),
    }
    query.exec(db).await.map(|_| ())
}

pub async fn list(db: &DatabaseConnection, user_id: Uuid) -> Result<Vec<link::Model>, DbErr> {
    link::Entity::find()
        .filter(link::Column::UserId.eq(user_id))
        .all(db)
        .await
}
