//! Slack error types. The Web API returns HTTP 200 with `{ok: false, error}`
//! for most failures, so `SlackApiError` carries Slack's error code and the
//! route layer maps it onto the connection's validation state.

use thiserror::Error;

/// Any failed Slack Web API call. `status` is the HTTP status (200 for
/// `ok: false` envelopes, 0 for transport failures); `code` is Slack's error
/// string (`invalid_auth`, `channel_not_found`, …) when present.
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct SlackApiError {
    pub status: u16,
    pub code: Option<String>,
    pub message: String,
}

impl SlackApiError {
    pub fn envelope(code: &str) -> Self {
        Self {
            status: 200,
            code: Some(code.to_string()),
            message: format!("Slack responded with `{code}`"),
        }
    }

    pub fn http(status: u16) -> Self {
        Self { status, code: None, message: format!("Slack responded {status}") }
    }

    pub fn transport() -> Self {
        Self { status: 0, code: None, message: "Slack request failed".to_string() }
    }

    /// How this error reflects on the stored token, mirroring the contract's
    /// `SlackValidationStatus` values.
    pub fn token_state(&self) -> SlackTokenState {
        match self.code.as_deref() {
            Some("token_expired") => SlackTokenState::Expiring,
            Some("invalid_auth" | "token_revoked" | "account_inactive" | "not_authed") => {
                SlackTokenState::Invalid
            }
            _ => SlackTokenState::Unrelated,
        }
    }
}

/// Token verdict derived from a Slack error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackTokenState {
    /// Error says the token is expired / about to rotate.
    Expiring,
    /// Error says the token is dead.
    Invalid,
    /// Error has nothing to do with the token.
    Unrelated,
}
