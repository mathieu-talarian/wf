//! Jira Cloud integration: Basic-auth reqwest client, site-URL normalization,
//! credential validation, connection flow, data mappers, and actions. Migration
//! plan §10.2.

pub mod client;
pub mod errors;
pub mod issues;
pub mod site_url;
pub mod status;
pub mod types;
pub mod validate;

pub use client::{JiraClient, JiraCreds};
pub use errors::{JiraActionError, JiraApiError, JiraNotConnected, JiraWriteError};
pub use site_url::{is_same_jira_origin, normalize_site_url, SiteUrlResult};
pub use status::{classify_queue_failure, validation_status_for_http, JiraValidationStatus, QueueErrorKind};
pub use validate::{validate_credentials, JiraConnectInput, JiraValidated, JiraValidationError};

pub use issues::adf::{adf_to_text, text_to_adf};
pub use issues::dashboard::{fetch_dashboard_queues, fetch_queue_page, QUEUE_KEYS};
pub use issues::fields::{
    build_issue_fields, normalize_create_meta_fields, BuildFieldsOpts, FieldMetaMap,
    JiraAllowedValue, JiraFieldMeta, JiraFieldSchema,
};
pub use issues::search::{approximate_count, fetch_issue_detail, fetch_issue_page};
pub use issues::jql::{build_queue_jql, quote_jql_string, JiraQueueKey, QueueJqlCtx};
pub use issues::reads::{
    create_meta, edit_meta, list_boards, list_issue_types, list_projects, list_transitions,
    search_users, sprint_issues, AssignableQuery,
};
pub use issues::writes::{
    add_comment, assign_issue, create_issue, edit_issue, log_work, transition_issue,
};
pub use types::*;
