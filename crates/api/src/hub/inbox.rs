//! Needs-you inbox: unread QA messages, failed runs, review requests, and due
//! reminders, ranked heuristically (QA + failed runs first, then reviews,
//! then reminders; recency within each band). The endpoint is projection-only;
//! AI generation stays on explicit assist endpoints.

use sea_orm::prelude::Uuid;
use wf_db::tables::{
    github_pat_connections as gh, github_pull_requests, github_workflow_runs, reminders,
    slack_messages,
};

use crate::error::AppError;
use crate::hub::types::{HubInbox, HubInboxItem, HubInboxQa, HubInboxReview, HubInboxRun};
use crate::notes::routes::reminder_of;
use crate::state::AppState;

const MAX_QA: usize = 12;
const MAX_REVIEWS: usize = 8;
const MAX_RUNS: usize = 6;
const MAX_REMINDERS: usize = 12;

/// Durable read-model load: no provider I/O and no process-local data cache.
pub async fn inbox(state: &AppState, user_id: Uuid) -> Result<HubInbox, AppError> {
    compute_inbox(state, user_id).await
}

async fn compute_inbox(state: &AppState, user_id: Uuid) -> Result<HubInbox, AppError> {
    let mut items: Vec<(i64, HubInboxItem)> = Vec::new();
    let (qa, github, reminders) = tokio::join!(
        qa_items(state, user_id),
        github_items(state, user_id),
        reminder_items(state, user_id)
    );
    items.extend(qa?);
    items.extend(github?);
    items.extend(reminders?);

    Ok(HubInbox {
        items: rank(items),
        brief: None,
        ranked_by: "heuristic".to_string(),
    })
}

/// QA band: latest unread Slack message per ticket, with unread count attached.
async fn qa_items(state: &AppState, user_id: Uuid) -> Result<Vec<(i64, HubInboxItem)>, AppError> {
    let unread = slack_messages::unread_ticket_summaries(&state.db, user_id, MAX_QA as u64).await?;
    Ok(unread
        .into_iter()
        .map(|summary| (0, qa_item(summary)))
        .collect())
}

fn qa_item(summary: slack_messages::UnreadTicketSummary) -> HubInboxItem {
    HubInboxItem {
        id: format!("qa:{}:{}", summary.channel_id, summary.ts),
        kind: "qa".to_string(),
        ticket_key: Some(summary.ticket_key),
        source_label: format!("#{}", summary.channel_name),
        title: truncate(&summary.body, 140),
        occurred_at: summary.posted_at.to_rfc3339(),
        urgency: String::new(),
        qa: Some(HubInboxQa {
            channel_id: summary.channel_id,
            thread_ts: summary.thread_ts,
            permalink: None,
            unread: summary.unread,
        }),
        run: None,
        review: None,
        reminder: None,
    }
}

async fn github_items(
    state: &AppState,
    user_id: Uuid,
) -> Result<Vec<(i64, HubInboxItem)>, AppError> {
    let Some(connection) = gh::select_row(&state.db, user_id).await? else {
        return Ok(vec![]);
    };
    let repos = selected_repos(connection.selected_repos.as_ref());
    let (runs, reviews) = tokio::join!(
        run_items(state, user_id, &repos),
        review_items(state, user_id, &repos, &connection.github_login)
    );
    let mut items = runs?;
    items.extend(reviews?);
    Ok(items)
}

/// Failed-run band from the durable workflow-run projection.
async fn run_items(
    state: &AppState,
    user_id: Uuid,
    repos: &[String],
) -> Result<Vec<(i64, HubInboxItem)>, AppError> {
    let runs = github_workflow_runs::list_failed_recent(
        &state.db,
        user_id,
        repos,
        MAX_RUNS as u64,
    )
    .await?;
    Ok(runs.into_iter().map(|run| (0, run_item(run))).collect())
}

fn selected_repos(value: Option<&serde_json::Value>) -> Vec<String> {
    value.cloned().and_then(|json| serde_json::from_value(json).ok()).unwrap_or_default()
}

fn run_item(run: github_workflow_runs::Model) -> HubInboxItem {
    let conclusion = run.conclusion.clone().unwrap_or_else(|| "unknown".to_string());
    let workflow_name = run.name.clone();
    HubInboxItem {
        id: format!("run:{}:{}", run.repo, run.run_id),
        kind: "run".to_string(),
        ticket_key: None,
        source_label: format!("CI · {workflow_name}"),
        title: format!("{workflow_name} failed on {}", run.repo),
        occurred_at: run.updated_at.to_rfc3339(),
        urgency: String::new(),
        qa: None,
        run: Some(HubInboxRun {
            repo: run.repo,
            run_id: run.run_id.to_string(),
            workflow_name,
            conclusion,
            url: run.url,
        }),
        review: None,
        reminder: None,
    }
}

/// Review band from the durable pull-request projection.
async fn review_items(
    state: &AppState,
    user_id: Uuid,
    repos: &[String],
    login: &str,
) -> Result<Vec<(i64, HubInboxItem)>, AppError> {
    let pulls = github_pull_requests::list_recent(&state.db, user_id, repos, 200).await?;
    Ok(pulls
        .into_iter()
        .filter(|pull| requested(pull, login))
        .take(MAX_REVIEWS)
        .map(|pull| (1, review_item(pull)))
        .collect())
}

fn requested(pull: &github_pull_requests::Model, login: &str) -> bool {
    serde_json::from_value::<Vec<String>>(pull.requested_reviewer_logins.clone())
        .unwrap_or_default()
        .iter()
        .any(|candidate| candidate == login)
}

fn review_item(pull: github_pull_requests::Model) -> HubInboxItem {
    HubInboxItem {
        id: format!("review:{}:{}", pull.repo, pull.number),
        kind: "review".to_string(),
        ticket_key: None,
        source_label: "review requested".to_string(),
        title: pull.title.clone(),
        occurred_at: pull.updated_at.to_rfc3339(),
        urgency: String::new(),
        qa: None,
        run: None,
        review: Some(HubInboxReview {
            repo: pull.repo,
            pr_number: pull.number,
            title: pull.title.clone(),
            url: pull.url,
        }),
        reminder: None,
    }
}

/// Reminder band: reminders that are now due.
async fn reminder_items(
    state: &AppState,
    user_id: Uuid,
) -> Result<Vec<(i64, HubInboxItem)>, AppError> {
    let due = reminders::list_due(&state.db, user_id, MAX_REMINDERS as u64).await?;
    Ok(due.into_iter().map(|row| (2, reminder_item(row))).collect())
}

fn reminder_item(row: reminders::Model) -> HubInboxItem {
    HubInboxItem {
        id: format!("reminder:{}", row.id),
        kind: "reminder".to_string(),
        ticket_key: Some(row.ticket_key.clone()),
        source_label: "reminder".to_string(),
        title: row.body.clone(),
        occurred_at: row.due_at.to_rfc3339(),
        urgency: String::new(),
        qa: None,
        run: None,
        review: None,
        reminder: Some(reminder_of(row)),
    }
}

/// Heuristic rank: band ascending, recency descending within a band; the final
/// position becomes each item's `urgency`.
fn rank(mut items: Vec<(i64, HubInboxItem)>) -> Vec<HubInboxItem> {
    items.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.occurred_at.cmp(&a.1.occurred_at)));
    items
        .into_iter()
        .enumerate()
        .map(|(i, (_, mut item))| {
            item.urgency = i.to_string();
            item
        })
        .collect()
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}
