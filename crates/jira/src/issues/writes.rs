//! Write-side runners (port of `write-runners.ts`). All mutations flow through
//! here; create/edit additionally pass user `fields` through the metadata
//! allowlist (`build_issue_fields`) before touching Jira. Upstream 4xx become
//! `JiraWriteError` so the user sees Jira's message with the right status.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::adf::text_to_adf;
use super::enc;
use super::fields::{
    build_issue_fields, BuildFieldsOpts, FieldMetaMap, JiraAllowedValue, JiraFieldMeta,
    JiraFieldSchema,
};
use super::mappers::{map_comment, RawComment};
use super::reads::{create_meta, edit_meta};
use crate::client::JiraClient;
use crate::errors::{JiraActionError, JiraWriteError};
use crate::types::{
    JiraComment, JiraCreateIssueInput, JiraCreateIssueResult, JiraFieldDescriptor, JiraWorklogInput,
};

fn descriptor_to_meta(d: &JiraFieldDescriptor) -> JiraFieldMeta {
    JiraFieldMeta {
        field_id: d.field_id.clone(),
        required: d.required,
        schema: JiraFieldSchema {
            r#type: d.schema.r#type.clone(),
            items: d.schema.items.clone(),
            system: d.schema.system.clone(),
            custom: d.schema.custom.clone(),
        },
        allowed_values: d
            .allowed_values
            .iter()
            .map(|v| JiraAllowedValue { id: v.id.clone(), value: v.value.clone() })
            .collect(),
    }
}

fn to_meta_map(descriptors: &[JiraFieldDescriptor]) -> FieldMetaMap {
    descriptors.iter().map(|d| (d.field_id.clone(), descriptor_to_meta(d))).collect()
}

fn reject(reason: String) -> JiraActionError {
    JiraActionError::Write(JiraWriteError { status: 400, message: reason })
}

pub async fn transition_issue(
    client: &JiraClient,
    key: &str,
    transition_id: &str,
) -> Result<(), JiraActionError> {
    let path = format!("/rest/api/3/issue/{}/transitions", enc(key));
    client
        .post::<Value>(&path, &json!({ "transition": { "id": transition_id } }))
        .await
        .map_err(JiraActionError::as_write)?;
    Ok(())
}

pub async fn add_comment(
    client: &JiraClient,
    key: &str,
    body: &str,
) -> Result<JiraComment, JiraActionError> {
    let path = format!("/rest/api/3/issue/{}/comment", enc(key));
    let raw: RawComment = client
        .post(&path, &json!({ "body": text_to_adf(body) }))
        .await
        .map_err(JiraActionError::as_write)?;
    Ok(map_comment(&raw))
}

pub async fn assign_issue(
    client: &JiraClient,
    key: &str,
    account_id: Option<&str>,
) -> Result<(), JiraActionError> {
    let path = format!("/rest/api/3/issue/{}/assignee", enc(key));
    client
        .put::<Value>(&path, &json!({ "accountId": account_id }))
        .await
        .map_err(JiraActionError::as_write)?;
    Ok(())
}

pub async fn log_work(
    client: &JiraClient,
    key: &str,
    input: &JiraWorklogInput,
) -> Result<(), JiraActionError> {
    let mut body = Map::new();
    body.insert("timeSpent".to_string(), json!(input.time_spent));
    if let Some(started) = &input.started {
        body.insert("started".to_string(), json!(started));
    }
    if let Some(comment) = &input.comment {
        body.insert("comment".to_string(), text_to_adf(comment));
    }
    let path = format!("/rest/api/3/issue/{}/worklog", enc(key));
    client.post::<Value>(&path, &Value::Object(body)).await.map_err(JiraActionError::as_write)?;
    Ok(())
}

#[derive(Deserialize)]
struct CreatedIssue {
    id: Option<String>,
    key: Option<String>,
}

async fn build_create_payload(
    client: &JiraClient,
    input: &JiraCreateIssueInput,
) -> Result<Map<String, Value>, JiraActionError> {
    let descriptors =
        create_meta(client, &input.project_key, &input.issue_type_id).await.map_err(JiraActionError::Api)?.fields;
    let mut meta = to_meta_map(&descriptors);
    // `project`/`issuetype` are set explicitly, never from user input.
    meta.remove("project");
    meta.remove("issuetype");
    let mut fields =
        build_issue_fields(&meta, &input.fields, &BuildFieldsOpts { enforce_required: true, ..Default::default() })
            .map_err(reject)?;
    fields.insert("project".to_string(), json!({ "key": input.project_key }));
    fields.insert("issuetype".to_string(), json!({ "id": input.issue_type_id }));
    Ok(fields)
}

pub async fn create_issue(
    client: &JiraClient,
    input: &JiraCreateIssueInput,
) -> Result<JiraCreateIssueResult, JiraActionError> {
    let fields = build_create_payload(client, input).await?;
    let res: CreatedIssue = client
        .post("/rest/api/3/issue", &json!({ "fields": fields }))
        .await
        .map_err(JiraActionError::as_write)?;
    Ok(JiraCreateIssueResult { id: res.id.unwrap_or_default(), key: res.key.unwrap_or_default() })
}

pub async fn edit_issue(
    client: &JiraClient,
    key: &str,
    fields: &Map<String, Value>,
) -> Result<(), JiraActionError> {
    let descriptors = edit_meta(client, key).await.map_err(JiraActionError::Api)?.fields;
    let meta = to_meta_map(&descriptors);
    let built =
        build_issue_fields(&meta, fields, &BuildFieldsOpts { enforce_required: false, ..Default::default() })
            .map_err(reject)?;
    let path = format!("/rest/api/3/issue/{}", enc(key));
    client
        .put::<Value>(&path, &json!({ "fields": built }))
        .await
        .map_err(JiraActionError::as_write)?;
    Ok(())
}
