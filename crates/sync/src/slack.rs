//! Slack channel polling (hub contract §6). One scope per watched channel
//! (`source = "slack"`, `entity_kind = "channel"`, `scope_key = channel id`).
//! Each poll ingests new channel messages (plus full threads for roots with
//! replies) into `slack_messages`, matching the Jira key regex in the text.
//! The cursor is the max Slack `ts` seen; the **baseline poll backfills one
//! page as already-read** so a fresh connection doesn't flood the inbox.

use std::collections::{HashMap, HashSet};

use futures::{stream, StreamExt, TryStreamExt};
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use wf_db::tables::slack_messages::{self as messages, UpsertSlackMessageInput};
use wf_db::Db;
use wf_slack::{SlackClient, SlackRawMessage};

const PROVIDER_CONCURRENCY: usize = 4;

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
    let raw = expand_threads(client, channel_id, &page).await?;
    let inputs = build_inputs(client, channel_id, channel_name, &raw, bot_user_id, baseline).await;

    let new_cursor = page.iter().map(|m| m.ts.clone()).max();
    let written = messages::upsert_many(db, user_id, inputs).await.map_err(|e| e.to_string())?;
    Ok(SlackPollOutcome { written, new_cursor: new_cursor.or_else(|| cursor.map(str::to_string)) })
}

/// Expands every thread root in the page (a root with replies) into its full
/// reply set so replies inherit the root's ticket match, then returns all
/// messages sorted by `ts` and de-duplicated.
async fn expand_threads(
    client: &SlackClient,
    channel_id: &str,
    page: &[SlackRawMessage],
) -> Result<Vec<SlackRawMessage>, String> {
    let batches = stream::iter(page.iter().cloned())
        .map(|message| expand_message(client, channel_id, message))
        .buffer_unordered(PROVIDER_CONCURRENCY)
        .try_collect::<Vec<_>>()
        .await?;
    let mut raw = batches.into_iter().flatten().collect::<Vec<_>>();
    raw.sort_by(|a, b| a.ts.cmp(&b.ts));
    raw.dedup_by(|a, b| a.ts == b.ts);
    Ok(raw)
}

async fn expand_message(
    client: &SlackClient,
    channel_id: &str,
    message: SlackRawMessage,
) -> Result<Vec<SlackRawMessage>, String> {
    let is_root = message.thread_ts.as_deref().is_none_or(|ts| ts == message.ts);
    if message.reply_count.unwrap_or(0) == 0 || !is_root {
        return Ok(vec![message]);
    }
    client.replies(channel_id, &message.ts).await.map_err(|error| error.to_string())
}

/// Maps each thread root `ts` to the ticket key in its text, so replies can
/// inherit it when their own text carries no key.
fn collect_root_keys(raw: &[SlackRawMessage]) -> HashMap<String, Option<String>> {
    let mut root_keys: HashMap<String, Option<String>> = HashMap::new();
    for message in raw {
        let root = message.thread_ts.clone().unwrap_or_else(|| message.ts.clone());
        if root == message.ts {
            root_keys.insert(root, match_ticket_key(&message.text));
        }
    }
    root_keys
}

/// Turns expanded messages into upsert rows (skipping any with an unparseable ts).
async fn build_inputs(
    client: &SlackClient,
    channel_id: &str,
    channel_name: &str,
    raw: &[SlackRawMessage],
    bot_user_id: Option<&str>,
    baseline: bool,
) -> Vec<UpsertSlackMessageInput> {
    let root_keys = collect_root_keys(raw);
    let profiles = load_profiles(client, raw).await;
    let context = MessageContext { channel_id, channel_name, bot_user_id, baseline };
    raw.iter()
        .filter_map(|message| message_to_input(&profiles, &root_keys, message, context))
        .collect()
}

async fn load_profiles(
    client: &SlackClient,
    raw: &[SlackRawMessage],
) -> HashMap<String, (String, Option<String>)> {
    let ids = raw.iter().filter_map(profile_id).collect::<HashSet<_>>();
    stream::iter(ids)
        .map(|id| load_profile(client, id))
        .buffer_unordered(PROVIDER_CONCURRENCY)
        .collect()
        .await
}

async fn load_profile(client: &SlackClient, id: String) -> (String, (String, Option<String>)) {
    let profile = client.user_profile(&id).await;
    let value = profile
        .map(|profile| (profile.display_name, profile.image_72))
        .unwrap_or_else(|_| (id.clone(), None));
    (id, value)
}

fn profile_id(message: &SlackRawMessage) -> Option<String> {
    message.user.clone()
}

/// Per-poll constants threaded into each message's row construction.
#[derive(Clone, Copy)]
struct MessageContext<'a> {
    channel_id: &'a str,
    channel_name: &'a str,
    bot_user_id: Option<&'a str>,
    baseline: bool,
}

/// Builds one upsert row, resolving the author (memoized in `profiles`) and the
/// inherited thread ticket key. Returns `None` if the Slack ts won't parse.
fn message_to_input(
    profiles: &HashMap<String, (String, Option<String>)>,
    root_keys: &HashMap<String, Option<String>>,
    message: &SlackRawMessage,
    ctx: MessageContext<'_>,
) -> Option<UpsertSlackMessageInput> {
    let thread_ts = message.thread_ts.clone().unwrap_or_else(|| message.ts.clone());
    let ticket_key = match_ticket_key(&message.text)
        .or_else(|| root_keys.get(&thread_ts).cloned().flatten());
    let is_bot = message.bot_id.is_some()
        || message.user.as_deref().is_some_and(|u| Some(u) == ctx.bot_user_id);
    let (author_id, author_name, avatar) = resolve_author(profiles, message, is_bot);
    let posted_at = ts_to_datetime(&message.ts)?;
    Some(UpsertSlackMessageInput {
        channel_id: ctx.channel_id.to_string(),
        channel_name: ctx.channel_name.to_string(),
        ts: message.ts.clone(),
        thread_ts,
        author_id,
        author_name,
        author_avatar_url: avatar,
        is_bot,
        body: message.text.clone(),
        ticket_key,
        is_read: ctx.baseline || is_bot,
        posted_at,
    })
}

/// Uses the profiles loaded for this poll; bots and lookup failures degrade to ids.
fn resolve_author(
    profiles: &HashMap<String, (String, Option<String>)>,
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
    if let Some((name, avatar)) = profiles.get(&id) {
        return (id.clone(), name.clone(), avatar.clone());
    }
    (id.clone(), id, None)
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
