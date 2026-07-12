//! The wake-up tick (A1 spec §4): reconcile scopes → claim due → poll each →
//! advance/back off. Bounded by batch + wall-clock budget; idempotent; safe
//! to run concurrently (leases). Shared by `wf-api` and the future worker.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt, TryStreamExt};
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use tracing::Instrument;
use sea_orm::DbErr;
use serde::Serialize;
use wf_core::{Sealed, TokenCipher};
use wf_db::tables::{
    events, github_pat_connections as gh, github_pull_requests as pull_rows,
    github_workflow_runs as run_rows, jira_issues as issue_rows,
    jira_pat_connections as jira, slack_connections as slack_conn, sync_state,
};
use wf_db::Db;
use wf_github::{GithubClient, PolledPullRequest, PolledWorkflowRun};
use wf_jira::{JiraClient, JiraCreds, PolledIssue};

use crate::cursor::{self, GithubCursor, JiraCursor};
use crate::normalize;

#[derive(Debug, Clone)]
pub struct TickOptions {
    pub batch: u64,
    pub budget: Duration,
    pub lease_secs: u64,
    pub poll_interval_secs: u64,
    /// Max scopes polled concurrently (bounded so in-flight scopes can't
    /// exhaust the DB pool). `0`/`1` keep the old serial behavior.
    pub concurrency: usize,
    /// Identifies this tick run in `lease_owner`.
    pub owner: String,
    /// Test seam: override the GitHub REST base (wiremock).
    pub github_base: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TickSummary {
    pub scopes_claimed: usize,
    pub scopes_ok: usize,
    pub scopes_failed: usize,
    pub events_written: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum TickError {
    #[error("tick db error: {0}")]
    Db(#[from] DbErr),
}

struct StoredConnection<T> {
    row: T,
    token: Result<String, String>,
}

impl<T> StoredConnection<T> {
    fn token(&self) -> Result<&str, ScopeError> {
        self.token.as_deref().map_err(|error| ScopeError::Poll(error.to_string()))
    }
}

struct TickConnections {
    github: HashMap<Uuid, StoredConnection<gh::Model>>,
    jira: HashMap<Uuid, StoredConnection<jira::Model>>,
    slack: HashMap<Uuid, StoredConnection<slack_conn::Model>>,
}

/// One bounded tick (spec §4.2). Errors inside a scope are isolated (recorded
/// + backed off); only DB-level failures abort the tick.
pub async fn run_tick(db: &Db, cipher: &TokenCipher, opts: &TickOptions) -> Result<TickSummary, TickError> {
    let connections = reconcile_all(db, cipher).await?;
    let claimed = sync_state::claim_due(db, opts.batch, &opts.owner, opts.lease_secs).await?;
    let claimed_count = claimed.len();
    let deadline = Instant::now() + opts.budget;
    // Independent scopes (SKIP LOCKED isolates them) poll concurrently, bounded so
    // in-flight scopes can't exhaust the DB pool. Past the budget new scopes skip —
    // their leases expire and are re-claimed next tick (spec §8).
    let deltas: Vec<Result<Option<StepDelta>, TickError>> = stream::iter(claimed)
        .map(|scope| poll_one(db, &connections, scope, opts, deadline))
        .buffer_unordered(opts.concurrency.max(1))
        .collect()
        .await;
    let mut summary = TickSummary { scopes_claimed: claimed_count, ..TickSummary::default() };
    for delta in deltas {
        if let Some(d) = delta? {
            summary.apply(d);
        }
    }
    Ok(summary)
}

/// Runs `step_scope` unless the tick budget is already spent (then the scope is
/// skipped: its lease expires and it's re-claimed next tick). Owned `scope` keeps
/// the concurrent futures free of borrows from the claimed batch.
async fn poll_one(
    db: &Db,
    connections: &TickConnections,
    scope: sync_state::Model,
    opts: &TickOptions,
    deadline: Instant,
) -> Result<Option<StepDelta>, TickError> {
    if Instant::now() >= deadline {
        return Ok(None);
    }
    let span = tracing::info_span!("tick.scope", source = %scope.source, scope = %scope.scope_key);
    step_scope(db, connections, &scope, opts).instrument(span).await.map(Some)
}

/// Polls one claimed scope, records the outcome on `sync_state`, and returns the
/// per-scope delta. Kept pure (no `&mut summary`) so scopes run concurrently.
async fn step_scope(
    db: &Db,
    connections: &TickConnections,
    scope: &sync_state::Model,
    opts: &TickOptions,
) -> Result<StepDelta, TickError> {
    match process_scope(db, connections, scope, opts).await {
        Ok(ScopeOutcome { written, new_cursor }) => {
            let cursor = new_cursor.or_else(|| scope.cursor.clone());
            sync_state::complete_ok(db, scope, &opts.owner, cursor, opts.poll_interval_secs).await?;
            Ok(StepDelta { ok: true, written })
        }
        Err(ScopeError::Db(e)) => Err(e.into()),
        Err(ScopeError::Poll(msg)) => {
            let backoff = backoff_secs(opts.poll_interval_secs, scope.consecutive_errors);
            tracing::warn!(scope = %scope.scope_key, kind = %scope.entity_kind, error = %msg, "scope poll failed");
            sync_state::complete_err(db, scope, &opts.owner, &msg, backoff).await?;
            Ok(StepDelta { ok: false, written: 0 })
        }
    }
}

/// Per-scope outcome folded into `TickSummary` after the concurrent poll.
struct StepDelta {
    ok: bool,
    written: u64,
}

impl TickSummary {
    fn apply(&mut self, d: StepDelta) {
        self.scopes_ok += usize::from(d.ok);
        self.scopes_failed += usize::from(!d.ok);
        self.events_written += d.written;
    }
}

struct ScopeOutcome {
    written: u64,
    new_cursor: Option<String>,
}

/// Scope-level failure split: `Db` aborts the tick, `Poll` is isolated.
enum ScopeError {
    Db(DbErr),
    Poll(String),
}

impl From<DbErr> for ScopeError {
    fn from(e: DbErr) -> Self {
        ScopeError::Db(e)
    }
}

/// Exponential backoff capped at 1h: `interval * 2^min(errors, 5)`.
pub fn backoff_secs(interval: u64, consecutive_errors: i32) -> u64 {
    let exp = consecutive_errors.clamp(0, 5) as u32;
    (interval.saturating_mul(2u64.saturating_pow(exp))).min(3600)
}

/// Rebuilds `sync_state` rows from every connection's selections (spec §4.2.1).
/// Connections whose PAT is known-invalid get their scopes removed (spec §8).
async fn reconcile_all(db: &Db, cipher: &TokenCipher) -> Result<TickConnections, DbErr> {
    let (github, jira, slack) = futures::join!(
        gh::list_all(db),
        jira::list_all(db),
        slack_conn::list_all(db),
    );
    let (github, jira, slack) = (github?, jira?, slack?);
    reconcile_github(db, &github).await?;
    reconcile_jira(db, &jira).await?;
    reconcile_slack(db, &slack).await?;
    Ok(TickConnections {
        github: github_connections(cipher, github),
        jira: jira_connections(cipher, jira),
        slack: slack_connections(cipher, slack),
    })
}

async fn reconcile_github(db: &Db, rows: &[gh::Model]) -> Result<(), DbErr> {
    stream::iter(rows.iter().cloned())
        .map(|row| async move {
            let desired = github_scopes(&row);
            sync_state::replace_scopes(db, row.user_id, "github", &desired).await
        })
        .buffer_unordered(4)
        .try_collect::<Vec<_>>()
        .await
        .map(|_| ())
}

async fn reconcile_jira(db: &Db, rows: &[jira::Model]) -> Result<(), DbErr> {
    stream::iter(rows.iter().cloned())
        .map(|row| async move {
            let desired = jira_scopes(&row);
            sync_state::replace_scopes(db, row.user_id, "jira", &desired).await
        })
        .buffer_unordered(4)
        .try_collect::<Vec<_>>()
        .await
        .map(|_| ())
}

async fn reconcile_slack(db: &Db, rows: &[slack_conn::Model]) -> Result<(), DbErr> {
    stream::iter(rows.iter().cloned())
        .map(|row| async move {
            let desired = slack_scopes(&row);
            sync_state::replace_scopes(db, row.user_id, "slack", &desired).await
        })
        .buffer_unordered(4)
        .try_collect::<Vec<_>>()
        .await
        .map(|_| ())
}

fn github_connections(
    cipher: &TokenCipher,
    rows: Vec<gh::Model>,
) -> HashMap<Uuid, StoredConnection<gh::Model>> {
    rows.into_iter()
        .map(|row| {
            let token = open_sealed(
                cipher,
                &row.access_token_ciphertext,
                &row.access_token_iv,
                &row.access_token_auth_tag,
            );
            (row.user_id, StoredConnection { row, token })
        })
        .collect()
}

fn jira_connections(
    cipher: &TokenCipher,
    rows: Vec<jira::Model>,
) -> HashMap<Uuid, StoredConnection<jira::Model>> {
    rows.into_iter()
        .map(|row| {
            let token = open_sealed(
                cipher,
                &row.api_token_ciphertext,
                &row.api_token_iv,
                &row.api_token_auth_tag,
            );
            (row.user_id, StoredConnection { row, token })
        })
        .collect()
}

fn slack_connections(
    cipher: &TokenCipher,
    rows: Vec<slack_conn::Model>,
) -> HashMap<Uuid, StoredConnection<slack_conn::Model>> {
    rows.into_iter()
        .map(|row| {
            let token = open_sealed(
                cipher,
                &row.bot_token_ciphertext,
                &row.bot_token_iv,
                &row.bot_token_auth_tag,
            );
            (row.user_id, StoredConnection { row, token })
        })
        .collect()
}

/// One scope per watched channel; the channel id is the scope key.
fn slack_scopes(conn: &slack_conn::Model) -> Vec<sync_state::ScopeKey> {
    if conn.validation_status != "valid" {
        return vec![];
    }
    watched_channels(conn)
        .into_iter()
        .map(|(id, _)| sync_state::ScopeKey { scope_key: id, entity_kind: "channel".to_string() })
        .collect()
}

/// `watched_channels` jsonb (`[{id, name}]`) → `(id, name)` pairs.
fn watched_channels(conn: &slack_conn::Model) -> Vec<(String, String)> {
    let Some(json) = conn.watched_channels.as_ref() else { return vec![] };
    serde_json::from_value::<Vec<serde_json::Value>>(json.clone())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| {
            let id = v.get("id")?.as_str()?.to_string();
            let name = v.get("name")?.as_str()?.to_string();
            Some((id, name))
        })
        .collect()
}

fn github_scopes(conn: &gh::Model) -> Vec<sync_state::ScopeKey> {
    if conn.validation_status == "invalid" {
        return vec![];
    }
    json_strings(conn.selected_repos.as_ref())
        .into_iter()
        .flat_map(|repo| {
            ["workflow_run", "pull_request"].map(|kind| sync_state::ScopeKey {
                scope_key: repo.clone(),
                entity_kind: kind.to_string(),
            })
        })
        .collect()
}

fn jira_scopes(conn: &jira::Model) -> Vec<sync_state::ScopeKey> {
    if conn.validation_status == "invalid" {
        return vec![];
    }
    json_strings(conn.selected_projects.as_ref())
        .into_iter()
        .map(|p| sync_state::ScopeKey { scope_key: p, entity_kind: "issue".to_string() })
        .collect()
}

/// `Option<Json>` → `Vec<String>` (selected repos/projects are JSON arrays).
fn json_strings(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(|j| serde_json::from_value(j.clone()).ok()).unwrap_or_default()
}

/// Routes a claimed scope to its source-specific poll.
async fn process_scope(
    db: &Db,
    connections: &TickConnections,
    scope: &sync_state::Model,
    opts: &TickOptions,
) -> Result<ScopeOutcome, ScopeError> {
    match scope.source.as_str() {
        "github" => process_github(db, connections, scope, opts).await,
        "jira" => process_jira(db, connections, scope).await,
        "slack" => process_slack(db, connections, scope).await,
        other => Err(ScopeError::Poll(format!("unknown source {other:?}"))),
    }
}

async fn process_slack(
    db: &Db,
    connections: &TickConnections,
    scope: &sync_state::Model,
) -> Result<ScopeOutcome, ScopeError> {
    let Some(connection) = connections.slack.get(&scope.user_id) else {
        sync_state::replace_scopes(db, scope.user_id, "slack", &[]).await?;
        return Ok(ScopeOutcome { written: 0, new_cursor: None });
    };
    poll_slack(db, connection.token()?, &connection.row, scope).await
}

/// Resolves the channel display name from the connection's watched list, then
/// polls that channel and maps the result into a [`ScopeOutcome`].
async fn poll_slack(
    db: &Db,
    token: &str,
    conn: &slack_conn::Model,
    scope: &sync_state::Model,
) -> Result<ScopeOutcome, ScopeError> {
    let channel_name = slack_channel_name(conn, scope);
    let client = wf_slack::SlackClient::new(token);
    let outcome = crate::slack::poll_channel(
        db,
        &client,
        scope.user_id,
        &scope.scope_key,
        &channel_name,
        conn.bot_user_id.as_deref(),
        scope.cursor.as_deref(),
    )
    .await
    .map_err(ScopeError::Poll)?;
    Ok(ScopeOutcome { written: outcome.written, new_cursor: outcome.new_cursor })
}

/// Display name for the scope's channel id, falling back to the id itself.
fn slack_channel_name(conn: &slack_conn::Model, scope: &sync_state::Model) -> String {
    watched_channels(conn)
        .into_iter()
        .find(|(id, _)| id == &scope.scope_key)
        .map(|(_, name)| name)
        .unwrap_or_else(|| scope.scope_key.clone())
}

async fn process_github(
    db: &Db,
    connections: &TickConnections,
    scope: &sync_state::Model,
    opts: &TickOptions,
) -> Result<ScopeOutcome, ScopeError> {
    let Some(connection) = connections.github.get(&scope.user_id) else {
        // Connection deleted: scope rows are orphans — self-heal.
        sync_state::replace_scopes(db, scope.user_id, "github", &[]).await?;
        return Ok(ScopeOutcome { written: 0, new_cursor: None });
    };
    let client = make_github_client(connection.token()?.to_string(), opts.github_base.as_deref());
    let (owner, repo) = split_repo(&scope.scope_key).map_err(ScopeError::Poll)?;
    let cur: Option<GithubCursor> = cursor::parse(scope.cursor.as_deref());
    poll_github_kind(db, &client, scope, &owner, &repo, cur).await
}

fn make_github_client(token: String, base: Option<&str>) -> GithubClient {
    match base {
        Some(b) => GithubClient::with_base(token, b),
        None => GithubClient::new(token),
    }
}

/// Fetches the page for the scope's entity kind, filters by cursor, inserts.
async fn poll_github_kind(
    db: &Db,
    client: &GithubClient,
    scope: &sync_state::Model,
    owner: &str,
    repo: &str,
    cur: Option<GithubCursor>,
) -> Result<ScopeOutcome, ScopeError> {
    match scope.entity_kind.as_str() {
        "workflow_run" => poll_runs(db, client, scope, owner, repo, cur).await,
        "pull_request" => poll_pulls(db, client, scope, owner, repo, cur).await,
        other => Err(ScopeError::Poll(format!("unknown github kind {other:?}"))),
    }
}

async fn poll_runs(
    db: &Db,
    client: &GithubClient,
    scope: &sync_state::Model,
    owner: &str,
    repo: &str,
    cur: Option<GithubCursor>,
) -> Result<ScopeOutcome, ScopeError> {
    let items = wf_github::list_runs_any_status(client, owner, repo)
        .await
        .map_err(|e| ScopeError::Poll(e.to_string()))?;
    let projections = items.iter().map(run_projection).collect();
    run_rows::upsert_many(db, scope.user_id, &scope.scope_key, projections).await?;
    let (new, next) = filter_new(cur.as_ref(), &items, |r| (r.updated_at, r.id));
    let evs = collect_runs(scope.user_id, &scope.scope_key, &new);
    insert(db, evs, next).await
}

async fn poll_pulls(
    db: &Db,
    client: &GithubClient,
    scope: &sync_state::Model,
    owner: &str,
    repo: &str,
    cur: Option<GithubCursor>,
) -> Result<ScopeOutcome, ScopeError> {
    let items = wf_github::list_pulls_page(client, owner, repo)
        .await
        .map_err(|e| ScopeError::Poll(e.to_string()))?;
    let projections = items.iter().map(pull_projection).collect();
    pull_rows::upsert_many(db, scope.user_id, &scope.scope_key, projections).await?;
    let (new, next) = filter_new(cur.as_ref(), &items, |p| (p.updated_at, p.number));
    let evs = collect_pulls(scope.user_id, &scope.scope_key, &new);
    insert(db, evs, next).await
}

fn collect_runs(
    user_id: Uuid,
    repo: &str,
    items: &[&PolledWorkflowRun],
) -> Vec<events::InsertEventInput> {
    items.iter().filter_map(|r| normalize::workflow_run_event(user_id, repo, r)).collect()
}

fn collect_pulls(
    user_id: Uuid,
    repo: &str,
    items: &[&PolledPullRequest],
) -> Vec<events::InsertEventInput> {
    items.iter().filter_map(|p| normalize::pull_request_event(user_id, repo, p)).collect()
}

async fn insert(
    db: &Db,
    evs: Vec<events::InsertEventInput>,
    next: Option<GithubCursor>,
) -> Result<ScopeOutcome, ScopeError> {
    let written = events::insert_ignore_dups(db, evs).await?;
    Ok(ScopeOutcome { written, new_cursor: next.as_ref().map(cursor::encode) })
}

/// Cursor filter (spec §6.2): with a cursor, keep items strictly after it; on
/// the **baseline poll (no cursor) emit nothing** — just establish the
/// watermark. Returns the kept items + the max `(updated, id)` over ALL items.
pub(crate) fn filter_new<'a, T>(
    cur: Option<&GithubCursor>,
    items: &'a [T],
    key: impl Fn(&T) -> (chrono::DateTime<chrono::Utc>, i64),
) -> (Vec<&'a T>, Option<GithubCursor>) {
    let max = items
        .iter()
        .map(&key)
        .max()
        .map(|(updated_at, id)| GithubCursor { updated_at, id });
    let next = match (&max, cur) {
        (Some(m), Some(c)) if !c.is_before(m.updated_at, m.id) => Some(c.clone()),
        (None, Some(c)) => Some(c.clone()),
        _ => max,
    };
    let kept = match cur {
        None => vec![],
        Some(c) => items.iter().filter(|t| { let (u, i) = key(t); c.is_before(u, i) }).collect(),
    };
    (kept, next)
}

async fn process_jira(
    db: &Db,
    connections: &TickConnections,
    scope: &sync_state::Model,
) -> Result<ScopeOutcome, ScopeError> {
    let Some(connection) = connections.jira.get(&scope.user_id) else {
        sync_state::replace_scopes(db, scope.user_id, "jira", &[]).await?;
        return Ok(ScopeOutcome { written: 0, new_cursor: None });
    };
    let conn = &connection.row;
    let creds = JiraCreds {
        site_url: conn.site_url.clone(),
        email: conn.email.clone(),
        token: connection.token()?.to_string(),
    };
    let site_key = conn.cloud_id.clone().unwrap_or_else(|| host_of(&conn.site_url));
    let client = JiraClient::new(&creds);
    let issues = wf_jira::fetch_recent_issues(&client, &scope.scope_key)
        .await
        .map_err(|e| ScopeError::Poll(e.to_string()))?;
    persist_jira_projection(db, scope, &issues).await?;
    let cur: Option<JiraCursor> = cursor::parse(scope.cursor.as_deref());
    let (new, next) = filter_new_jira(cur.as_ref(), &issues);
    let evs = collect_issues(db, scope, &site_key, &new).await?;
    let written = events::insert_ignore_dups(db, evs).await?;
    Ok(ScopeOutcome { written, new_cursor: next.as_ref().map(cursor::encode) })
}

async fn persist_jira_projection(
    db: &Db,
    scope: &sync_state::Model,
    issues: &[PolledIssue],
) -> Result<(), ScopeError> {
    let inputs = issues.iter().filter_map(jira_projection).collect();
    issue_rows::upsert_many(db, scope.user_id, &scope.scope_key, inputs).await?;
    Ok(())
}

fn pull_projection(pr: &PolledPullRequest) -> pull_rows::UpsertPullRequestInput {
    pull_rows::UpsertPullRequestInput {
        number: pr.number,
        state: pr.state.clone(),
        title: pr.title.clone().unwrap_or_default(),
        url: pr.html_url.clone().unwrap_or_default(),
        draft: pr.draft.unwrap_or(false),
        head_ref: pr.head.as_ref().map(|h| h.ref_name.clone()),
        author_login: pr.user.as_ref().map(|u| u.login.clone()),
        assignee_logins: pr.assignees.iter().map(|u| u.login.clone()).collect(),
        requested_reviewer_logins: pr.requested_reviewers.iter().map(|u| u.login.clone()).collect(),
        updated_at: pr.updated_at.into(),
    }
}

fn run_projection(run: &PolledWorkflowRun) -> run_rows::UpsertWorkflowRunInput {
    run_rows::UpsertWorkflowRunInput {
        run_id: run.id,
        workflow_id: run.workflow_id,
        name: run.name.clone().or_else(|| run.display_title.clone()).unwrap_or_default(),
        status: run.status.clone().unwrap_or_else(|| "unknown".into()),
        conclusion: run.conclusion.clone(),
        url: run.html_url.clone().unwrap_or_default(),
        head_branch: run.head_branch.clone(),
        created_at: run.created_at.into(),
        updated_at: run.updated_at.into(),
    }
}

fn jira_projection(issue: &PolledIssue) -> Option<issue_rows::UpsertJiraIssueInput> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let updated = issue.updated.as_deref().and_then(cursor::parse_jira_ts).unwrap_or(now);
    let created = issue.created.as_deref().and_then(cursor::parse_jira_ts).unwrap_or(updated);
    Some(issue_rows::UpsertJiraIssueInput {
        issue_key: issue.key.clone(),
        summary: issue.summary.clone().unwrap_or_default(),
        status_id: issue.status_id.clone()?,
        status_name: issue.status_name.clone().unwrap_or_default(),
        status_category: issue.status_category.clone().unwrap_or_default(),
        assignee_name: issue.assignee_name.clone(),
        priority_name: issue.priority_name.clone(),
        issue_type_name: issue.issue_type_name.clone(),
        created_at: created,
        updated_at: updated,
        url: issue.url.clone(),
    })
}

