//! Needs-you inbox: unread QA messages, failed runs, review requests, and due
//! reminders, ranked heuristically (QA + failed runs first, then reviews,
//! then reminders; recency within each band). AI ranking/brief plug in via
//! the `ai` module when the user's toggle is on.

use std::collections::HashMap;

use sea_orm::prelude::Uuid;
use wf_db::tables::{reminders, slack_messages};
use wf_github::{GithubPullRequestBasic, GithubQueueKey};

use crate::error::AppError;
use crate::hub::types::{
    HubInbox, HubInboxItem, HubInboxQa, HubInboxReview, HubInboxRun, HubRunPill,
};
use crate::notes::routes::reminder_of;
use crate::state::AppState;

const MAX_QA: usize = 12;
const MAX_REVIEWS: usize = 8;
const MAX_RUNS: usize = 6;

pub async fn inbox(state: &AppState, user_id: Uuid) -> Result<HubInbox, AppError> {
    let mut items: Vec<(i64, HubInboxItem)> = Vec::new();
    items.extend(qa_items(state, user_id).await?);
    items.extend(run_items(state, user_id).await);
    items.extend(review_items(state, user_id).await);
    items.extend(reminder_items(state, user_id).await?);

    let ranked = rank(items);
    let brief = morning_brief(state, user_id, &ranked).await;
    Ok(HubInbox { items: ranked, brief, ranked_by: "heuristic".to_string() })
}

/// QA band: latest unread Slack message per ticket, with unread count attached.
async fn qa_items(state: &AppState, user_id: Uuid) -> Result<Vec<(i64, HubInboxItem)>, AppError> {
    let unread = slack_messages::list_unread(&state.db, user_id, 100).await?;
    let mut per_ticket: HashMap<String, (i64, slack_messages::Model)> = HashMap::new();
    for row in unread {
        let Some(key) = row.ticket_key.clone() else { continue };
        let entry = per_ticket.entry(key).or_insert((0, row.clone()));
        entry.0 += 1;
        if row.posted_at > entry.1.posted_at {
            entry.1 = row;
        }
    }
    Ok(per_ticket
        .into_iter()
        .take(MAX_QA)
        .map(|(key, (count, latest))| (0, qa_item(key, count, latest)))
        .collect())
}

fn qa_item(key: String, count: i64, latest: slack_messages::Model) -> HubInboxItem {
    HubInboxItem {
        id: format!("qa:{}:{}", latest.channel_id, latest.ts),
        kind: "qa".to_string(),
        ticket_key: Some(key),
        source_label: format!("#{}", latest.channel_name),
        title: truncate(&latest.body, 140),
        occurred_at: latest.posted_at.to_rfc3339(),
        urgency: 0,
        qa: Some(HubInboxQa {
            channel_id: latest.channel_id,
            thread_ts: latest.thread_ts,
            permalink: None,
            unread: count,
        }),
        run: None,
        review: None,
        reminder: None,
    }
}

/// Failed-run band: failed entries from the cached actions-strip snapshot.
async fn run_items(state: &AppState, user_id: Uuid) -> Vec<(i64, HubInboxItem)> {
    let Ok(runs) = crate::hub::runs::runs(state, user_id).await else {
        return Vec::new();
    };
    runs.runs
        .iter()
        .filter(|p| p.status == "failed")
        .take(MAX_RUNS)
        .map(|pill| (0, run_item(pill)))
        .collect()
}

fn run_item(pill: &HubRunPill) -> HubInboxItem {
    HubInboxItem {
        id: format!("run:{}:{}", pill.repo, pill.run_id),
        kind: "run".to_string(),
        ticket_key: None,
        source_label: format!("CI · {}", pill.workflow_name),
        title: format!("{} failed on {}", pill.workflow_name, pill.repo),
        occurred_at: pill.started_at.clone(),
        urgency: 0,
        qa: None,
        run: Some(HubInboxRun {
            repo: pill.repo.clone(),
            run_id: pill.run_id,
            workflow_name: pill.workflow_name.clone(),
            conclusion: "failure".to_string(),
            url: pill.url.clone(),
        }),
        review: None,
        reminder: None,
    }
}

/// Review band: the existing review-requested dashboard queue.
async fn review_items(state: &AppState, user_id: Uuid) -> Vec<(i64, HubInboxItem)> {
    let Ok(queue) =
        crate::github::dashboard::get_queue(state, user_id, GithubQueueKey::ReviewRequested).await
    else {
        return Vec::new();
    };
    queue
        .pull_requests
        .iter()
        .take(MAX_REVIEWS)
        .map(|pull| (1, review_item(pull)))
        .collect()
}

fn review_item(pull: &GithubPullRequestBasic) -> HubInboxItem {
    HubInboxItem {
        id: format!("review:{}:{}", pull.repository.full_name, pull.number),
        kind: "review".to_string(),
        ticket_key: None,
        source_label: "review requested".to_string(),
        title: pull.title.clone(),
        occurred_at: pull.updated_at.clone(),
        urgency: 0,
        qa: None,
        run: None,
        review: Some(HubInboxReview {
            repo: pull.repository.full_name.clone(),
            pr_number: pull.number,
            title: pull.title.clone(),
            url: pull.url.clone(),
        }),
        reminder: None,
    }
}

/// Reminder band: reminders that are now due.
async fn reminder_items(
    state: &AppState,
    user_id: Uuid,
) -> Result<Vec<(i64, HubInboxItem)>, AppError> {
    let due = reminders::list_due(&state.db, user_id).await?;
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
        urgency: 0,
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
            item.urgency = i as i64;
            item
        })
        .collect()
}

/// AI morning brief: only when the user's toggle is on AND a key is
/// configured; cached 6h; failures degrade to no brief (never an error).
async fn morning_brief(
    state: &AppState,
    user_id: Uuid,
    items: &[HubInboxItem],
) -> Option<crate::hub::types::HubBrief> {
    let settings = crate::ai::settings::get(state, user_id).await.ok()?;
    if !settings.morning_brief {
        return None;
    }
    if let Some(cached) = crate::hub::cache::get_brief(user_id) {
        return Some(cached);
    }
    crate::ai::openai::require_key(state).ok()?;
    let digest = brief_digest(items);
    let system = "You write a 2-3 sentence 'morning brief' for a developer's \
        dashboard, summarizing what most needs their attention right now \
        (QA blockers and failed deploys first). Plain text, no lists, no greeting.";
    let prompt = if digest.is_empty() {
        "Nothing is pending. Say so in one short sentence.".to_string()
    } else {
        format!("Current items, most urgent first:\n{digest}")
    };
    let summary = crate::ai::openai::complete(state, system, &prompt).await.ok()?;
    let brief =
        crate::hub::types::HubBrief { summary, generated_at: chrono::Utc::now().to_rfc3339() };
    crate::hub::cache::put_brief(user_id, &brief);
    Some(brief)
}

fn brief_digest(items: &[HubInboxItem]) -> String {
    items
        .iter()
        .take(20)
        .map(|i| format!("- [{}] {} ({})", i.kind, i.title, i.source_label))
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}
