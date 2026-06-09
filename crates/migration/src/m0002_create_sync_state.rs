use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TABLE sync_state (
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  source text NOT NULL,
  scope_key text NOT NULL,
  entity_kind text NOT NULL,
  "cursor" text,
  last_polled_at timestamptz,
  next_poll_at timestamptz NOT NULL DEFAULT now(),
  consecutive_errors integer NOT NULL DEFAULT 0,
  last_error text,
  lease_owner text,
  lease_until timestamptz,
  PRIMARY KEY (user_id, source, scope_key, entity_kind)
);
CREATE INDEX sync_state_due_idx ON sync_state (next_poll_at);
ALTER TABLE sync_state ENABLE ROW LEVEL SECURITY;
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP).await.map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS sync_state;")
            .await
            .map(|_| ())
    }
}
