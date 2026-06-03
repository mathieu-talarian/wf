//! Jira Cloud integration: Basic-auth reqwest client, site-URL normalization,
//! credential validation, and (Phase 4 cont.) connection flow, data mappers, and
//! actions. Migration plan §10.2.

pub mod client;
pub mod errors;
pub mod site_url;
pub mod status;
pub mod validate;

pub use client::{JiraClient, JiraCreds};
pub use errors::{JiraApiError, JiraNotConnected, JiraWriteError};
pub use site_url::{is_same_jira_origin, normalize_site_url, SiteUrlResult};
pub use status::{validation_status_for_http, JiraValidationStatus};
pub use validate::{validate_credentials, JiraConnectInput, JiraValidated, JiraValidationError};
