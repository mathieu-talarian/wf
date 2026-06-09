//! Credential validation (port of `jira/validate.ts`): normalize the site URL,
//! then `GET /rest/api/3/myself`, mapping transport/HTTP failures to a typed
//! validation status (parallel to GitHub PAT validation).

use serde::Deserialize;
use thiserror::Error;

use crate::client::{JiraClient, JiraCreds};
use crate::errors::JiraApiError;
use crate::site_url::{normalize_site_url, SiteUrlResult};
use crate::status::{validation_status_for_http, JiraValidationStatus};

#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct JiraValidationError {
    pub status: JiraValidationStatus,
    pub message: String,
    pub http_status: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct JiraConnectInput {
    pub site_url: String,
    pub email: String,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct JiraValidated {
    pub origin: String,
    pub account_id: String,
    pub display_name: String,
    pub email_address: Option<String>,
}

#[derive(Deserialize)]
struct Myself {
    #[serde(rename = "accountId")]
    account_id: String,
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(rename = "emailAddress")]
    email_address: Option<String>,
}

fn validation_from_api(err: &JiraApiError) -> JiraValidationError {
    let http = if err.status == 0 { None } else { Some(err.status) };
    let status = validation_status_for_http(http);
    JiraValidationError { status, message: status.message().to_string(), http_status: http }
}

/// Normalizes the site URL to an origin, mapping a parse failure to an
/// `Invalid` validation error.
fn resolve_origin(site_url: &str) -> Result<String, JiraValidationError> {
    match normalize_site_url(site_url) {
        SiteUrlResult::Ok { origin } => Ok(origin),
        SiteUrlResult::Err { reason } => Err(JiraValidationError {
            status: JiraValidationStatus::Invalid,
            message: reason,
            http_status: None,
        }),
    }
}

/// Validate Jira Cloud credentials (port of `validateCredentials`).
pub async fn validate_credentials(
    input: &JiraConnectInput,
) -> Result<JiraValidated, JiraValidationError> {
    let origin = resolve_origin(&input.site_url)?;
    let client = JiraClient::new(&JiraCreds {
        site_url: origin.clone(),
        email: input.email.clone(),
        token: input.token.clone(),
    });
    match client.get::<Myself>("/rest/api/3/myself", &[]).await {
        Ok(me) => Ok(JiraValidated {
            origin,
            account_id: me.account_id,
            display_name: me.display_name,
            email_address: me.email_address,
        }),
        Err(e) => Err(validation_from_api(&e)),
    }
}
