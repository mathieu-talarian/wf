//! GitHub PAT value types (port of the type unions in `pat/validate.ts`).

use serde::Serialize;

/// How a token was issued, detected by prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatTokenKind {
    ClassicPat,
    FineGrainedPat,
    Unknown,
}

impl PatTokenKind {
    /// `github_pat_` → fine-grained, `ghp_` → classic, else unknown.
    pub fn detect(token: &str) -> Self {
        if token.starts_with("github_pat_") {
            PatTokenKind::FineGrainedPat
        } else if token.starts_with("ghp_") {
            PatTokenKind::ClassicPat
        } else {
            PatTokenKind::Unknown
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PatTokenKind::ClassicPat => "classic_pat",
            PatTokenKind::FineGrainedPat => "fine_grained_pat",
            PatTokenKind::Unknown => "unknown",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "classic_pat" => PatTokenKind::ClassicPat,
            "fine_grained_pat" => PatTokenKind::FineGrainedPat,
            _ => PatTokenKind::Unknown,
        }
    }
}

/// Outcome of validating a token against GitHub. The web app keys connection UI
/// off these values, so the mapping must match `pat/validate.ts` exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatValidationStatus {
    Valid,
    Invalid,
    NeedsSso,
    NeedsOrgApproval,
    MissingPermissions,
    Unknown,
}

impl PatValidationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PatValidationStatus::Valid => "valid",
            PatValidationStatus::Invalid => "invalid",
            PatValidationStatus::NeedsSso => "needs_sso",
            PatValidationStatus::NeedsOrgApproval => "needs_org_approval",
            PatValidationStatus::MissingPermissions => "missing_permissions",
            PatValidationStatus::Unknown => "unknown",
        }
    }

    /// User-facing message for each status (port of the `MESSAGES` map).
    pub fn message(self) -> &'static str {
        match self {
            PatValidationStatus::Valid => "Token is valid.",
            PatValidationStatus::Invalid => {
                "Token is invalid or was revoked. Create a new token and try again."
            }
            PatValidationStatus::NeedsSso => {
                "Authorize this token for your organization's SAML SSO in GitHub, then retry."
            }
            PatValidationStatus::NeedsOrgApproval => {
                "An organization owner must approve this fine-grained token before it works."
            }
            PatValidationStatus::MissingPermissions => {
                "Token is missing a required scope, or the organization blocked it."
            }
            PatValidationStatus::Unknown => {
                "Could not validate the token. Check the token and try again."
            }
        }
    }
}

/// Identity + metadata read from a valid token.
#[derive(Debug, Clone)]
pub struct PatValidationResult {
    pub github_user_id: i64,
    pub login: String,
    pub token_kind: PatTokenKind,
    pub scopes: Option<Vec<String>>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}
