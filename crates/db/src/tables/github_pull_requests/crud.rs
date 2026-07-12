//! Typed writes and bounded reads for the GitHub pull-request projection.

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::OnConflict;
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait};
use sea_orm::{QueryFilter, QueryOrder, QuerySelect};

use super::entity as pull;

pub struct UpsertPullRequestInput {
    pub number: i64,
    pub state: String,
    pub title: String,
    pub url: String,
    pub draft: bool,
    pub head_ref: Option<String>,
    pub author_login: Option<String>,
    pub assignee_logins: Vec<String>,
    pub requested_reviewer_logins: Vec<String>,
    pub updated_at: DateTimeWithTimeZone,
}

fn active_model(user_id: Uuid, repo: &str, input: UpsertPullRequestInput) -> pull::ActiveModel {
    pull::ActiveModel {
        id: NotSet,
        user_id: Set(user_id),
        repo: Set(repo.to_string()),
        number: Set(input.number),
        state: Set(input.state),
        title: Set(input.title),
        url: Set(input.url),
        draft: Set(input.draft),
        head_ref: Set(input.head_ref),
        author_login: Set(input.author_login),
        assignee_logins: Set(serde_json::json!(input.assignee_logins)),
        requested_reviewer_logins: Set(serde_json::json!(input.requested_reviewer_logins)),
        updated_at: Set(input.updated_at),
        synced_at: Set(chrono::Utc::now().into()),
    }
}

fn conflict() -> OnConflict {
    OnConflict::columns([pull::Column::UserId, pull::Column::Repo, pull::Column::Number])
        .update_columns([
            pull::Column::State,
            pull::Column::Title,
            pull::Column::Url,
            pull::Column::Draft,
            pull::Column::HeadRef,
            pull::Column::AuthorLogin,
            pull::Column::AssigneeLogins,
            pull::Column::RequestedReviewerLogins,
            pull::Column::UpdatedAt,
            pull::Column::SyncedAt,
        ])
        .to_owned()
}

pub async fn upsert_many(
    db: &DatabaseConnection,
    user_id: Uuid,
    repo: &str,
    inputs: Vec<UpsertPullRequestInput>,
) -> Result<(), DbErr> {
    if inputs.is_empty() {
        return Ok(());
    }
    let models = inputs.into_iter().map(|input| active_model(user_id, repo, input));
    pull::Entity::insert_many(models).on_conflict(conflict()).exec(db).await.map(|_| ())
}

pub async fn list_recent(
    db: &DatabaseConnection,
    user_id: Uuid,
    repos: &[String],
    limit: u64,
) -> Result<Vec<pull::Model>, DbErr> {
    if repos.is_empty() {
        return Ok(vec![]);
    }
    let cutoff: DateTimeWithTimeZone = (chrono::Utc::now() - chrono::Duration::days(30)).into();
    let current = Condition::any()
        .add(pull::Column::State.eq("open"))
        .add(pull::Column::UpdatedAt.gte(cutoff));
    pull::Entity::find()
        .filter(pull::Column::UserId.eq(user_id))
        .filter(pull::Column::Repo.is_in(repos.iter().cloned()))
        .filter(current)
        .order_by_desc(pull::Column::UpdatedAt)
        .limit(limit.min(500))
        .all(db)
        .await
}
