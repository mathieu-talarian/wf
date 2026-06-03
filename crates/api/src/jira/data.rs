//! Jira read orchestration (port of `jira/pat/runners.ts` data runners +
//! `action-runners.ts` reads). Each call loads the stored row, decrypts the
//! token, builds a client, and delegates to `wf-jira`. The dashboard degrades to
//! a disconnected payload when no connection exists; the rest require one.

use sea_orm::prelude::Uuid;
use wf_core::Sealed;
use wf_db::entities::jira_pat_connections as jira;
use wf_db::repositories::jira_pat;
use wf_jira::{
    create_meta as jira_create_meta, edit_meta as jira_edit_meta, fetch_dashboard_queues,
    fetch_issue_detail, fetch_issue_page, fetch_queue_page, list_boards, list_issue_types,
    list_projects, list_transitions, search_users, sprint_issues, AssignableQuery, JiraAccountSummary,
    JiraBoard, JiraClient, JiraCreateMeta, JiraCreds, JiraDashboard, JiraEditMeta, JiraIssueDetail,
    JiraIssuePage, JiraIssueType, JiraNotConnected, JiraProject, JiraQueueKey, JiraTransition,
    JiraUser, QueueJqlCtx, SUMMARY_FIELDS,
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
    let row = jira_pat::select_row(&state.db, user_id)
        .await?
        .ok_or_else(|| AppError::from(JiraNotConnected("No Jira connection".to_string())))?;
    let client = client_for(state, &row)?;
    Ok(Connected { row, client })
}

/// `GET /me/jira/dashboard` (port of `runDashboard`): disconnected payload when
/// no row, else every queue with a best-effort `touch_last_used`.
pub async fn dashboard(state: &AppState, user_id: Uuid) -> Result<JiraDashboard, AppError> {
    let Some(row) = jira_pat::select_row(&state.db, user_id).await? else {
        return Ok(JiraDashboard {
            account: JiraAccountSummary {
                connected: false,
                site_url: None,
                account_id: None,
                display_name: None,
            },
            queues: vec![],
            selected_projects: vec![],
        });
    };
    let client = client_for(state, &row)?;
    let ctx = ctx_of(&row);
    let queues = fetch_dashboard_queues(&client, &ctx).await;
    let _ = jira_pat::touch_last_used(&state.db, user_id).await;
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