/// Jira flavor of [`filter_new`]: same baseline + strictly-after semantics
/// over `(updated, key)`.
pub(crate) fn filter_new_jira<'a>(
    cur: Option<&JiraCursor>,
    issues: &'a [wf_jira::PolledIssue],
) -> (Vec<&'a wf_jira::PolledIssue>, Option<JiraCursor>) {
    let max = issues
        .iter()
        .filter_map(|i| i.updated.as_deref().map(|u| (u.to_string(), i.key.clone())))
        .max_by(|a, b| cmp_jira(&a.0, &a.1, &b.0, &b.1))
        .map(|(updated, issue_key)| JiraCursor { updated, issue_key });
    let next = match (&max, cur) {
        (Some(m), Some(c)) if !c.is_before(&m.updated, &m.issue_key) => Some(c.clone()),
        (None, Some(c)) => Some(c.clone()),
        _ => max,
    };
    let kept = match cur {
        None => vec![],
        Some(c) => issues
            .iter()
            .filter(|i| i.updated.as_deref().is_some_and(|u| c.is_before(u, &i.key)))
            .collect(),
    };
    (kept, next)
}

fn cmp_jira(a_ts: &str, a_key: &str, b_ts: &str, b_key: &str) -> std::cmp::Ordering {
    use crate::cursor::parse_jira_ts;
    match (parse_jira_ts(a_ts), parse_jira_ts(b_ts)) {
        (Some(a), Some(b)) => a.cmp(&b).then_with(|| a_key.cmp(b_key)),
        (a, b) => a.is_some().cmp(&b.is_some()).then_with(|| a_key.cmp(b_key)),
    }
}

