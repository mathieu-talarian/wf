//! Deserialization targets for the Slack Web API responses we consume. Only
//! the fields we read are declared; everything else is dropped by serde.

use serde::Deserialize;

/// `auth.test` — token validation + workspace identity.
#[derive(Debug, Clone, Deserialize)]
pub struct SlackAuthTest {
    pub team: Option<String>,
    /// The bot's own user id — used to flag our messages as `is_bot`.
    pub user_id: Option<String>,
}

/// One entry of `conversations.list`.
#[derive(Debug, Clone, Deserialize)]
pub struct SlackChannelInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub is_member: bool,
    #[serde(default)]
    pub topic: Option<SlackChannelTopic>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SlackChannelTopic {
    pub value: String,
}

/// One entry of `conversations.history` / `conversations.replies`.
#[derive(Debug, Clone, Deserialize)]
pub struct SlackRawMessage {
    pub ts: String,
    #[serde(default)]
    pub thread_ts: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub bot_id: Option<String>,
    #[serde(default)]
    pub text: String,
    /// Present on thread roots in `history`; tells us to fetch replies.
    #[serde(default)]
    pub reply_count: Option<u64>,
}

/// `chat.postMessage` result.
#[derive(Debug, Clone, Deserialize)]
pub struct SlackPostedMessage {
    pub ts: String,
}

/// `users.info` → `user.profile` projection.
#[derive(Debug, Clone, Deserialize)]
pub struct SlackUserProfile {
    pub display_name: String,
    pub real_name: Option<String>,
    pub image_72: Option<String>,
}
