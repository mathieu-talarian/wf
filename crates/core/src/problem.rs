//! RFC 9457 Problem Details.
//!
//! Port of `core/problem.ts` + the status/slug/title/detail mapping in
//! `core/http.ts`. Every error response is `application/problem+json`. `type` is
//! a stable URI under [`PROBLEM_TYPE_BASE`]; `reason` is an extension member
//! carrying the PAT/Jira validation status when relevant.

use serde::Serialize;

pub const PROBLEM_TYPE_BASE: &str = "https://docs.workflow.app/problems/";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    pub type_uri: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ProblemDetails {
    /// Builds a problem body from a status `slug`, exactly as `sendProblem` does:
    /// `type = PROBLEM_TYPE_BASE + slug`.
    pub fn new(
        status: u16,
        slug: &str,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            type_uri: format!("{PROBLEM_TYPE_BASE}{slug}"),
            title: title.into(),
            status,
            detail: detail.into(),
            instance: None,
            reason: None,
        }
    }

    pub fn with_instance(mut self, instance: Option<String>) -> Self {
        self.instance = instance;
        self
    }

    pub fn with_reason(mut self, reason: Option<String>) -> Self {
        self.reason = reason;
        self
    }
}

/// Canonical slug for a framework-level status (port of `problemSlug`).
/// Domain errors (e.g. 401 `unauthorized`) build their own slug directly.
pub fn slug_for_status(status: u16) -> &'static str {
    match status {
        400 => "bad-request",
        401 => "unauthorized",
        404 => "not-found",
        _ => "internal-error",
    }
}

/// Canonical title for a status (port of `problemTitle`).
pub fn title_for_status(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Internal Server Error",
    }
}

/// Detail string for a status: 5xx never leaks internals (port of
/// `problemDetail`).
pub fn detail_for_status(status: u16, message: &str) -> String {
    if status >= 500 {
        "An unexpected error occurred.".to_string()
    } else {
        message.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_with_type_field_and_omits_empty_extensions() {
        let p = ProblemDetails::new(404, "not-found", "Not Found", "nope");
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["type"], "https://docs.workflow.app/problems/not-found");
        assert_eq!(json["title"], "Not Found");
        assert_eq!(json["status"], 404);
        assert_eq!(json["detail"], "nope");
        assert!(json.get("instance").is_none());
        assert!(json.get("reason").is_none());
    }

    #[test]
    fn includes_instance_and_reason_when_present() {
        let p = ProblemDetails::new(403, "needs-sso", "Forbidden", "d")
            .with_instance(Some("/api/me/github".to_string()))
            .with_reason(Some("needs_sso".to_string()));
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["instance"], "/api/me/github");
        assert_eq!(json["reason"], "needs_sso");
    }

    #[test]
    fn five_hundred_detail_is_generic() {
        assert_eq!(detail_for_status(500, "db exploded"), "An unexpected error occurred.");
        assert_eq!(detail_for_status(400, "bad field"), "bad field");
    }
}
