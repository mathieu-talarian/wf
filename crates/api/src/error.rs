//! `AppError` — the HTTP-boundary error aggregate.
//!
//! Replaces Effect's `Data.TaggedError` channel. Each variant maps to a status
//! and an RFC 9457 Problem body via actix's `ResponseError` (migration plan
//! §3.3, §9). Domain crates return their own error enums; this aggregate gains
//! `From` impls for them as the integration phases land.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, ResponseError};
use wf_core::problem::{detail_for_status, slug_for_status, title_for_status, ProblemDetails};

// Phase 1 has no handlers that fail with `AppError`; the variants and helpers
// are consumed from Phase 2 onward (auth, db, integrations).
#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Authentication / authorization failure → 401 `unauthorized`.
    #[error("unauthorized: {0}")]
    Auth(String),
    /// Request validation / parse failure → 400 `bad-request`.
    #[error("validation: {0}")]
    Validation(String),
    /// Resource not found → 404 `not-found`.
    #[error("not found: {0}")]
    NotFound(String),
    /// Anything unexpected → 500 `internal-error` (detail never leaks).
    #[error("internal error: {0}")]
    Internal(anyhow::Error),
}

#[allow(dead_code)]
impl AppError {
    pub fn internal(err: impl Into<anyhow::Error>) -> Self {
        AppError::Internal(err.into())
    }

    /// Builds the RFC 9457 body for this error. `instance` is the request
    /// target (`/api/...`), supplied by the caller when available.
    pub fn problem(&self, instance: Option<String>) -> ProblemDetails {
        let status = self.status_code().as_u16();
        let slug = slug_for_status(status);
        let detail = detail_for_status(status, &self.to_string());
        ProblemDetails::new(status, slug, title_for_status(status), detail).with_instance(instance)
    }
}

impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::Auth(_) => StatusCode::UNAUTHORIZED,
            AppError::Validation(_) => StatusCode::BAD_REQUEST,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let status = self.status_code();
        let problem = self.problem(None);
        // Keep today's logging split: >=500 at error (with cause), 4xx at warn.
        if status.as_u16() >= 500 {
            tracing::error!(target: "http.error", status = status.as_u16(), cause = %self, "request failed");
        } else {
            tracing::warn!(target: "http.error", status = status.as_u16(), detail = %problem.detail, "request failed");
        }
        HttpResponse::build(status)
            .content_type("application/problem+json")
            .body(serde_json::to_string(&problem).unwrap_or_else(|_| "{}".to_string()))
    }
}
