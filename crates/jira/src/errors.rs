//! Jira error types (port of `jira/errors.ts`).

use thiserror::Error;

/// Any non-2xx (or unexpected redirect / transport failure) from the Jira REST
/// API. `status` is 0 for transport failures (parallel to the TS `JiraApiError`).
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct JiraApiError {
    pub status: u16,
    pub message: String,
    pub error_messages: Vec<String>,
}

impl JiraApiError {
    pub fn new(status: u16, message: impl Into<String>, error_messages: Vec<String>) -> Self {
        Self { status, message: message.into(), error_messages }
    }

    /// Transport failure (couldn't reach Jira); status 0.
    pub fn transport() -> Self {
        Self { status: 0, message: "Jira request failed".to_string(), error_messages: vec![] }
    }

    /// A redirect response — credentials must never be replayed to the target.
    pub fn redirect() -> Self {
        Self { status: 502, message: "Unexpected Jira redirect".to_string(), error_messages: vec![] }
    }
}

/// Raised by write operations so the route layer can surface the upstream Jira
/// status (400/403/404/...) instead of a generic 502 (port of `JiraWriteError`).
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct JiraWriteError {
    pub status: u16,
    pub message: String,
}

/// No Jira credentials connected for this user (port of `JiraNotConnectedError`).
#[derive(Debug, Clone, Error)]
#[error("{0}")]
pub struct JiraNotConnected(pub String);
