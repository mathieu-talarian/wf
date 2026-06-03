//! Jira write orchestration (port of `write-runners.ts`). Each call resolves the
//! connection's client and delegates to `wf-jira`, which enforces the create/edit
//! metadata allowlist. `JiraActionError` carries the upstream status through.

use sea_orm::prelude::Uuid;
use serde_json::{Map, Value};
use wf_jira::{
    add_comment, assign_issue, create_issue, edit_issue, log_work, transition_issue, JiraComment,
    JiraCreateIssueInput, JiraCreateIssueResult, JiraWorklogInput,
};

use crate::error::AppError;
use crate::jira::data;
use crate::state::AppState;

pub async fn transition(
    state: &AppState,
    user_id: Uuid,
    key: &str,
    transition_id: &str,
) -> Result<(), AppError> {
    let client = data::connected_client(state, user_id).await?;
    Ok(transition_issue(&client, key, transition_id).await?)
}

pub async fn comment(
    state: &AppState,
    user_id: Uuid,
    key: &str,
    body: &str,
) -> Result<JiraComment, AppError> {
    let client = data::connected_client(state, user_id).await?;
    Ok(add_comment(&client, key, body).await?)
}

pub async fn assign(
    state: &AppState,
    user_id: Uuid,
    key: &str,
    account_id: Option<&str>,
) -> Result<(), AppError> {
    let client = data::connected_client(state, user_id).await?;
    Ok(assign_issue(&client, key, account_id).await?)
}

pub async fn worklog(
    state: &AppState,
    user_id: Uuid,
    key: &str,
    input: &JiraWorklogInput,
) -> Result<(), AppError> {
    let client = data::connected_client(state, user_id).await?;
    Ok(log_work(&client, key, input).await?)
}

pub async fn create(
    state: &AppState,
    user_id: Uuid,
    input: &JiraCreateIssueInput,
) -> Result<JiraCreateIssueResult, AppError> {
    let client = data::connected_client(state, user_id).await?;
    Ok(create_issue(&client, input).await?)
}

pub async fn edit(
    state: &AppState,
    user_id: Uuid,
    key: &str,
    fields: &Map<String, Value>,
) -> Result<(), AppError> {
    let client = data::connected_client(state, user_id).await?;
    Ok(edit_issue(&client, key, fields).await?)
}
