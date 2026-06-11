//! Needs-you inbox: unread QA messages, failed runs, review requests, and due
//! reminders, ranked heuristically (QA + failed runs first, then reviews,
//! then reminders; recency within each band). AI ranking/brief plug in via
//! the `ai` module when the user's toggle is on.

use std::collections::HashMap;

use sea_orm::prelude::Uuid;
use wf_db::tables::{reminders, slack_messages};
use wf_github::GithubQueueKey;

use crate::error::AppError;
use crate::hub::types::{HubInbox, HubInboxItem, HubInboxQa, HubInboxReview, HubInboxRun};
use crate::notes::routes::reminder_of;
use crate::state::AppState;

const MAX_QA: usize = 12;
const MAX_REVIEWS: usize = 8;
const MAX_RUNS: usize = 6;

pub async fn inbox(state: &AppState, user_id: Uuid) -> Result<HubInbox, AppError> {
    let mut items: Vec<(i64, HubInboxItem)> = Vec::new();

    // QA — latest unread Slack message per ticket, unread count attached.
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
    for (key, (count, latest)) in per_ticket.into_iter().take(MAX_QA) {
        items.push((
            0,
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
            },
        ));
    }

    // Failed runs — from the actions-strip snapshot (cached).
    if let Ok(runs) = crate::hub::runs::runs(state, user_id).await {
        for pill in runs.runs.iter().filter(|p| p.status == "failed").take(MAX_RUNS) {
            items.push((
                0,
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
                },
            ));
        }
    }

    // Review requests — the existing dashboard queue.
    if let Ok(queue) =
        crate::github::dashboard::get_queue(state, user_id, GithubQueueKey::ReviewRequested).await
    {
        for pull in queue.pull_requests.iter().take(MAX_REVIEWS) {
            items.push((
                1,
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
                },
            ));
        }
    }

    // Due reminders.
    for row in reminders::list_due(&state.db, user_id).await? {
        items.push((
            2,
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
            },
        ));
    }

    // Heuristic rank: band asc, recency desc inside a band.
    items.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.occurred_at.cmp(&a.1.occurred_at)));
    let ranked: Vec<HubInboxItem> = items
        .into_iter()
        .enumerate()
        .map(|(i, (_, mut item))| {
            item.urgency = i as i64;
            item
        })
        .collect();

    Ok(HubInbox { items: ranked, brief: None, ranked_by: "heuristic".to_string() })
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}
