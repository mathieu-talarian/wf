//! Slack data flows: ticket-matched threads (served from the synced
//! `slack_messages` rows), replies (live `chat.postMessage`, echoed into the
//! local table), and mark-read.

use std::collections::BTreeMap;

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use wf_db::tables::slack_connections;
use wf_db::tables::slack_messages::{self as messages, UpsertSlackMessageInput};
use wf_slack::SlackClient;

use crate::error::AppError;
use crate::slack::pat;
use crate::slack::summary::{self, SlackThread, SlackThreadsResult};
use crate::state::AppState;

/// Threads are served from the DB (populated by the sync tick); permalinks are
/// fetched live per thread root, best-effort (a missing permalink never fails
/// the read).
pub async fn threads(
    state: &AppState,
    user_id: Uuid,
    ticket_key: &str,
) -> Result<SlackThreadsResult, AppError> {
    let rows = messages::list_for_ticket(&state.db, user_id, ticket_key).await?;
    let unread_count = rows.iter().filter(|r| !r.is_read && !r.is_bot).count() as i64;
    let grouped = group_by_thread(&rows);
    let client = client_if_connected(state, user_id).await;
    let mut threads = build_threads(client, grouped).await;
    // Newest activity first.
    threads.sort_by(|a, b| {
        let last = |t: &SlackThread| t.messages.last().map(|m| m.posted_at.clone());
        last(b).cmp(&last(a))
    });
    Ok(SlackThreadsResult { ticket_key: ticket_key.to_string(), unread_count, threads })
}

/// Groups rows by (channel, thread_ts), preserving chronological order.
fn group_by_thread(rows: &[messages::Model]) -> BTreeMap<(String, String), Vec<&messages::Model>> {
    let mut grouped: BTreeMap<(String, String), Vec<&messages::Model>> = BTreeMap::new();
    for row in rows {
        grouped
            .entry((row.channel_id.clone(), row.thread_ts.clone()))
            .or_default()
            .push(row);
    }
    grouped
}

/// Builds thread views, fetching each root's permalink best-effort.
async fn build_threads(
    client: Option<SlackClient>,
    grouped: BTreeMap<(String, String), Vec<&messages::Model>>,
) -> Vec<SlackThread> {
    let mut threads = Vec::with_capacity(grouped.len());
    for ((channel_id, thread_ts), group) in grouped {
        let channel_name = group[0].channel_name.clone();
        let permalink = match &client {
            Some(client) => client.permalink(&channel_id, &thread_ts).await.ok(),
            None => None,
        };
        threads.push(SlackThread {
            channel_id,
            channel_name,
            thread_ts,
            permalink,
            messages: group.into_iter().map(summary::message_of).collect(),
        });
    }
    threads
}

async fn client_if_connected(state: &AppState, user_id: Uuid) -> Option<SlackClient> {
    let row = wf_db::tables::slack_connections::select_row(&state.db, user_id).await.ok()??;
    let token = pat::open_token(state, &row).ok()?;
    Some(SlackClient::new(&token))
}

/// Posts a reply into a thread and echoes it into `slack_messages` (as read —
/// it's our own message) so the thread view updates without waiting a tick.
pub async fn reply(
    state: &AppState,
    user_id: Uuid,
    channel_id: &str,
    thread_ts: &str,
    text: &str,
) -> Result<summary::SlackMessage, AppError> {
    let row = pat::require_row(state, user_id).await?;
    let token = pat::open_token(state, &row)?;
    let posted = match SlackClient::new(&token).post_message(channel_id, thread_ts, text).await {
        Ok(posted) => posted,
        Err(e) => {
            pat::note_token_error(state, user_id, &e).await;
            return Err(e.into());
        }
    };
    let posted_at: DateTimeWithTimeZone = chrono::Utc::now().into();
    let echo = ReplyEcho {
        channel_id,
        thread_ts,
        text,
        bot_user_id: row.bot_user_id.clone().unwrap_or_default(),
        ts: posted.ts.clone(),
        posted_at,
    };
    echo_reply(state, user_id, echo).await?;
    Ok(reply_message(&row, posted.ts, text, posted_at))
}

/// The fields needed to echo a just-posted reply into `slack_messages`.
struct ReplyEcho<'a> {
    channel_id: &'a str,
    thread_ts: &'a str,
    text: &'a str,
    bot_user_id: String,
    ts: String,
    posted_at: DateTimeWithTimeZone,
}

/// Echoes our own reply into the local table (as read) so the thread view
/// updates without waiting for a sync tick. Channel name + ticket key are
/// inherited from the existing thread rows.
async fn echo_reply(state: &AppState, user_id: Uuid, echo: ReplyEcho<'_>) -> Result<(), AppError> {
    let thread_rows =
        messages::list_for_ticket_thread(&state.db, user_id, echo.channel_id, echo.thread_ts).await?;
    let channel_name = thread_rows
        .first()
        .map(|r| r.channel_name.clone())
        .unwrap_or_else(|| echo.channel_id.to_string());
    let ticket_key = thread_rows.first().and_then(|r| r.ticket_key.clone());
    let input = UpsertSlackMessageInput {
        channel_id: echo.channel_id.to_string(),
        channel_name,
        ts: echo.ts,
        thread_ts: echo.thread_ts.to_string(),
        author_id: echo.bot_user_id,
        author_name: "you".to_string(),
        author_avatar_url: None,
        is_bot: true,
        body: echo.text.to_string(),
        ticket_key,
        is_read: true,
        posted_at: echo.posted_at,
    };
    messages::upsert_many(&state.db, user_id, vec![input]).await?;
    Ok(())
}

fn reply_message(
    row: &slack_connections::Model,
    ts: String,
    text: &str,
    posted_at: DateTimeWithTimeZone,
) -> summary::SlackMessage {
    summary::SlackMessage {
        ts,
        author: summary::SlackAuthor {
            id: row.bot_user_id.clone().unwrap_or_default(),
            display_name: "you".to_string(),
            avatar_url: None,
        },
        text: text.to_string(),
        posted_at: posted_at.to_rfc3339(),
        unread: false,
        is_bot: true,
    }
}

pub async fn mark_read(state: &AppState, user_id: Uuid, ticket_key: &str) -> Result<(), AppError> {
    messages::mark_read(&state.db, user_id, ticket_key).await?;
    Ok(())
}
