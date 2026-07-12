//! Durable current-state projection for GitHub pull requests.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "github_pull_requests")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: Uuid,
    pub repo: String,
    pub number: i64,
    pub state: String,
    pub title: String,
    pub url: String,
    pub draft: bool,
    pub head_ref: Option<String>,
    pub author_login: Option<String>,
    pub assignee_logins: Json,
    pub requested_reviewer_logins: Json,
    pub updated_at: DateTimeWithTimeZone,
    pub synced_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
