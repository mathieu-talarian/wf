//! Durable current-state projection for Jira issues.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "jira_issues")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: Uuid,
    pub project: String,
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
    pub synced_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
