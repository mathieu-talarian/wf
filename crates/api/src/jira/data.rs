//! Jira read orchestration (port of `jira/pat/runners.ts` data runners +
//! `action-runners.ts` reads). Each call loads the stored row, decrypts the
//! token, builds a client, and delegates to `wf-jira`. The dashboard degrades to
//! a disconnected payload when no connection exists; the rest require one.

use sea_orm::prelude::Uuid;
use wf_core::Sealed;
use wf_db::tables::{jira_issues, jira_pat_connections as jira};
use wf_jira::{
    create_meta as jira_create_meta, edit_meta as jira_edit_meta, fetch_issue_detail,
    fetch_issue_page, fetch_queue_page, list_boards, list_issue_types,
    list_projects, list_transitions, search_users, sprint_issues, AssignableQuery, JiraAccountSummary,
    JiraBoard, JiraClient, JiraCreateMeta, JiraCreds, JiraDashboard, JiraEditMeta, JiraIssueDetail,
    JiraIssuePage, JiraIssueSummary, JiraIssueType, JiraNamedIcon, JiraNotConnected, JiraProject,
    JiraQueueKey, JiraQueueResult, JiraStatus, JiraTransition, JiraUser, QueueJqlCtx, QUEUE_KEYS,
    SUMMARY_FIELDS,
};

use crate::error::AppError;
use crate::github::summary::json_string_array;
use crate::state::AppState;

struct Connected {
    row: jira::Model,
    client: JiraClient,
}

fn client_for(state: &AppState, row: &jira::Model) -> Result<JiraClient, AppError> {
    let token = state
        .cipher
        .open(&Sealed {
            ciphertext: row.api_token_ciphertext.clone(),
            iv: row.api_token_iv.clone(),
            auth_tag: row.api_token_auth_tag.clone(),
        })
        .map_err(|e| AppError::internal(anyhow::anyhow!(e)))?;
    Ok(JiraClient::new(&JiraCreds {
        site_url: row.site_url.clone(),
        email: row.email.clone(),
        token,
    }))
}

fn ctx_of(row: &jira::Model) -> QueueJqlCtx {
    QueueJqlCtx {
        account_id: row.account_id.clone(),
        selected_projects: json_string_array(&row.selected_projects),
    }
}

async fn load_connected(state: &AppState, user_id: Uuid) -> Result<Connected, AppError> {
    let row = jira::select_row(&state.db, user_id)
        .await?
        .ok_or_else(|| AppError::from(JiraNotConnected("No Jira connection".to_string())))?;
    let client = client_for(state, &row)?;
    Ok(Connected { row, client })
}

/// Connection's client only (port of `loadConnected` for the write paths).
pub(crate) async fn connected_client(
    state: &AppState,
    user_id: Uuid,
) -> Result<JiraClient, AppError> {
    Ok(load_connected(state, user_id).await?.client)
}

/// The `connected: false` dashboard payload returned when no Jira row exists.
fn disconnected_dashboard() -> JiraDashboard {
    JiraDashboard {
        account: JiraAccountSummary {
            connected: false,
            site_url: None,
            account_id: None,
            display_name: None,
        },
        queues: vec![],
        selected_projects: vec![],
    }
}

/// `GET /me/jira/dashboard` (port of `runDashboard`): disconnected payload when
/// no row, else every queue with a best-effort `touch_last_used`.
pub async fn dashboard(state: &AppState, user_id: Uuid) -> Result<JiraDashboard, AppError> {
    let Some(row) = jira::select_row(&state.db, user_id).await? else {
        return Ok(disconnected_dashboard());
    };
    let ctx = ctx_of(&row);
    let rows = jira_issues::list_recent(&state.db, user_id, &ctx.selected_projects, 500).await?;
    let issues: Vec<JiraIssueSummary> = rows.into_iter().map(issue_summary).collect();
    let queues = QUEUE_KEYS
        .into_iter()
        .map(|key| projected_queue(key, &issues, &row.display_name))
        .collect();
    spawn_touch_last_used(state, user_id);
    Ok(JiraDashboard {
        account: JiraAccountSummary {
            connected: true,
            site_url: Some(row.site_url),
            account_id: Some(row.account_id),
            display_name: Some(row.display_name),
        },
        queues,
        selected_projects: ctx.selected_projects,
    })
}

fn spawn_touch_last_used(state: &AppState, user_id: Uuid) {
    let db = state.db.clone();
    tokio::spawn(async move {
        let _ = jira::touch_last_used(&db, user_id).await;
    });
}

