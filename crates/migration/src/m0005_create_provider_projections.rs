use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE INDEX IF NOT EXISTS slack_messages_unread_ticket_latest_idx
  ON slack_messages (user_id, ticket_key, posted_at DESC, id DESC)
  WHERE ticket_key IS NOT NULL AND is_read = false AND is_bot = false;

CREATE INDEX IF NOT EXISTS events_jira_issue_status_idx
  ON events (user_id, scope_key, ((payload->>'issueKey')), id DESC)
  WHERE source = 'jira' AND payload ? 'statusId';

CREATE TABLE github_pull_requests (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  repo text NOT NULL,
  number bigint NOT NULL,
  state text NOT NULL,
  title text NOT NULL,
  url text NOT NULL,
  draft boolean NOT NULL DEFAULT false,
  head_ref text,
  author_login text,
  assignee_logins jsonb NOT NULL DEFAULT '[]'::jsonb,
  requested_reviewer_logins jsonb NOT NULL DEFAULT '[]'::jsonb,
  updated_at timestamptz NOT NULL,
  synced_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (user_id, repo, number)
);
CREATE INDEX github_pr_current_idx
  ON github_pull_requests (user_id, state, updated_at DESC);
CREATE INDEX github_pr_repo_idx
  ON github_pull_requests (user_id, repo, updated_at DESC);
ALTER TABLE github_pull_requests ENABLE ROW LEVEL SECURITY;

CREATE TABLE github_workflow_runs (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  repo text NOT NULL,
  run_id bigint NOT NULL,
  workflow_id bigint,
  name text NOT NULL,
  status text NOT NULL,
  conclusion text,
  url text NOT NULL,
  head_branch text,
  created_at timestamptz NOT NULL,
  updated_at timestamptz NOT NULL,
  synced_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (user_id, repo, run_id)
);
CREATE INDEX github_runs_current_idx
  ON github_workflow_runs (user_id, updated_at DESC);
CREATE INDEX github_runs_repo_idx
  ON github_workflow_runs (user_id, repo, updated_at DESC);
ALTER TABLE github_workflow_runs ENABLE ROW LEVEL SECURITY;

CREATE TABLE jira_issues (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  project text NOT NULL,
  issue_key text NOT NULL,
  summary text NOT NULL,
  status_id text NOT NULL,
  status_name text NOT NULL,
  status_category text NOT NULL,
  assignee_name text,
  priority_name text,
  issue_type_name text,
  created_at timestamptz NOT NULL,
  updated_at timestamptz NOT NULL,
  url text NOT NULL,
  synced_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (user_id, issue_key)
);
CREATE INDEX jira_issues_current_idx
  ON jira_issues (user_id, project, updated_at DESC);
CREATE INDEX jira_issues_status_idx
  ON jira_issues (user_id, status_id, updated_at DESC);
ALTER TABLE jira_issues ENABLE ROW LEVEL SECURITY;
"#;

const DOWN: &str = r#"
DROP TABLE IF EXISTS jira_issues;
DROP TABLE IF EXISTS github_workflow_runs;
DROP TABLE IF EXISTS github_pull_requests;
DROP INDEX IF EXISTS events_jira_issue_status_idx;
DROP INDEX IF EXISTS slack_messages_unread_ticket_latest_idx;
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
