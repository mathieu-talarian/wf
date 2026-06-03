//! HTTP-status → validation-status mapping (port of `jira/issues/status.ts`,
//! validation half; `classifyQueueFailure` lands with the dashboard in 4c).

use serde::{Deserialize, Serialize};

use crate::issues::jql::JiraQueueKey;

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

/// Whether a failing dashboard queue should degrade quietly (Agile/sprint
/// features absent on a non-Software site) or surface as a real error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueErrorKind {
    FeatureUnavailable,
    Error,
}

const AGILE_ABSENT: [u16; 3] = [400, 403, 404];

/// Port of `classifyQueueFailure`: an Agile-only `active_sprint` queue that fails
/// with 400/403/404 is treated as "feature unavailable" (non-Software site);
/// everything else is a real error.
pub fn classify_queue_failure(key: JiraQueueKey, status: Option<u16>) -> QueueErrorKind {
    match status {
        Some(s) if key == JiraQueueKey::ActiveSprint && AGILE_ABSENT.contains(&s) => {
            QueueErrorKind::FeatureUnavailable
        }
        _ => QueueErrorKind::Error,
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

    #[test]
    fn classify_degrades_active_sprint_on_agile_absent() {
        for s in [400, 403, 404] {
            assert_eq!(
                classify_queue_failure(JiraQueueKey::ActiveSprint, Some(s)),
                QueueErrorKind::FeatureUnavailable
            );
        }
    }

    #[test]
    fn classify_treats_5xx_active_sprint_as_real_error() {
        assert_eq!(
            classify_queue_failure(JiraQueueKey::ActiveSprint, Some(500)),
            QueueErrorKind::Error
        );
    }

    #[test]
    fn classify_treats_403_on_core_queue_as_real_error() {
        assert_eq!(classify_queue_failure(JiraQueueKey::Assigned, Some(403)), QueueErrorKind::Error);
    }
}
