//! Slack channel polling (hub contract §6). One scope per watched channel
//! (`source = "slack"`, `entity_kind = "channel"`, `scope_key = channel id`).
//! Each poll ingests new channel messages (plus full threads for roots with
//! replies) into `slack_messages`, matching the Jira key regex in the text.
//! The cursor is the max Slack `ts` seen; the **baseline poll backfills one
//! page as already-read** so a fresh connection doesn't flood the inbox.

use std::collections::HashMap;

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use wf_db::tables::slack_messages::{self as messages, UpsertSlackMessageInput};
use wf_db::Db;
use wf_slack::{SlackClient, SlackRawMessage};

/// `\b[A-Z][A-Z0-9]+-\d+\b` — any Jira-shaped key; the hub board narrows to
/// keys it actually knows.
pub fn ticket_key_regex() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\b[A-Z][A-Z0-9]+-\d+\b").expect("static regex compiles"))
}

pub fn match_ticket_key(text: &str) -> Option<String> {
    ticket_key_regex().find(text).map(|m| m.as_str().to_string())
}

pub struct SlackPollOutcome {
    pub written: u64,
    pub new_cursor: Option<String>,
}

/// Polls one channel scope. `cursor` is the last seen Slack ts.
pub async fn poll_channel(
    db: &Db,
    client: &SlackClient,
    user_id: Uuid,
    channel_id: &str,
    channel_name: &str,
    bot_user_id: Option<&str>,
    cursor: Option<&str>,
) -> Result<SlackPollOutcome, String> {
    let baseline = cursor.is_none();
    let limit = if baseline { 50 } else { 100 };
    let page = client
        .history(channel_id, cursor, limit)
        .await
        .map_err(|e| e.to_string())?;
    if page.is_empty() {
        return Ok(SlackPollOutcome { written: 0, new_cursor: cursor.map(str::to_string) });
    }

    // Expand threads: any root in the page with replies gets fully fetched so
    // replies inherit the root's ticket match.
    let mut raw: Vec<SlackRawMessage> = Vec::new();
    for message in &page {
        if message.reply_count.unwrap_or(0) > 0 && message.thread_ts.as_deref().is_none_or(|t| t == message.ts) {
            let thread =
                client.replies(channel_id, &message.ts).await.map_err(|e| e.to_string())?;
            raw.extend(thread);
        } else {
            raw.push(message.clone());
        }
    }
    raw.sort_by(|a, b| a.ts.cmp(&b.ts));
    raw.dedup_by(|a, b| a.ts == b.ts);

    // Thread roots' ticket keys propagate to their replies.
    let mut root_keys: HashMap<String, Option<String>> = HashMap::new();
    for message in &raw {
        let root = message.thread_ts.clone().unwrap_or_else(|| message.ts.clone());
        if root == message.ts {
            root_keys.insert(root, match_ticket_key(&message.text));
        }
    }

    let mut profiles: HashMap<String, (String, Option<String>)> = HashMap::new();
    let mut inputs = Vec::with_capacity(raw.len());
    for message in &raw {
        let thread_ts = message.thread_ts.clone().unwrap_or_else(|| message.ts.clone());
        let ticket_key = match_ticket_key(&message.text)
            .or_else(|| root_keys.get(&thread_ts).cloned().flatten());
        let is_bot = message.bot_id.is_some()
            || message.user.as_deref().is_some_and(|u| Some(u) == bot_user_id);
        let (author_id, author_name, avatar) =
            resolve_author(client, &mut profiles, message, is_bot).await;
        let Some(posted_at) = ts_to_datetime(&message.ts) else { continue };
        inputs.push(UpsertSlackMessageInput {
            channel_id: channel_id.to_string(),
            channel_name: channel_name.to_string(),
            ts: message.ts.clone(),
            thread_ts,
            author_id,
            author_name,
            author_avatar_url: avatar,
            is_bot,
            body: message.text.clone(),
            ticket_key,
            is_read: baseline || is_bot,
            posted_at,
        });
    }

    let new_cursor = page.iter().map(|m| m.ts.clone()).max();
    let written = messages::upsert_many(db, user_id, inputs).await.map_err(|e| e.to_string())?;
    Ok(SlackPollOutcome { written, new_cursor: new_cursor.or_else(|| cursor.map(str::to_string)) })
}

/// `users.info`, memoized per poll; bots and lookup failures degrade to ids.
async fn resolve_author(
    client: &SlackClient,
    cache: &mut HashMap<String, (String, Option<String>)>,
    message: &SlackRawMessage,
    is_bot: bool,
) -> (String, String, Option<String>) {
    let id = message
        .user
        .clone()
        .or_else(|| message.bot_id.clone())
        .unwrap_or_else(|| "unknown".to_string());
    if is_bot && message.user.is_none() {
        return (id.clone(), "bot".to_string(), None);
    }
    if let Some((name, avatar)) = cache.get(&id) {
        return (id.clone(), name.clone(), avatar.clone());
    }
    let (name, avatar) = match client.user_profile(&id).await {
        Ok(profile) => (profile.display_name, profile.image_72),
        Err(_) => (id.clone(), None),
    };
    cache.insert(id.clone(), (name.clone(), avatar.clone()));
    (id, name, avatar)
}

/// Slack ts (`"1718012345.000200"`) → timestamptz.
pub fn ts_to_datetime(ts: &str) -> Option<DateTimeWithTimeZone> {
    let secs: f64 = ts.parse().ok()?;
    let dt = chrono::DateTime::from_timestamp(
        secs.trunc() as i64,
        ((secs.fract()) * 1_000_000_000.0) as u32,
    )?;
    Some(dt.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_jira_shaped_keys_only() {
        assert_eq!(match_ticket_key("fix for GPT-4387 deployed"), Some("GPT-4387".to_string()));
        assert_eq!(match_ticket_key("ABC1-22 too"), Some("ABC1-22".to_string()));
        assert_eq!(match_ticket_key("no key here, just a-1 or X-"), None);
    }

    #[test]
    fn ts_parses() {
        let dt = ts_to_datetime("1718012345.000200").unwrap();
        assert_eq!(dt.timestamp(), 1_718_012_345);
        assert!(ts_to_datetime("garbage").is_none());
    }
}