fn projected_queue(
    key: JiraQueueKey,
    issues: &[JiraIssueSummary],
    display_name: &str,
) -> JiraQueueResult {
    let selected: Vec<_> = issues
        .iter()
        .filter(|issue| queue_match(issue, key, display_name))
        .take(50)
        .cloned()
        .collect();
    JiraQueueResult {
        key,
        approximate_total: Some(selected.len() as i64),
        issues: selected,
        next_cursor: None,
        is_last: true,
        error: None,
    }
}

fn queue_match(issue: &JiraIssueSummary, key: JiraQueueKey, display_name: &str) -> bool {
    match key {
        JiraQueueKey::Assigned => issue.assignee.as_ref().is_some_and(|u| u.display_name == display_name),
        JiraQueueKey::ActiveSprint => issue.status.category != "done",
        JiraQueueKey::PreviouslyMine | JiraQueueKey::Reported | JiraQueueKey::Watching => false,
    }
}

fn issue_summary(row: jira_issues::Model) -> JiraIssueSummary {
    JiraIssueSummary {
        key: row.issue_key,
        summary: row.summary,
        status: JiraStatus { name: row.status_name, category: row.status_category },
        issue_type: JiraNamedIcon { name: row.issue_type_name.unwrap_or_default(), icon_url: None },
        priority: row.priority_name.map(|name| JiraNamedIcon { name, icon_url: None }),
        assignee: row.assignee_name.map(projected_user),
        project_key: row.project,
        updated: row.updated_at.to_rfc3339(),
        url: row.url,
    }
}

fn projected_user(display_name: String) -> JiraUser {
    JiraUser { account_id: String::new(), display_name, avatar_url: None, email_address: None }
}

pub async fn queue(
    state: &AppState,
    user_id: Uuid,
    key: JiraQueueKey,
    cursor: Option<&str>,
) -> Result<JiraIssuePage, AppError> {
    let c = load_connected(state, user_id).await?;
    Ok(fetch_queue_page(&c.client, key, &ctx_of(&c.row), cursor).await?)
}

pub async fn search(
    state: &AppState,
    user_id: Uuid,
    jql: &str,
    cursor: Option<&str>,
) -> Result<JiraIssuePage, AppError> {
    let c = load_connected(state, user_id).await?;
    Ok(fetch_issue_page(&c.client, jql, SUMMARY_FIELDS, cursor).await?)
}

pub async fn issue(state: &AppState, user_id: Uuid, key: &str) -> Result<JiraIssueDetail, AppError> {
    let c = load_connected(state, user_id).await?;
    Ok(fetch_issue_detail(&c.client, key).await?)
}

pub async fn projects(state: &AppState, user_id: Uuid) -> Result<Vec<JiraProject>, AppError> {
    let c = load_connected(state, user_id).await?;
    Ok(list_projects(&c.client).await?)
}

pub async fn issue_types(
    state: &AppState,
    user_id: Uuid,
    project_key: &str,
) -> Result<Vec<JiraIssueType>, AppError> {
    let c = load_connected(state, user_id).await?;
    Ok(list_issue_types(&c.client, project_key).await?)
}

pub async fn boards(state: &AppState, user_id: Uuid) -> Result<Vec<JiraBoard>, AppError> {
    let c = load_connected(state, user_id).await?;
    Ok(list_boards(&c.client).await?)
}

pub async fn sprint(
    state: &AppState,
    user_id: Uuid,
    board_id: i64,
) -> Result<JiraIssuePage, AppError> {
    let c = load_connected(state, user_id).await?;
    Ok(sprint_issues(&c.client, board_id).await?)
}

pub async fn transitions(
    state: &AppState,
    user_id: Uuid,
    key: &str,
) -> Result<Vec<JiraTransition>, AppError> {
    let c = load_connected(state, user_id).await?;
    Ok(list_transitions(&c.client, key).await?)
}

pub async fn users(
    state: &AppState,
    user_id: Uuid,
    input: &AssignableQuery,
) -> Result<Vec<JiraUser>, AppError> {
    let c = load_connected(state, user_id).await?;
    Ok(search_users(&c.client, input).await?)
}

pub async fn create_meta(
    state: &AppState,
    user_id: Uuid,
    project_key: &str,
    issue_type_id: &str,
) -> Result<JiraCreateMeta, AppError> {
    let c = load_connected(state, user_id).await?;
    Ok(jira_create_meta(&c.client, project_key, issue_type_id).await?)
}

pub async fn edit_meta(state: &AppState, user_id: Uuid, key: &str) -> Result<JiraEditMeta, AppError> {
    let c = load_connected(state, user_id).await?;
    Ok(jira_edit_meta(&c.client, key).await?)
}
