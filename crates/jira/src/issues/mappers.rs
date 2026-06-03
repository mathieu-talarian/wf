//! Mappers from raw Jira REST payloads to the lean DTOs in `types.rs` (port of
//! `issues/mappers.ts`). ADF rich text is flattened to plain text here; we never
//! ship raw ADF/HTML to the client as trusted markup.

use serde::Deserialize;
use serde_json::Value;

use super::adf::adf_to_text;
use crate::types::{
    JiraAllowedRef, JiraBoard, JiraComment, JiraDescriptorSchema, JiraFieldDescriptor,
    JiraIssueDetail, JiraIssueSummary, JiraIssueType, JiraNamedIcon, JiraProject, JiraStatus,
    JiraTransition, JiraUser,
};

#[derive(Debug, Clone, Deserialize)]
struct RawAvatar {
    #[serde(rename = "24x24")]
    x24: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawUser {
    account_id: Option<String>,
    display_name: Option<String>,
    email_address: Option<String>,
    avatar_urls: Option<RawAvatar>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawNamed {
    name: Option<String>,
    icon_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawProject {
    id: Option<String>,
    key: Option<String>,
    name: Option<String>,
    avatar_urls: Option<RawAvatar>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawStatusCategory {
    key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawStatus {
    name: Option<String>,
    status_category: Option<RawStatusCategory>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawComment {
    id: Option<String>,
    author: Option<RawUser>,
    body: Option<Value>,
    created: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawCommentContainer {
    comments: Option<Vec<RawComment>>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawFields {
    summary: Option<String>,
    status: Option<RawStatus>,
    issuetype: Option<RawNamed>,
    priority: Option<RawNamed>,
    assignee: Option<RawUser>,
    reporter: Option<RawUser>,
    project: Option<RawProject>,
    updated: Option<String>,
    labels: Option<Vec<String>>,
    description: Option<Value>,
    comment: Option<RawCommentContainer>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawIssue {
    key: Option<String>,
    fields: Option<RawFields>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawTransitionTo {
    name: Option<String>,
    #[serde(rename = "statusCategory")]
    status_category: Option<RawStatusCategory>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawTransition {
    id: Option<String>,
    name: Option<String>,
    to: Option<RawTransitionTo>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawSchema {
    r#type: Option<String>,
    items: Option<String>,
    system: Option<String>,
    custom: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawFieldMeta {
    name: Option<String>,
    required: Option<bool>,
    schema: Option<RawSchema>,
    allowed_values: Option<Vec<JiraAllowedRef>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawIssueType {
    id: Option<String>,
    name: Option<String>,
    icon_url: Option<String>,
    subtask: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawBoard {
    id: Option<i64>,
    name: Option<String>,
    r#type: Option<String>,
}

pub fn map_user(raw: Option<&RawUser>) -> Option<JiraUser> {
    let raw = raw?;
    let account_id = raw.account_id.clone()?;
    Some(JiraUser {
        display_name: raw.display_name.clone().unwrap_or_else(|| account_id.clone()),
        avatar_url: raw.avatar_urls.as_ref().and_then(|a| a.x24.clone()),
        email_address: raw.email_address.clone(),
        account_id,
    })
}

fn map_named(raw: Option<&RawNamed>) -> Option<JiraNamedIcon> {
    raw.map(|r| JiraNamedIcon {
        name: r.name.clone().unwrap_or_default(),
        icon_url: r.icon_url.clone(),
    })
}

pub fn map_issue_summary(site_url: &str, raw: &RawIssue) -> JiraIssueSummary {
    let f = raw.fields.as_ref();
    let key = raw.key.clone().unwrap_or_default();
    let project_key = f
        .and_then(|f| f.project.as_ref())
        .and_then(|p| p.key.clone())
        .unwrap_or_else(|| key.split('-').next().unwrap_or("").to_string());
    JiraIssueSummary {
        summary: f.and_then(|f| f.summary.clone()).unwrap_or_default(),
        status: JiraStatus {
            name: f
                .and_then(|f| f.status.as_ref())
                .and_then(|s| s.name.clone())
                .unwrap_or_else(|| "Unknown".to_string()),
            category: f
                .and_then(|f| f.status.as_ref())
                .and_then(|s| s.status_category.as_ref())
                .and_then(|c| c.key.clone())
                .unwrap_or_else(|| "undefined".to_string()),
        },
        issue_type: map_named(f.and_then(|f| f.issuetype.as_ref()))
            .unwrap_or(JiraNamedIcon { name: String::new(), icon_url: None }),
        priority: map_named(f.and_then(|f| f.priority.as_ref())),
        assignee: map_user(f.and_then(|f| f.assignee.as_ref())),
        project_key,
        updated: f.and_then(|f| f.updated.clone()).unwrap_or_default(),
        url: format!("{site_url}/browse/{key}"),
        key,
    }
}

pub fn map_comment(raw: &RawComment) -> JiraComment {
    JiraComment {
        id: raw.id.clone().unwrap_or_default(),
        author: map_user(raw.author.as_ref()),
        body: adf_to_text(raw.body.as_ref()),
        created: raw.created.clone().unwrap_or_default(),
    }
}

pub fn map_issue_detail(site_url: &str, raw: &RawIssue) -> JiraIssueDetail {
    let summary = map_issue_summary(site_url, raw);
    let f = raw.fields.as_ref();
    JiraIssueDetail {
        description: adf_to_text(f.and_then(|f| f.description.as_ref())),
        reporter: map_user(f.and_then(|f| f.reporter.as_ref())),
        labels: f.and_then(|f| f.labels.clone()).unwrap_or_default(),
        comments: f
            .and_then(|f| f.comment.as_ref())
            .and_then(|c| c.comments.as_ref())
            .map(|cs| cs.iter().map(map_comment).collect())
            .unwrap_or_default(),
        summary,
    }
}

pub fn map_transition(raw: &RawTransition) -> JiraTransition {
    JiraTransition {
        id: raw.id.clone().unwrap_or_default(),
        name: raw.name.clone().unwrap_or_default(),
        to_status: raw.to.as_ref().and_then(|t| t.name.clone()).unwrap_or_default(),
        to_category: raw
            .to
            .as_ref()
            .and_then(|t| t.status_category.as_ref())
            .and_then(|c| c.key.clone())
            .unwrap_or_else(|| "undefined".to_string()),
    }
}

pub fn map_project(raw: &RawProject) -> JiraProject {
    JiraProject {
        id: raw.id.clone().unwrap_or_default(),
        key: raw.key.clone().unwrap_or_default(),
        name: raw.name.clone().unwrap_or_default(),
        avatar_url: raw.avatar_urls.as_ref().and_then(|a| a.x24.clone()),
    }
}

pub fn map_field_descriptor(field_id: &str, raw: &RawFieldMeta) -> JiraFieldDescriptor {
    JiraFieldDescriptor {
        field_id: field_id.to_string(),
        name: raw.name.clone().unwrap_or_else(|| field_id.to_string()),
        required: raw.required.unwrap_or(false),
        schema: JiraDescriptorSchema {
            r#type: raw
                .schema
                .as_ref()
                .and_then(|s| s.r#type.clone())
                .unwrap_or_else(|| "string".to_string()),
            items: raw.schema.as_ref().and_then(|s| s.items.clone()),
            system: raw.schema.as_ref().and_then(|s| s.system.clone()),
            custom: raw.schema.as_ref().and_then(|s| s.custom.clone()),
        },
        allowed_values: raw.allowed_values.clone().unwrap_or_default(),
    }
}

pub fn map_issue_type(raw: &RawIssueType) -> JiraIssueType {
    JiraIssueType {
        id: raw.id.clone().unwrap_or_default(),
        name: raw.name.clone().unwrap_or_default(),
        icon_url: raw.icon_url.clone(),
        subtask: raw.subtask.unwrap_or(false),
    }
}

pub fn map_board(raw: &RawBoard) -> JiraBoard {
    JiraBoard {
        id: raw.id.unwrap_or(0),
        name: raw.name.clone().unwrap_or_default(),
        r#type: raw.r#type.clone().unwrap_or_default(),
    }
}
