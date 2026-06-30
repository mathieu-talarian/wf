//! Needs-you inbox: unread QA messages, failed runs, review requests, and due
//! reminders, ranked heuristically (QA + failed runs first, then reviews,
//! then reminders; recency within each band). AI ranking/brief plug in via
//! the `ai` module when the user's toggle is on.

use std::collections::HashMap;

use sea_orm::prelude::Uuid;
use wf_db::tables::{events, reminders, slack_messages};
use wf_github::dashboard::types::GithubPullRequestQueue;
use wf_github::{GithubPullRequestBasic, GithubQueueKey};

use crate::error::AppError;
use crate::hub::cache;
use crate::hub::types::{HubInbox, HubInboxItem, HubInboxQa, HubInboxReview, HubInboxRun};
use crate::notes::routes::reminder_of;
use crate::state::AppState;

const MAX_QA: usize = 12;
const MAX_REVIEWS: usize = 8;
const MAX_RUNS: usize = 6;

/// Stale-while-revalidate + single-flight, matching `hub::runs`/`hub::board`.
/// The morning brief is never generated on this path — `compute_inbox` reads it
/// from cache and spawns generation in the background, so the request never
/// blocks on OpenAI.
pub async fn inbox(state: &AppState, user_id: Uuid) -> Result<HubInbox, AppError> {
    match cache::get_inbox_swr(user_id) {
        cache::Freshness::Fresh(v) => return Ok(v),
        cache::Freshness::Stale(v) => {
            spawn_refresh(state, user_id);
            return Ok(v);
        }
        cache::Freshness::Missing => {}
    }
    let lock = cache::refresh_lock("inbox", user_id);
    let _guard = lock.lock().await;
    if let Some(cached) = cache::get_inbox(user_id) {
        return Ok(cached);
    }
    let inbox = compute_inbox(state, user_id).await?;
    cache::put_inbox(user_id, &inbox);
    Ok(inbox)
}

/// Background revalidation, guarded so only one refresh per user runs at a time.
fn spawn_refresh(state: &AppState, user_id: Uuid) {
    let lock = cache::refresh_lock("inbox", user_id);
    let Ok(guard) = lock.try_lock_owned() else { return };
    let state = state.clone();
    actix_web::rt::spawn(async move {
        let _guard = guard;
        if let Ok(inbox) = compute_inbox(&state, user_id).await {
            cache::put_inbox(user_id, &inbox);
        }
    });
}

async fn compute_inbox(state: &AppState, user_id: Uuid) -> Result<HubInbox, AppError> {
    let mut items: Vec<(i64, HubInboxItem)> = Vec::new();
    let (qa, runs, reviews, reminders) = tokio::join!(
        qa_items(state, user_id),
        run_items(state, user_id),
        review_items(state, user_id),
        reminder_items(state, user_id)
    );
    items.extend(qa?);
    items.extend(runs?);
    items.extend(reviews);
    items.extend(reminders?);

    let ranked = rank(items);
    let brief = cache::get_brief(user_id); // cache-only — never blocks on OpenAI
    spawn_brief_if_stale(state, user_id, &ranked);
    Ok(HubInbox {
        items: ranked,
        brief,
        ranked_by: "heuristic".to_string(),
    })
}

