//! Typed writes and bounded reads for the Jira issue projection.

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::OnConflict;
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait};
use sea_orm::{QueryFilter, QueryOrder, QuerySelect};

use super::entity as issue;

pub struct UpsertJiraIssueInput {
    pub issue_key: String,
    pub summary: String,
    pub status_id: String,
    pub status_name: String,
    pub status_category: String,
    pub assignee_name: Option<String>,
    pub priority_name: Option<String>,
    pub issue_type_name: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    pub url: String,
}

fn active_model(user_id: Uuid, project: &str, input: UpsertJiraIssueInput) -> issue::ActiveModel {
    issue::ActiveModel {
        id: NotSet,
        user_id: Set(user_id),
        project: Set(project.to_string()),
        issue_key: Set(input.issue_key),
        summary: Set(input.summary),
        status_id: Set(input.status_id),
        status_name: Set(input.status_name),
        status_category: Set(input.status_category),
        assignee_name: Set(input.assignee_name),
        priority_name: Set(input.priority_name),
        issue_type_name: Set(input.issue_type_name),
        created_at: Set(input.created_at),
        updated_at: Set(input.updated_at),
        url: Set(input.url),
        synced_at: Set(chrono::Utc::now().into()),
    }
}

fn conflict() -> OnConflict {
    OnConflict::columns([issue::Column::UserId, issue::Column::IssueKey])
        .update_columns([
            issue::Column::Project,
            issue::Column::Summary,
            issue::Column::StatusId,
            issue::Column::StatusName,
            issue::Column::StatusCategory,
            issue::Column::AssigneeName,
            issue::Column::PriorityName,
            issue::Column::IssueTypeName,
            issue::Column::CreatedAt,
            issue::Column::UpdatedAt,
            issue::Column::Url,
            issue::Column::SyncedAt,
        ])
        .to_owned()
}

pub async fn upsert_many(
    db: &DatabaseConnection,
    user_id: Uuid,
    project: &str,
    inputs: Vec<UpsertJiraIssueInput>,
) -> Result<(), DbErr> {
    if inputs.is_empty() {
        return Ok(());
    }
    let models = inputs.into_iter().map(|input| active_model(user_id, project, input));
    issue::Entity::insert_many(models).on_conflict(conflict()).exec(db).await.map(|_| ())
}

pub async fn list_recent(
    db: &DatabaseConnection,
    user_id: Uuid,
    projects: &[String],
    limit: u64,
) -> Result<Vec<issue::Model>, DbErr> {
    if projects.is_empty() {
        return Ok(vec![]);
    }
    let cutoff: DateTimeWithTimeZone = (chrono::Utc::now() - chrono::Duration::days(30)).into();
    issue::Entity::find()
        .filter(issue::Column::UserId.eq(user_id))
        .filter(issue::Column::Project.is_in(projects.iter().cloned()))
        .filter(issue::Column::UpdatedAt.gte(cutoff))
        .order_by_desc(issue::Column::UpdatedAt)
        .limit(limit.min(500))
        .all(db)
        .await
}
