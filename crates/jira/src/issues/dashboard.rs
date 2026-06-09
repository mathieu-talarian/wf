//! Multi-queue dashboard (port of `issues/dashboard.ts`). Each queue is fetched
//! concurrently and wrapped so a single failing/unsupported queue (e.g. Agile
//! sprint JQL on a non-Software site) degrades to a typed `error` marker instead
//! of failing the whole dashboard.

use super::search::{approximate_count, fetch_issue_page};
use super::jql::{build_queue_jql, JiraQueueKey, QueueJqlCtx};
use crate::client::JiraClient;
use crate::errors::JiraApiError;
use crate::status::classify_queue_failure;
use crate::types::{JiraIssuePage, JiraQueueResult, SUMMARY_FIELDS};

pub const QUEUE_KEYS: [JiraQueueKey; 5] = [
    JiraQueueKey::Assigned,
    JiraQueueKey::PreviouslyMine,
    JiraQueueKey::Reported,
    JiraQueueKey::Watching,
    JiraQueueKey::ActiveSprint,
];

fn degraded(key: JiraQueueKey, err: &JiraApiError) -> JiraQueueResult {
    let status = if err.status == 0 { None } else { Some(err.status) };
    JiraQueueResult {
        key,
        approximate_total: None,
        issues: vec![],
        next_cursor: None,
        is_last: true,
        error: Some(classify_queue_failure(key, status)),
    }
}

fn to_queue_result(key: JiraQueueKey, page: JiraIssuePage, total: Option<i64>) -> JiraQueueResult {
    JiraQueueResult {
        key,
        approximate_total: total,
        issues: page.issues,
        next_cursor: page.next_cursor,
        is_last: page.is_last,
        error: None,
    }
}

async fn load_queue(client: &JiraClient, key: JiraQueueKey, ctx: &QueueJqlCtx) -> JiraQueueResult {
    let jql = build_queue_jql(key, ctx);
    let (page, total) =
        futures::join!(fetch_issue_page(client, &jql, SUMMARY_FIELDS, None), approximate_count(client, &jql));
    match (page, total) {
        (Ok(page), Ok(total)) => to_queue_result(key, page, total),
        (Err(e), _) | (_, Err(e)) => degraded(key, &e),
    }
}

/// Fetch every queue concurrently; never fails (failures degrade per-queue).
pub async fn fetch_dashboard_queues(
    client: &JiraClient,
    ctx: &QueueJqlCtx,
) -> Vec<JiraQueueResult> {
    futures::future::join_all(QUEUE_KEYS.iter().map(|&key| load_queue(client, key, ctx))).await
}

/// A single queue's page (port of `fetchQueuePage`).
pub async fn fetch_queue_page(
    client: &JiraClient,
    key: JiraQueueKey,
    ctx: &QueueJqlCtx,
    cursor: Option<&str>,
) -> Result<JiraIssuePage, JiraApiError> {
    fetch_issue_page(client, &build_queue_jql(key, ctx), SUMMARY_FIELDS, cursor).await
}