/// Normalizes new issues after one batched previous-status lookup.
async fn collect_issues(
    db: &Db,
    scope: &sync_state::Model,
    site_key: &str,
    issues: &[&wf_jira::PolledIssue],
) -> Result<Vec<events::InsertEventInput>, ScopeError> {
    let keys = issues.iter().map(|issue| issue.key.clone()).collect::<Vec<_>>();
    let previous =
        events::latest_jira_statuses(db, scope.user_id, &scope.scope_key, &keys).await?;
    Ok(issues
        .iter()
        .filter_map(|issue| {
            normalize::jira_issue_event(
                scope.user_id,
                &scope.scope_key,
                site_key,
                previous.get(&issue.key).map(String::as_str),
                issue,
            )
        })
        .collect())
}

/// Decrypts a sealed PAT, mapping crypto failures to a scope error string.
fn open_sealed(
    cipher: &TokenCipher,
    ciphertext: &str,
    iv: &str,
    auth_tag: &str,
) -> Result<String, String> {
    let sealed = Sealed {
        ciphertext: ciphertext.to_string(),
        iv: iv.to_string(),
        auth_tag: auth_tag.to_string(),
    };
    cipher.open(&sealed).map_err(|e| format!("token decrypt failed: {e}"))
}

/// `"owner/repo"` → `(owner, repo)`.
fn split_repo(scope_key: &str) -> Result<(String, String), String> {
    scope_key
        .split_once('/')
        .map(|(o, r)| (o.to_string(), r.to_string()))
        .ok_or_else(|| format!("malformed repo scope {scope_key:?}"))
}

