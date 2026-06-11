use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TABLE slack_connections (
  user_id uuid NOT NULL PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  bot_token_ciphertext text NOT NULL,
  bot_token_iv text NOT NULL,
  bot_token_auth_tag text NOT NULL,
  team_name text,
  bot_user_id text,
  watched_channels jsonb,
  last_validated_at timestamptz,
  validation_status text NOT NULL DEFAULT 'unchecked',
  validation_error text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE slack_connections ENABLE ROW LEVEL SECURITY;

CREATE TABLE slack_messages (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  channel_id text NOT NULL,
  channel_name text NOT NULL,
  ts text NOT NULL,
  thread_ts text NOT NULL,
  author_id text NOT NULL,
  author_name text NOT NULL,
  author_avatar_url text,
  is_bot boolean NOT NULL DEFAULT false,
  body text NOT NULL,
  ticket_key text,
  is_read boolean NOT NULL DEFAULT false,
  posted_at timestamptz NOT NULL,
  ingested_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX slack_messages_user_channel_ts_idx
  ON slack_messages (user_id, channel_id, ts);
CREATE INDEX slack_messages_ticket_idx ON slack_messages (user_id, ticket_key);
ALTER TABLE slack_messages ENABLE ROW LEVEL SECURITY;

CREATE TABLE notes (
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  ticket_key text NOT NULL,
  body text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, ticket_key)
);
ALTER TABLE notes ENABLE ROW LEVEL SECURITY;

CREATE TABLE note_links (
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  from_ticket text NOT NULL,
  to_ticket text NOT NULL,
  snippet text NOT NULL,
  PRIMARY KEY (user_id, from_ticket, to_ticket)
);
CREATE INDEX note_links_to_idx ON note_links (user_id, to_ticket);
ALTER TABLE note_links ENABLE ROW LEVEL SECURITY;

CREATE TABLE reminders (
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  id text NOT NULL,
  ticket_key text NOT NULL,
  body text NOT NULL,
  due_at timestamptz NOT NULL,
  state text NOT NULL DEFAULT 'pending',
  snoozed_until timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, id)
);
CREATE INDEX reminders_ticket_idx ON reminders (user_id, ticket_key);
ALTER TABLE reminders ENABLE ROW LEVEL SECURITY;

CREATE TABLE ticket_links (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  ticket_key text NOT NULL,
  repo text NOT NULL,
  pr_number bigint,
  branch text,
  created_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT ticket_links_target CHECK (
    (pr_number IS NOT NULL AND branch IS NULL)
    OR (pr_number IS NULL AND branch IS NOT NULL)
  )
);
CREATE UNIQUE INDEX ticket_links_pr_idx
  ON ticket_links (user_id, repo, pr_number) WHERE pr_number IS NOT NULL;
CREATE UNIQUE INDEX ticket_links_branch_idx
  ON ticket_links (user_id, repo, branch) WHERE branch IS NOT NULL;
ALTER TABLE ticket_links ENABLE ROW LEVEL SECURITY;

ALTER TABLE jira_pat_connections ADD COLUMN board_mapping jsonb;
ALTER TABLE users ADD COLUMN ai_settings jsonb;
"#;

const DOWN: &str = r#"
ALTER TABLE users DROP COLUMN IF EXISTS ai_settings;
ALTER TABLE jira_pat_connections DROP COLUMN IF EXISTS board_mapping;
DROP TABLE IF EXISTS ticket_links;
DROP TABLE IF EXISTS reminders;
DROP TABLE IF EXISTS note_links;
DROP TABLE IF EXISTS notes;
DROP TABLE IF EXISTS slack_messages;
DROP TABLE IF EXISTS slack_connections;
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP).await.map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(DOWN).await.map(|_| ())
    }
}
