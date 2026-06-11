//! SeaORM entity for `slack_connections` — matches `migration::m0003`. PK is
//! `user_id` (FK → users.id, ON DELETE CASCADE). The bot token is sealed with
//! the same AES-GCM scheme as the GitHub/Jira PATs; `watched_channels` is a
//! jsonb array of `{id, name}` objects projected by the API layer.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "slack_connections")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    pub bot_token_ciphertext: String,
    pub bot_token_iv: String,
    pub bot_token_auth_tag: String,
    pub team_name: Option<String>,
    pub bot_user_id: Option<String>,
    pub watched_channels: Option<Json>,
    pub last_validated_at: Option<DateTimeWithTimeZone>,
    pub validation_status: String,
    pub validation_error: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