fn host_of(site_url: &str) -> String {
    url::Url::parse(site_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| site_url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff_secs(120, 0), 120);
        assert_eq!(backoff_secs(120, 1), 240);
        assert_eq!(backoff_secs(120, 3), 960);
        assert_eq!(backoff_secs(120, 50), 3600);
    }

    #[test]
    fn filter_new_baseline_emits_nothing_but_advances() {
        let items = vec![(Utc.timestamp_opt(100, 0).unwrap(), 1i64)];
        let (kept, next) = filter_new(None, &items, |t| *t);
        assert!(kept.is_empty());
        assert_eq!(next.unwrap().id, 1);
    }

    #[test]
    fn filter_new_keeps_strictly_after_and_never_regresses() {
        let cur = GithubCursor { updated_at: Utc.timestamp_opt(100, 0).unwrap(), id: 5 };
        let items = vec![
            (Utc.timestamp_opt(99, 0).unwrap(), 9i64),
            (Utc.timestamp_opt(100, 0).unwrap(), 5),
            (Utc.timestamp_opt(100, 0).unwrap(), 6),
        ];
        let (kept, next) = filter_new(Some(&cur), &items, |t| *t);
        assert_eq!(kept.len(), 1);
        let next = next.unwrap();
        assert_eq!((next.updated_at, next.id), (Utc.timestamp_opt(100, 0).unwrap(), 6));

        let older = vec![(Utc.timestamp_opt(50, 0).unwrap(), 1i64)];
        let (kept, next) = filter_new(Some(&cur), &older, |t| *t);
        assert!(kept.is_empty());
        assert_eq!(next.unwrap(), cur); // never regress below the cursor
    }

    #[test]
    fn split_repo_and_host() {
        assert_eq!(split_repo("o/r").unwrap(), ("o".into(), "r".into()));
        assert!(split_repo("nope").is_err());
        assert_eq!(host_of("https://x.atlassian.net"), "x.atlassian.net");
    }
}
