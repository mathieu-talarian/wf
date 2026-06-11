//! Slack Web API integration: a Bearer-auth client for the bot token plus the
//! typed calls the API/sync layers need (auth.test validation, channel
//! listing, history/replies polling, posting replies, permalinks).

mod client;
mod errors;
mod types;

pub use client::SlackClient;
pub use errors::{SlackApiError, SlackTokenState};
pub use types::{
    SlackAuthTest, SlackChannelInfo, SlackPostedMessage, SlackRawMessage, SlackUserProfile,
};
