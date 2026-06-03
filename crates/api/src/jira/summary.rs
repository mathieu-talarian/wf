//! Jira connection summary DTO (port of `jira/pat/account.ts` projection).
//! Built from the `jira_pat_connections` row; the empty form is returned when
//! not connected.

use chrono::SecondsFormat;
use sea_orm::prelude::DateTimeWithTimeZone;
use serde::Serialize;
use wf_db::entities::jira_pat_connections as jira;

use crate::github::summary::json_string_array;

#[derive(Serialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraConnectionSummary {
    connected: bool,
    site_url: Option<String>,
    account_id: Option<String>,
    display_name: Option<String>,
    email: Option<String>,
    selected_projects: Vec<String>,
    validation_status: Option<String>,
    validation_error: Option<String>,
    last_validated_at: Option<String>,
    last_used_at: Option<String>,
    last_four: Option<String>,
    connected_at: Option<String>,
}

fn iso(dt: Option<DateTimeWithTimeZone>) -> Option<String> {
    dt.map(|d| d.with_timezone(&chrono::Utc).to_rfc3339_opts(SecondsFormat::Millis, true))
}

pub fn from_row(row: Option<jira::Model>) -> JiraConnectionSummary {
    match row {
        None => JiraConnectionSummary { connected: false, ..Default::default() },
        Some(row) => JiraConnectionSummary {
            connected: true,
            site_url: Some(row.site_url),
            account_id: Some(row.account_id),
            display_name: Some(row.display_name),
            email: Some(row.email),
            selected_projects: json_string_array(&row.selected_projects),
            validation_status: Some(row.validation_status),
            validation_error: row.validation_error,
            last_validated_at: iso(row.last_validated_at),
            last_used_at: iso(row.last_used_at),
            last_four: row.last_four,
            connected_at: Some(
                row.created_at
                    .with_timezone(&chrono::Utc)
                    .to_rfc3339_opts(SecondsFormat::Millis, true),
            ),
        },
    }
}
