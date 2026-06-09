use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TABLE events (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  source text NOT NULL,
  type text NOT NULL,
  external_id text NOT NULL,
  scope_key text NOT NULL,
  actor text,
  title text,
  url text,
  occurred_at timestamptz NOT NULL,
  payload jsonb NOT NULL,
  ingested_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX events_user_source_external_idx ON events (user_id, source, external_id);
CREATE INDEX events_user_id_idx ON events (user_id, id);
CREATE INDEX events_user_scope_id_idx ON events (user_id, scope_key, id);
ALTER TABLE events ENABLE ROW LEVEL SECURITY;
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP).await.map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS events;")
            .await
            .map(|_| ())
    }
}
