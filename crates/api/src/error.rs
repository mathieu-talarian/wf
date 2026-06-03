//! `AppError` — the HTTP-boundary error aggregate.
//!
//! Replaces Effect's `Data.TaggedError` channel. Each kind maps to a status and
//! an RFC 9457 Problem body via actix's `ResponseError` (migration plan §3.3,
//! §9). It additionally carries the request `instance` and the optional
//! `reason` extension member (the PAT/Jira validation status, Phase 3+), since
//! `ResponseError::error_response` has no access to the request.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, ResponseError};
use sea_orm::DbErr;
use wf_core::auth::AuthError;
use wf_core::problem::{detail_for_status, slug_for_status, title_for_status, ProblemDetails};

#[derive(Debug, thiserror::Error)]
enum ErrorKind {
    #[error("unauthorized: {0}")]
    Auth(String),
    #[error("validation: {0}")]
    Validation(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Db(DbErr),
    #[error("internal error: {0}")]
    Internal(anyhow::Error),
}

impl ErrorKind {
    /// The user-facing detail for 4xx kinds (5xx never leaks this).
    fn message(&self) -> &str {
        match self {
            ErrorKind::Auth(m) | ErrorKind::Validation(m) | ErrorKind::NotFound(m) => m,
            ErrorKind::Db(_) | ErrorKind::Internal(_) => "",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{kind}")]
pub struct AppError {
    kind: ErrorKind,
    instance: Option<String>,
    reason: Option<String>,
}

#[allow(dead_code)]
impl AppError {
    pub fn auth(message: impl Into<String>) -> Self {
        Self::of(ErrorKind::Auth(message.into()))
    }
    pub fn validation(message: impl Into<String>) -> Self {
        Self::of(ErrorKind::Validation(message.into()))
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::of(ErrorKind::NotFound(message.into()))
    }
    pub fn internal(err: impl Into<anyhow::Error>) -> Self {
        Self::of(ErrorKind::Internal(err.into()))
    }

    fn of(kind: ErrorKind) -> Self {
        Self {
            kind,
            instance: None,
            reason: None,
        }
    }

    /// Attaches the request target (`/api/...`) to the problem body.
    pub fn at(mut self, instance: Option<String>) -> Self {
        self.instance = instance;
        self
    }

    /// Attaches the `reason` extension member (e.g. `needs_sso`).
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn problem(&self) -> ProblemDetails {
        let status = self.status_code().as_u16();
        let detail = detail_for_status(status, self.kind.message());
        ProblemDetails::new(status, slug_for_status(status), title_for_status(status), detail)
            .with_instance(self.instance.clone())
            .with_reason(self.reason.clone())
    }
}

impl From<DbErr> for AppError {
    fn from(e: DbErr) -> Self {
        Self::of(ErrorKind::Db(e))
    }
}

impl From<AuthError> for AppError {
    fn from(e: AuthError) -> Self {
        Self::auth(e.0)
    }
}

impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self.kind {
            ErrorKind::Auth(_) => StatusCode::UNAUTHORIZED,
            ErrorKind::Validation(_) => StatusCode::BAD_REQUEST,
            ErrorKind::NotFound(_) => StatusCode::NOT_FOUND,
            ErrorKind::Db(_) | ErrorKind::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let status = self.status_code();
        let problem = self.problem();
        // Keep the TS logging split: >=500 at error (with cause), 4xx at warn.
        if status.as_u16() >= 500 {
            tracing::error!(target: "http.error", status = status.as_u16(), cause = %self.kind, "request failed");
        } else {
            tracing::warn!(target: "http.error", status = status.as_u16(), detail = %problem.detail, "request failed");
        }
        HttpResponse::build(status)
            .content_type("application/problem+json")
            .body(serde_json::to_string(&problem).unwrap_or_else(|_| "{}".to_string()))
    }
}
