//! GitHub connection summary DTO (port of `pat/summary.ts`). Built from the
//! `github_pat_connections` row; the empty form is returned when not connected.

use chrono::SecondsFormat;
use sea_orm::prelude::DateTimeWithTimeZone;
use serde::Serialize;
use serde_json::Value;
use wf_db::entities::github_pat_connections as gh;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GithubConnectionSummary {
    connected: bool,
    login: Option<String>,
    token_kind: Option<String>,
    scopes: Option<Vec<String>>,
    selected_repos: Vec<String>,
    validation_status: Option<String>,
    validation_error: Option<String>,
    last_validated_at: Option<String>,
    last_used_at: Option<String>,
    expires_at: Option<String>,
    last_four: Option<String>,
    connected_at: Option<String>,
}

fn iso(dt: Option<DateTimeWithTimeZone>) -> Option<String> {
    dt.map(|d| d.with_timezone(&chrono::Utc).to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn split_scopes(scope: Option<&str>) -> Option<Vec<String>> {
    let scope = scope?;
    if scope.is_empty() {
        return None;
    }
    Some(
        scope
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// jsonb string array → `Vec<String>` (null/non-array → empty).
pub fn json_string_array(v: &Option<Value>) -> Vec<String> {
    match v {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

pub fn from_row(row: Option<gh::Model>) -> GithubConnectionSummary {
    match row {
        None => GithubConnectionSummary {
            connected: false,
            ..Default::default()
        },
        Some(row) => GithubConnectionSummary {
            connected: true,
            login: Some(row.github_login),
            token_kind: Some(row.token_kind),
            scopes: split_scopes(row.scope.as_deref()),
            selected_repos: json_string_array(&row.selected_repos),
            validation_status: Some(row.validation_status),
            validation_error: row.validation_error,
            last_validated_at: iso(row.last_validated_at),
            last_used_at: iso(row.last_used_at),
            expires_at: iso(row.expires_at),
            last_four: row.last_four,
            connected_at: Some(
                row.created_at
                    .with_timezone(&chrono::Utc)
                    .to_rfc3339_opts(SecondsFormat::Millis, true),
            ),
        },
    }
}
