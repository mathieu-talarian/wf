use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE INDEX IF NOT EXISTS slack_messages_unread_inbox_idx
  ON slack_messages (user_id, posted_at DESC)
  WHERE ticket_key IS NOT NULL AND is_read = false AND is_bot = false;

CREATE INDEX IF NOT EXISTS reminders_due_inbox_idx
  ON reminders (user_id, due_at, snoozed_until)
  WHERE state = 'pending';

CREATE INDEX IF NOT EXISTS events_workflow_runs_latest_idx
  ON events (user_id, scope_key, ((payload->>'runId')), occurred_at DESC, id DESC)
  WHERE source = 'github' AND type = 'github.workflow_run.completed';
"#;

const DOWN: &str = r#"
DROP INDEX IF EXISTS events_workflow_runs_latest_idx;
DROP INDEX IF EXISTS reminders_due_inbox_idx;
DROP INDEX IF EXISTS slack_messages_unread_inbox_idx;
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(UP)
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN)
            .await
            .map(|_| ())
    }
}
