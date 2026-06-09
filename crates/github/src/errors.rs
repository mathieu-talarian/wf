//! GitHub error types (port of `github/errors.ts` + `pat/validate.ts`'s
//! `PatValidationError` + the `GithubNotConnectedError`).

use thiserror::Error;

use crate::types::PatValidationStatus;

/// Token validation failure, carrying the classified status and the originating
/// HTTP status (when the failure came from a GitHub response).
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct PatValidationError {
    pub status: PatValidationStatus,
    pub message: String,
    pub http_status: Option<u16>,
}

impl PatValidationError {
    pub fn of(status: PatValidationStatus, http_status: Option<u16>) -> Self {
        Self {
            status,
            message: status.message().to_string(),
            http_status,
        }
    }

    /// Couldn't reach GitHub at all (network/transport).
    pub fn unreachable() -> Self {
        Self {
            status: PatValidationStatus::Unknown,
            message: "Could not reach GitHub to validate the token.".to_string(),
            http_status: None,
        }
    }
}

/// General GitHub API/transport errors for the data + write paths
/// (`GithubApiError` / `GithubWriteError`).
#[derive(Debug, Clone, Error)]
pub enum GithubError {
    /// Upstream read failed (transport or non-2xx on a read) → 502.
    #[error("github request failed: {0}")]
    Api(String),
    /// Write op (create PR, dispatch, merge) failed with a specific status.
    #[error("github write failed ({status}): {message}")]
    Write { status: u16, message: String },
    /// No GitHub token connected for this user → 404.
    #[error("no GitHub token connected")]
    NotConnected,
}
