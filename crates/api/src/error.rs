//! `AppError` — the HTTP-boundary error aggregate.
//!
//! Replaces Effect's `Data.TaggedError` channel. Each kind maps to a status and
//! an RFC 9457 Problem body via actix's `ResponseError` (migration plan §3.3,
//! §9). It additionally carries the request `instance` (and an optional
//! override `reason`), since `ResponseError::error_response` has no access to
//! the request. GitHub error kinds reproduce `github/routes/helpers.ts`.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, ResponseError};
use sea_orm::DbErr;
use wf_core::auth::AuthError;
use wf_core::problem::ProblemDetails;
use wf_github::errors::{GithubError, PatValidationError};
use wf_github::PatValidationStatus;

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
    #[error(transparent)]
    GithubValidation(PatValidationError),
    #[error(transparent)]
    Github(GithubError),
    #[error("internal error: {0}")]
    Internal(anyhow::Error),
}

/// The pieces of an RFC 9457 body for a given error kind:
/// `(status, slug, title, detail, reason)`.
struct Parts {
    status: u16,
    slug: &'static str,
    title: &'static str,
    detail: String,
    reason: Option<String>,
}

/// Port of `validationStatus`: HTTP status if known, else by reason
/// (invalid → 401, unknown → 502, else 403).
fn validation_http_status(e: &PatValidationError) -> u16 {
    if let Some(s) = e.http_status {
        return s;
    }
    match e.status {
        PatValidationStatus::Invalid => 401,
        PatValidationStatus::Unknown => 502,
        _ => 403,
    }
}

impl ErrorKind {
    fn parts(&self) -> Parts {
        match self {
            ErrorKind::Auth(m) => Parts {
                status: 401,
                slug: "unauthorized",
                title: "Unauthorized",
                detail: m.clone(),
                reason: None,
            },
            ErrorKind::Validation(m) => Parts {
                status: 400,
                slug: "bad-request",
                title: "Bad Request",
                detail: m.clone(),
                reason: None,
            },
            ErrorKind::NotFound(m) => Parts {
                status: 404,
                slug: "not-found",
                title: "Not Found",
                detail: m.clone(),
                reason: None,
            },
            ErrorKind::Db(_) | ErrorKind::Internal(_) => Parts {
                status: 500,
                slug: "internal-error",
                title: "Internal Server Error",
                detail: "An unexpected error occurred.".to_string(),
                reason: None,
            },
            ErrorKind::GithubValidation(e) => Parts {
                status: validation_http_status(e),
                slug: "github-token-rejected",
                title: "GitHub token rejected",
                detail: e.message.clone(),
                reason: Some(e.status.as_str().to_string()),
            },
            ErrorKind::Github(GithubError::NotConnected) => Parts {
                status: 404,
                slug: "github-not-connected",
                title: "GitHub not connected",
                detail: "No GitHub token is connected for this user.".to_string(),
                reason: None,
            },
            ErrorKind::Github(GithubError::Api(_)) => Parts {
                status: 502,
                slug: "github-request-failed",
                title: "GitHub request failed",
                detail: "The upstream GitHub request did not succeed.".to_string(),
                reason: None,
            },
            ErrorKind::Github(GithubError::Write { status, message }) => Parts {
                status: *status,
                slug: "github-write-failed",
                title: "GitHub action failed",
                detail: message.clone(),
                reason: None,
            },
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

    /// Overrides the `reason` extension member.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn problem(&self) -> ProblemDetails {
        let parts = self.kind.parts();
        ProblemDetails::new(parts.status, parts.slug, parts.title, parts.detail)
            .with_instance(self.instance.clone())
            .with_reason(self.reason.clone().or(parts.reason))
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

impl From<PatValidationError> for AppError {
    fn from(e: PatValidationError) -> Self {
        Self::of(ErrorKind::GithubValidation(e))
    }
}

impl From<GithubError> for AppError {
    fn from(e: GithubError) -> Self {
        Self::of(ErrorKind::Github(e))
    }
}

impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        StatusCode::from_u16(self.kind.parts().status)
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }

    fn error_response(&self) -> HttpResponse {
        let problem = self.problem();
        // Keep the TS logging split: >=500 at error (with cause), 4xx at warn.
        if problem.status >= 500 {
            tracing::error!(target: "http.error", status = problem.status, cause = %self.kind, "request failed");
        } else {
            tracing::warn!(target: "http.error", status = problem.status, detail = %problem.detail, "request failed");
        }
        HttpResponse::build(self.status_code())
            .content_type("application/problem+json")
            .body(serde_json::to_string(&problem).unwrap_or_else(|_| "{}".to_string()))
    }
}
