//! SeaORM entity for `github_pat_connections` (migration plan §6.3). PK is
//! `user_id` (FK → users.id, ON DELETE CASCADE). jsonb columns are kept as
//! `Json` (serde_json::Value) and projected by the repository/summary layer.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "github_pat_connections")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    pub github_user_id: i64,
    pub github_login: String,
    pub access_token_ciphertext: String,
    pub access_token_iv: String,
    pub access_token_auth_tag: String,
    pub token_kind: String,
    pub scope: Option<String>,
    pub permissions: Option<Json>,
    pub selected_repos: Option<Json>,
    pub favorite_workflows: Option<Json>,
    pub dashboard_snapshot: Option<Json>,
    pub last_four: Option<String>,
    pub expires_at: Option<DateTimeWithTimeZone>,
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
