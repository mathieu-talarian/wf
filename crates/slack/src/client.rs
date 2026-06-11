//! Bearer-auth reqwest wrapper for the Slack Web API. Slack signals most
//! failures as HTTP 200 + `{ok: false, error}`, so every call unwraps that
//! envelope; `base` is a test seam (wiremock) like the GitHub client's.

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;

use crate::errors::SlackApiError;
use crate::types::{
    SlackAuthTest, SlackChannelInfo, SlackPostedMessage, SlackRawMessage, SlackUserProfile,
};

const DEFAULT_BASE: &str = "https://slack.com/api";

pub struct SlackClient {
    http: reqwest::Client,
    token: String,
    base: String,
}

#[derive(Deserialize)]
struct Envelope {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    response_metadata: Option<ResponseMetadata>,
    #[serde(flatten)]
    rest: serde_json::Value,
}

#[derive(Deserialize)]
struct ResponseMetadata {
    #[serde(default)]
    next_cursor: Option<String>,
}

impl SlackClient {
    pub fn new(token: &str) -> Self {
        Self::with_base(token, DEFAULT_BASE)
    }

    pub fn with_base(token: &str, base: &str) -> Self {
        let http = reqwest::Client::builder().build().expect("reqwest client builds");
        Self { http, token: token.to_string(), base: base.trim_end_matches('/').to_string() }
    }

    /// One GET/POST round-trip, envelope unwrapped; returns the payload JSON
    /// plus the pagination cursor (empty string normalized to `None`).
    async fn call(
        &self,
        method: &str,
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<(serde_json::Value, Option<String>), SlackApiError> {
        let url = format!("{}/{method}", self.base);
        let mut req = match body {
            Some(b) => self.http.post(&url).json(b),
            None => self.http.get(&url).query(query),
        };
        req = req.bearer_auth(&self.token);
        let resp = req.send().await.map_err(|_| SlackApiError::transport())?;
        let status = resp.status().as_u16();
        if status >= 400 {
            return Err(SlackApiError::http(status));
        }
        let envelope: Envelope =
            resp.json().await.map_err(|_| SlackApiError::http(status))?;
        if !envelope.ok {
            return Err(SlackApiError::envelope(envelope.error.as_deref().unwrap_or("unknown")));
        }
        let cursor = envelope
            .response_metadata
            .and_then(|m| m.next_cursor)
            .filter(|c| !c.is_empty());
        Ok((envelope.rest, cursor))
    }

    fn field<T: DeserializeOwned>(payload: &serde_json::Value, key: &str) -> Result<T, SlackApiError> {
        serde_json::from_value(payload.get(key).cloned().unwrap_or(serde_json::Value::Null))
            .map_err(|_| SlackApiError {
                status: 200,
                code: None,
                message: format!("Invalid Slack response (missing `{key}`)"),
            })
    }

    /// `auth.test` — validates the token and returns workspace identity.
    pub async fn auth_test(&self) -> Result<SlackAuthTest, SlackApiError> {
        let (payload, _) = self.call("auth.test", &[], None).await?;
        serde_json::from_value(payload)
            .map_err(|_| SlackApiError::envelope("invalid_auth_test_response"))
    }

    /// `conversations.list` — all public channels, pagination followed.
    pub async fn list_channels(&self) -> Result<Vec<SlackChannelInfo>, SlackApiError> {
        let mut channels: Vec<SlackChannelInfo> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut query = vec![
                ("types", "public_channel".to_string()),
                ("exclude_archived", "true".to_string()),
                ("limit", "200".to_string()),
            ];
            if let Some(c) = &cursor {
                query.push(("cursor", c.clone()));
            }
            let (payload, next) = self.call("conversations.list", &query, None).await?;
            channels.extend(Self::field::<Vec<SlackChannelInfo>>(&payload, "channels")?);
            match next {
                Some(c) => cursor = Some(c),
                None => return Ok(channels),
            }
        }
    }

    /// `conversations.history` — messages in a channel newer than `oldest`
    /// (a Slack ts), oldest first. One page of up to `limit`.
    pub async fn history(
        &self,
        channel_id: &str,
        oldest: Option<&str>,
        limit: u16,
    ) -> Result<Vec<SlackRawMessage>, SlackApiError> {
        let mut query = vec![
            ("channel", channel_id.to_string()),
            ("limit", limit.to_string()),
        ];
        if let Some(oldest) = oldest {
            query.push(("oldest", oldest.to_string()));
        }
        let (payload, _) = self.call("conversations.history", &query, None).await?;
        let mut messages: Vec<SlackRawMessage> = Self::field(&payload, "messages")?;
        messages.reverse(); // Slack returns newest first
        Ok(messages)
    }

    /// `conversations.replies` — a full thread (root included), oldest first.
    pub async fn replies(
        &self,
        channel_id: &str,
        thread_ts: &str,
    ) -> Result<Vec<SlackRawMessage>, SlackApiError> {
        let query = vec![
            ("channel", channel_id.to_string()),
            ("ts", thread_ts.to_string()),
            ("limit", "200".to_string()),
        ];
        let (payload, _) = self.call("conversations.replies", &query, None).await?;
        Self::field(&payload, "messages")
    }

    /// `chat.postMessage` — reply in a thread.
    pub async fn post_message(
        &self,
        channel_id: &str,
        thread_ts: &str,
        text: &str,
    ) -> Result<SlackPostedMessage, SlackApiError> {
        let body = json!({ "channel": channel_id, "thread_ts": thread_ts, "text": text });
        let (payload, _) = self.call("chat.postMessage", &[], Some(&body)).await?;
        // ts sits at the envelope top level, not under a key.
        serde_json::from_value(payload)
            .map_err(|_| SlackApiError::envelope("invalid_post_message_response"))
    }

    /// `chat.getPermalink` — deep link for a thread root.
    pub async fn permalink(
        &self,
        channel_id: &str,
        message_ts: &str,
    ) -> Result<String, SlackApiError> {
        let query = vec![
            ("channel", channel_id.to_string()),
            ("message_ts", message_ts.to_string()),
        ];
        let (payload, _) = self.call("chat.getPermalink", &query, None).await?;
        Self::field(&payload, "permalink")
    }

    /// `users.info` — display name + avatar for message authors.
    pub async fn user_profile(&self, user_id: &str) -> Result<SlackUserProfile, SlackApiError> {
        let query = vec![("user", user_id.to_string())];
        let (payload, _) = self.call("users.info", &query, None).await?;
        let user: serde_json::Value = Self::field(&payload, "user")?;
        let real_name: Option<String> =
            user.get("real_name").and_then(|v| v.as_str()).map(String::from);
        let profile = user.get("profile").cloned().unwrap_or(serde_json::Value::Null);
        let mut parsed: SlackUserProfile = serde_json::from_value(profile)
            .unwrap_or(SlackUserProfile { display_name: String::new(), real_name: None, image_72: None });
        if parsed.display_name.is_empty() {
            parsed.display_name =
                real_name.clone().unwrap_or_else(|| user_id.to_string());
        }
        parsed.real_name = real_name;
        Ok(parsed)
    }
}
