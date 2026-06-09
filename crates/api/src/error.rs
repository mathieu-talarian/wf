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
use wf_jira::{
    JiraActionError, JiraApiError, JiraNotConnected, JiraValidationError, JiraValidationStatus,
    JiraWriteError,
};

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
    #[error(transparent)]
    JiraValidation(JiraValidationError),
    #[error(transparent)]
    JiraApi(JiraApiError),
    #[error(transparent)]
    JiraWrite(JiraWriteError),
    #[error(transparent)]
    JiraNotConnected(JiraNotConnected),
    #[error("internal error: {0}")]
    Internal(anyhow::Error),
}

/// Port of the Jira `validationStatus`: HTTP status if known, else by reason
/// (invalid → 401, missing_permissions → 403, else 502).
fn jira_validation_http_status(e: &JiraValidationError) -> u16 {
    if let Some(s) = e.http_status {
        return s;
    }
    match e.status {
        JiraValidationStatus::Invalid => 401,
        JiraValidationStatus::MissingPermissions => 403,
        _ => 502,
    }
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

/// Convenience constructor for a Problem with no `reason` extension member.
fn simple(status: u16, slug: &'static str, title: &'static str, detail: String) -> Parts {
    Parts { status, slug, title, detail, reason: None }
}

fn github_validation_parts(e: &PatValidationError) -> Parts {
    Parts {
        status: validation_http_status(e),
        slug: "github-token-rejected",
        title: "GitHub token rejected",
        detail: e.message.clone(),
        reason: Some(e.status.as_str().to_string()),
    }
}

fn github_parts(e: &GithubError) -> Parts {
    match e {
        GithubError::NotConnected => simple(
            404,
            "github-not-connected",
            "GitHub not connected",
            "No GitHub token is connected for this user.".to_string(),
        ),
        GithubError::Api(_) => simple(
            502,
            "github-request-failed",
            "GitHub request failed",
            "The upstream GitHub request did not succeed.".to_string(),
        ),
        GithubError::Write { status, message } => {
            simple(*status, "github-write-failed", "GitHub action failed", message.clone())
        }
    }
}

fn jira_validation_parts(e: &JiraValidationError) -> Parts {
    Parts {
        status: jira_validation_http_status(e),
        slug: "jira-token-rejected",
        title: "Jira credentials rejected",
        detail: e.message.clone(),
        reason: Some(e.status.as_str().to_string()),
    }
}

fn jira_write_parts(e: &JiraWriteError) -> Parts {
    let status = if (400..600).contains(&e.status) { e.status } else { 502 };
    simple(status, "jira-write-failed", "Jira action failed", e.message.clone())
}

/// Generic 500 for `Db`/`Internal` — details are deliberately opaque to clients.
fn internal_parts() -> Parts {
    simple(
        500,
        "internal-error",
        "Internal Server Error",
        "An unexpected error occurred.".to_string(),
    )
}

fn jira_not_connected_parts() -> Parts {
    simple(
        404,
        "jira-not-connected",
        "Jira not connected",
        "No Jira connection exists for this user.".to_string(),
    )
}

fn jira_api_parts() -> Parts {
    simple(
        502,
        "jira-request-failed",
        "Jira request failed",
        "The upstream Jira request did not succeed.".to_string(),
    )
}

impl ErrorKind {
    fn parts(&self) -> Parts {
        match self {
            ErrorKind::Auth(m) => simple(401, "unauthorized", "Unauthorized", m.clone()),
            ErrorKind::Validation(m) => simple(400, "bad-request", "Bad Request", m.clone()),
            ErrorKind::NotFound(m) => simple(404, "not-found", "Not Found", m.clone()),
            ErrorKind::Db(_) | ErrorKind::Internal(_) => internal_parts(),
            ErrorKind::GithubValidation(e) => github_validation_parts(e),
            ErrorKind::Github(e) => github_parts(e),
            ErrorKind::JiraValidation(e) => jira_validation_parts(e),
            ErrorKind::JiraNotConnected(_) => jira_not_connected_parts(),
            ErrorKind::JiraWrite(e) => jira_write_parts(e),
            ErrorKind::JiraApi(_) => jira_api_parts(),
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

impl From<JiraValidationError> for AppError {
    fn from(e: JiraValidationError) -> Self {
        Self::of(ErrorKind::JiraValidation(e))
    }
}

impl From<JiraApiError> for AppError {
    fn from(e: JiraApiError) -> Self {
        Self::of(ErrorKind::JiraApi(e))
    }
}

impl From<JiraWriteError> for AppError {
    fn from(e: JiraWriteError) -> Self {
        Self::of(ErrorKind::JiraWrite(e))
    }
}

impl From<JiraNotConnected> for AppError {
    fn from(e: JiraNotConnected) -> Self {
        Self::of(ErrorKind::JiraNotConnected(e))
    }
}

impl From<JiraActionError> for AppError {
    fn from(e: JiraActionError) -> Self {
        match e {
            JiraActionError::Api(e) => Self::from(e),
            JiraActionError::Write(e) => Self::from(e),
        }
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
