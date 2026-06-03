//! SeaORM entity for `jira_pat_connections` (migration plan §6.3). PK is
//! `user_id` (FK → users.id, ON DELETE CASCADE). The `selected_projects` jsonb is
//! kept as `Json` and projected by the repository/summary layer.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "jira_pat_connections")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    pub site_url: String,
    pub cloud_id: Option<String>,
    pub account_id: String,
    pub email: String,
    pub display_name: String,
    pub api_token_ciphertext: String,
    pub api_token_iv: String,
    pub api_token_auth_tag: String,
    pub selected_projects: Option<Json>,
    pub last_four: Option<String>,
    pub last_validated_at: Option<DateTimeWithTimeZone>,
    pub last_used_at: Option<DateTimeWithTimeZone>,
    pub validation_status: String,
    pub validation_error: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
