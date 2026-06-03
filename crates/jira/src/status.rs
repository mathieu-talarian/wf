//! HTTP-status → validation-status mapping (port of `jira/issues/status.ts`,
//! validation half; `classifyQueueFailure` lands with the dashboard in 4c).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JiraValidationStatus {
    Valid,
    Invalid,
    MissingPermissions,
    Unknown,
}

impl JiraValidationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::MissingPermissions => "missing_permissions",
            Self::Unknown => "unknown",
        }
    }

    /// User-facing message per status (port of `validate.ts#MESSAGES`).
    pub fn message(self) -> &'static str {
        match self {
            Self::Valid => "Credentials are valid.",
            Self::Invalid => "Email or API token was rejected. Create a new token and retry.",
            Self::MissingPermissions => "The token lacks permission to read your Jira profile.",
            Self::Unknown => "Could not reach Jira to validate the credentials.",
        }
    }
}

/// Port of `validationStatusForHttp`: 401 → invalid, 403 → missing_permissions,
/// anything else (incl. network/`None`) → unknown.
pub fn validation_status_for_http(status: Option<u16>) -> JiraValidationStatus {
    match status {
        Some(401) => JiraValidationStatus::Invalid,
        Some(403) => JiraValidationStatus::MissingPermissions,
        _ => JiraValidationStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_http_statuses() {
        assert_eq!(validation_status_for_http(Some(401)), JiraValidationStatus::Invalid);
        assert_eq!(validation_status_for_http(Some(403)), JiraValidationStatus::MissingPermissions);
        assert_eq!(validation_status_for_http(Some(404)), JiraValidationStatus::Unknown);
        assert_eq!(validation_status_for_http(Some(500)), JiraValidationStatus::Unknown);
        assert_eq!(validation_status_for_http(None), JiraValidationStatus::Unknown);
    }
}
