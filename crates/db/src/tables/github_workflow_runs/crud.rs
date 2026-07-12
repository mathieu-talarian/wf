//! Typed writes and bounded reads for the GitHub workflow-run projection.

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::OnConflict;
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait};
use sea_orm::{QueryFilter, QueryOrder, QuerySelect};

use super::entity as run;

pub struct UpsertWorkflowRunInput {
    pub run_id: i64,
    pub workflow_id: Option<i64>,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub url: String,
    pub head_branch: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

fn active_model(user_id: Uuid, repo: &str, input: UpsertWorkflowRunInput) -> run::ActiveModel {
    run::ActiveModel {
        id: NotSet,
        user_id: Set(user_id),
        repo: Set(repo.to_string()),
        run_id: Set(input.run_id),
        workflow_id: Set(input.workflow_id),
        name: Set(input.name),
        status: Set(input.status),
        conclusion: Set(input.conclusion),
        url: Set(input.url),
        head_branch: Set(input.head_branch),
        created_at: Set(input.created_at),
        updated_at: Set(input.updated_at),
        synced_at: Set(chrono::Utc::now().into()),
    }
}

fn conflict() -> OnConflict {
    OnConflict::columns([run::Column::UserId, run::Column::Repo, run::Column::RunId])
        .update_columns([
            run::Column::WorkflowId,
            run::Column::Name,
            run::Column::Status,
            run::Column::Conclusion,
            run::Column::Url,
            run::Column::HeadBranch,
            run::Column::CreatedAt,
            run::Column::UpdatedAt,
            run::Column::SyncedAt,
        ])
        .to_owned()
}

pub async fn upsert_many(
    db: &DatabaseConnection,
    user_id: Uuid,
    repo: &str,
    inputs: Vec<UpsertWorkflowRunInput>,
) -> Result<(), DbErr> {
    if inputs.is_empty() {
        return Ok(());
    }
    let models = inputs.into_iter().map(|input| active_model(user_id, repo, input));
    run::Entity::insert_many(models).on_conflict(conflict()).exec(db).await.map(|_| ())
}

pub async fn list_recent(
    db: &DatabaseConnection,
    user_id: Uuid,
    repos: &[String],
    limit: u64,
) -> Result<Vec<run::Model>, DbErr> {
    if repos.is_empty() {
        return Ok(vec![]);
    }
    run::Entity::find()
        .filter(run::Column::UserId.eq(user_id))
        .filter(run::Column::Repo.is_in(repos.iter().cloned()))
        .order_by_desc(run::Column::UpdatedAt)
        .limit(limit.min(200))
        .all(db)
        .await
}

/// Latest completed non-success runs for the inbox.
pub async fn list_failed_recent(
    db: &DatabaseConnection,
    user_id: Uuid,
    repos: &[String],
    limit: u64,
) -> Result<Vec<run::Model>, DbErr> {
    if repos.is_empty() {
        return Ok(vec![]);
    }
    let failed = Condition::any()
        .add(Condition::all().add(run::Column::Conclusion.is_not_null()).add(run::Column::Conclusion.ne("success")))
        .add(Condition::all().add(run::Column::Status.eq("completed")).add(run::Column::Conclusion.is_null()));
    run::Entity::find()
        .filter(run::Column::UserId.eq(user_id))
        .filter(run::Column::Repo.is_in(repos.iter().cloned()))
        .filter(failed)
        .order_by_desc(run::Column::UpdatedAt)
        .limit(limit.min(50))
        .all(db)
        .await
}