/// QA band: latest unread Slack message per ticket, with unread count attached.
async fn qa_items(state: &AppState, user_id: Uuid) -> Result<Vec<(i64, HubInboxItem)>, AppError> {
    let unread = slack_messages::list_unread(&state.db, user_id, 100).await?;
    let mut per_ticket: HashMap<String, (i64, slack_messages::Model)> = HashMap::new();
    for row in unread {
        let Some(key) = row.ticket_key.clone() else {
            continue;
        };
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
        urgency: String::new(),
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

/// Failed-run band: latest non-success workflow run events from the sync table.
async fn run_items(state: &AppState, user_id: Uuid) -> Result<Vec<(i64, HubInboxItem)>, AppError> {
    let runs =
        events::list_recent_failed_workflow_runs(&state.db, user_id, MAX_RUNS as u64).await?;
    Ok(runs.into_iter().map(|run| (0, run_item(run))).collect())
}

fn run_item(run: events::FailedWorkflowRun) -> HubInboxItem {
    HubInboxItem {
        id: format!("run:{}:{}", run.repo, run.run_id),
        kind: "run".to_string(),
        ticket_key: None,
        source_label: format!("CI · {}", run.workflow_name),
        title: format!("{} failed on {}", run.workflow_name, run.repo),
        occurred_at: run.occurred_at.to_rfc3339(),
        urgency: String::new(),
        qa: None,
        run: Some(HubInboxRun {
            repo: run.repo,
            run_id: run.run_id,
            workflow_name: run.workflow_name,
            conclusion: run.conclusion,
            url: run.url,
        }),
        review: None,
        reminder: None,
    }
}

/// Review band: the existing review-requested dashboard queue.
async fn review_items(state: &AppState, user_id: Uuid) -> Vec<(i64, HubInboxItem)> {
    let Ok(queue) = review_queue(state, user_id, GithubQueueKey::ReviewRequested).await else {
        return Vec::new();
    };
    queue
        .pull_requests
        .iter()
        .take(MAX_REVIEWS)
        .map(|pull| (1, review_item(pull)))
        .collect()
}

async fn review_queue(
    state: &AppState,
    user_id: Uuid,
    key: GithubQueueKey,
) -> Result<GithubPullRequestQueue, AppError> {
    if let Some(cached) = cache::get_review_queue(user_id, key) {
        return Ok(cached);
    }
    let queue = crate::github::dashboard::get_queue(state, user_id, key).await?;
    cache::put_review_queue(user_id, key, &queue);
    Ok(queue)
}

fn review_item(pull: &GithubPullRequestBasic) -> HubInboxItem {
    HubInboxItem {
        id: format!("review:{}:{}", pull.repository.full_name, pull.number),
        kind: "review".to_string(),
        ticket_key: None,
        source_label: "review requested".to_string(),
        title: pull.title.clone(),
        occurred_at: pull.updated_at.clone(),
        urgency: String::new(),
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

/// Spawns AI morning-brief generation off the request path when no fresh brief
/// is cached. Single-flight (a `"brief"` lock) so concurrent inbox builds don't
/// fire duplicate OpenAI calls; the result lands in the 6h brief cache and shows
/// on the next inbox refresh. Toggle-off / no-key / failure all degrade to nothing.
fn spawn_brief_if_stale(state: &AppState, user_id: Uuid, items: &[HubInboxItem]) {
    if cache::get_brief(user_id).is_some() {
        return;
    }
    let lock = cache::refresh_lock("brief", user_id);
    let Ok(guard) = lock.try_lock_owned() else { return };
    let state = state.clone();
    let items = items.to_vec();
    actix_web::rt::spawn(async move {
        let _guard = guard;
        generate_brief(&state, user_id, &items).await;
    });
}

async fn generate_brief(state: &AppState, user_id: Uuid, items: &[HubInboxItem]) {
    if !brief_enabled(state, user_id).await.unwrap_or(false)
        || crate::ai::openai::require_key(state).is_err()
    {
        return;
    }
    let Some(summary) = complete_brief(state, items).await else {
        return;
    };
    let brief = crate::hub::types::HubBrief {
        summary,
        generated_at: chrono::Utc::now().to_rfc3339(),
    };
    cache::put_brief(user_id, &brief);
}

async fn brief_enabled(state: &AppState, user_id: Uuid) -> Option<bool> {
    Some(
        crate::ai::settings::get(state, user_id)
            .await
            .ok()?
            .morning_brief,
    )
}

async fn complete_brief(state: &AppState, items: &[HubInboxItem]) -> Option<String> {
    let digest = brief_digest(items);
    let system = "You write a 2-3 sentence 'morning brief' for a developer's \
        dashboard, summarizing what most needs their attention right now \
        (QA blockers and failed deploys first). Plain text, no lists, no greeting.";
    let prompt = if digest.is_empty() {
        "Nothing is pending. Say so in one short sentence.".to_string()
    } else {
        format!("Current items, most urgent first:\n{digest}")
    };
    crate::ai::openai::complete(state, system, &prompt)
        .await
        .ok()
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
