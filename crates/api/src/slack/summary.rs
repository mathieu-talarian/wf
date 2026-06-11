//! Slack DTOs for the `slack` tag: connection summary, channel options,
//! ticket-matched threads. Shapes follow the Workflow Hub backend contract.

use serde::{Deserialize, Serialize};
use wf_db::tables::slack_connections as slack;

/// `GET /me/slack` — connection state + watched channels.
#[derive(Serialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SlackConnectionSummary {
    pub connected: bool,
    pub team_name: Option<String>,
    pub bot_user_id: Option<String>,
    /// `valid` | `expiring` | `invalid` | `unchecked`
    pub validation_status: String,
    pub watched_channels: Vec<SlackChannelRef>,
    pub last_validated_at: Option<String>,
}

/// One watched channel (also the JSON shape stored in
/// `slack_connections.watched_channels`).
#[derive(Serialize, Deserialize, Clone, utoipa::ToSchema)]
pub struct SlackChannelRef {
    pub id: String,
    pub name: String,
}

/// One entry of `GET /me/slack/channels` — a channel the bot can see.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SlackChannelOption {
    pub id: String,
    pub name: String,
    pub is_member: bool,
    pub topic: Option<String>,
}

/// `GET /me/slack/threads?ticketKey=` — QA threads matched to one ticket.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SlackThreadsResult {
    pub ticket_key: String,
    pub unread_count: i64,
    pub threads: Vec<SlackThread>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SlackThread {
    pub channel_id: String,
    pub channel_name: String,
    pub thread_ts: String,
    pub permalink: Option<String>,
    pub messages: Vec<SlackMessage>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SlackMessage {
    pub ts: String,
    pub author: SlackAuthor,
    pub text: String,
    pub posted_at: String,
    pub unread: bool,
    pub is_bot: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SlackAuthor {
    pub id: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
}

pub(crate) fn channels_of(row: &slack::Model) -> Vec<SlackChannelRef> {
    row.watched_channels
        .as_ref()
        .and_then(|v| serde_json::from_value::<Vec<SlackChannelRef>>(v.clone()).ok())
        .unwrap_or_default()
}

pub(crate) fn from_row(row: Option<slack::Model>) -> SlackConnectionSummary {
    match row {
        None => SlackConnectionSummary {
            validation_status: "unchecked".to_string(),
            ..SlackConnectionSummary::default()
        },
        Some(row) => SlackConnectionSummary {
            connected: true,
            team_name: row.team_name.clone(),
            bot_user_id: row.bot_user_id.clone(),
            validation_status: row.validation_status.clone(),
            watched_channels: channels_of(&row),
            last_validated_at: row.last_validated_at.map(|t| t.to_rfc3339()),
        },
    }
}

pub(crate) fn message_of(row: &wf_db::tables::slack_messages::Model) -> SlackMessage {
    SlackMessage {
        ts: row.ts.clone(),
        author: SlackAuthor {
            id: row.author_id.clone(),
            display_name: row.author_name.clone(),
            avatar_url: row.author_avatar_url.clone(),
        },
        text: row.body.clone(),
        posted_at: row.posted_at.to_rfc3339(),
        unread: !row.is_read && !row.is_bot,
        is_bot: row.is_bot,
    }
}
