//! Pure normalizers (A1 spec §5.1): provider DTO → `events` row input.
//! `external_id` forms are spec §6.1 — they encode provider instance + entity
//! + state, so re-observations collide (deduped) and state changes don't.

use sea_orm::prelude::Uuid;
use serde_json::json;
use wf_db::tables::events::InsertEventInput;
use wf_github::{PolledPullRequest, PolledWorkflowRun};
use wf_jira::PolledIssue;

use crate::cursor::parse_jira_ts;

/// `github.workflow_run.completed` for a completed run; `None` otherwise.
pub fn workflow_run_event(
    user_id: Uuid,
    repo: &str,
    run: &PolledWorkflowRun,
) -> Option<InsertEventInput> {
    if run.status.as_deref() != Some("completed") {
        return None;
    }
    let conclusion = run.conclusion.clone().unwrap_or_else(|| "unknown".into());
    Some(InsertEventInput {
        user_id,
        source: "github".into(),
        event_type: "github.workflow_run.completed".into(),
        external_id: format!("repo:{repo}:wfrun:{}:{}:{conclusion}", run.id, run.run_attempt),
        scope_key: repo.to_string(),
        actor: run.actor.as_ref().map(|a| a.login.clone()),
        title: run.display_title.clone().or_else(|| run.name.clone()),
        url: run.html_url.clone(),
        occurred_at: run.updated_at.fixed_offset(),
        payload: json!({
            "runId": run.id,
            "runAttempt": run.run_attempt,
            "conclusion": conclusion,
            "headBranch": run.head_branch,
            "name": run.name,
        }),
    })
}

/// `github.pull_request.{opened|merged|closed}` from the PR's current state.
pub fn pull_request_event(
    user_id: Uuid,
    repo: &str,
    pr: &PolledPullRequest,
) -> Option<InsertEventInput> {
    let (state, event_type, occurred_at) = match (pr.state.as_str(), pr.merged_at) {
        ("open", _) => ("open", "github.pull_request.opened", pr.created_at),
        ("closed", Some(merged)) => ("merged", "github.pull_request.merged", merged),
        ("closed", None) => {
            ("closed", "github.pull_request.closed", pr.closed_at.unwrap_or(pr.updated_at))
        }
        _ => return None,
    };
    Some(InsertEventInput {
        user_id,
        source: "github".into(),
        event_type: event_type.into(),
        external_id: format!("repo:{repo}:pr:{}:{state}", pr.number),
        scope_key: repo.to_string(),
        actor: pr.user.as_ref().map(|a| a.login.clone()),
        title: pr.title.clone(),
        url: pr.html_url.clone(),
        occurred_at: occurred_at.fixed_offset(),
        payload: json!({
            "number": pr.number,
            "state": state,
            "draft": pr.draft,
            "title": pr.title,
        }),
    })
}

/// Classify a Jira issue change: returns `(event_type, external_id)` or `None`
/// when the status is unchanged or missing.
fn jira_event_kind<'a>(
    site_key: &str,
    issue: &'a PolledIssue,
    prev_status_id: Option<&str>,
    status_id: &'a str,
) -> Option<(&'static str, String)> {
    match prev_status_id {
        None => Some(("jira.issue.created", format!("site:{site_key}:issue:{}:created", issue.key))),
        Some(prev) if prev == status_id => None,
        Some(_) => Some((
            "jira.issue.transitioned",
            format!(
                "site:{site_key}:issue:{}:status:{status_id}:updated:{}",
                issue.key,
                issue.updated.as_deref().unwrap_or("unknown")
            ),
        )),
    }
}

/// `jira.issue.created` (no prior ingested status) or
/// `jira.issue.transitioned` (status differs); `None` when unchanged or the
/// issue lacks a status id (spec §5.1).
pub fn jira_issue_event(
    user_id: Uuid,
    project: &str,
    site_key: &str,
    prev_status_id: Option<&str>,
    issue: &PolledIssue,
) -> Option<InsertEventInput> {
    let status_id = issue.status_id.as_deref()?;
    let (event_type, external_id) = jira_event_kind(site_key, issue, prev_status_id, status_id)?;
    Some(InsertEventInput {
        user_id,
        source: "jira".into(),
        event_type: event_type.into(),
        external_id,
        scope_key: project.to_string(),
        actor: None,
        title: issue.summary.clone(),
        url: Some(issue.url.clone()),
        occurred_at: occurred_ts(event_type, issue),
        payload: json!({
            "issueKey": issue.key,
            "statusId": status_id,
            "statusName": issue.status_name,
            "statusCategory": issue.status_category,
            "summary": issue.summary,
        }),
    })
}

/// `created` events use the issue's creation time; transitions use `updated`.
/// Unparseable/missing provider times fall back to "now" (ingestion time).
fn occurred_ts(event_type: &str, issue: &PolledIssue) -> sea_orm::prelude::DateTimeWithTimeZone {
    let raw = if event_type == "jira.issue.created" { &issue.created } else { &issue.updated };
    raw.as_deref()
        .and_then(parse_jira_ts)
        .unwrap_or_else(|| chrono::Utc::now().fixed_offset())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use wf_github::GithubActor;

    fn run(status: &str, conclusion: Option<&str>) -> PolledWorkflowRun {
        PolledWorkflowRun {
            id: 42,
            run_attempt: 2,
            name: Some("CI".into()),
            display_title: Some("Fix build".into()),
            status: Some(status.into()),
            conclusion: conclusion.map(Into::into),
            html_url: Some("https://x/runs/42".into()),
            head_branch: Some("main".into()),
            created_at: Utc.timestamp_opt(100, 0).unwrap(),
            updated_at: Utc.timestamp_opt(200, 0).unwrap(),
            actor: Some(GithubActor { login: "octocat".into() }),
        }
    }

    #[test]
    fn workflow_run_only_when_completed() {
        let uid = Uuid::new_v4();
        let ev = workflow_run_event(uid, "o/r", &run("completed", Some("failure"))).unwrap();
        assert_eq!(ev.external_id, "repo:o/r:wfrun:42:2:failure");
        assert_eq!(ev.event_type, "github.workflow_run.completed");
        assert_eq!(ev.payload["conclusion"], "failure");
        assert!(workflow_run_event(uid, "o/r", &run("in_progress", None)).is_none());
    }

    fn pr(state: &str, merged: bool) -> PolledPullRequest {
        PolledPullRequest {
            number: 7,
            state: state.into(),
            title: Some("Add feature".into()),
            html_url: Some("https://x/pull/7".into()),
            draft: Some(false),
            created_at: Utc.timestamp_opt(100, 0).unwrap(),
            updated_at: Utc.timestamp_opt(300, 0).unwrap(),
            closed_at: Some(Utc.timestamp_opt(300, 0).unwrap()),
            merged_at: merged.then(|| Utc.timestamp_opt(300, 0).unwrap()),
            user: Some(GithubActor { login: "octocat".into() }),
        }
    }

    #[test]
    fn pull_request_state_mapping() {
        let uid = Uuid::new_v4();
        assert_eq!(pull_request_event(uid, "o/r", &pr("open", false)).unwrap().external_id, "repo:o/r:pr:7:open");
        assert_eq!(
            pull_request_event(uid, "o/r", &pr("closed", true)).unwrap().event_type,
            "github.pull_request.merged"
        );
        assert_eq!(
            pull_request_event(uid, "o/r", &pr("closed", false)).unwrap().external_id,
            "repo:o/r:pr:7:closed"
        );
    }

    fn issue(status_id: &str) -> PolledIssue {
        PolledIssue {
            key: "PROJ-1".into(),
            summary: Some("Fix login".into()),
            status_id: Some(status_id.into()),
            status_name: Some("In Progress".into()),
            status_category: Some("indeterminate".into()),
            created: Some("2026-06-01T10:00:00.000+0200".into()),
            updated: Some("2026-06-02T11:00:00.000+0200".into()),
            url: "https://x/browse/PROJ-1".into(),
        }
    }

    #[test]
    fn jira_created_transitioned_unchanged() {
        let uid = Uuid::new_v4();
        let created = jira_issue_event(uid, "PROJ", "site1", None, &issue("3")).unwrap();
        assert_eq!(created.event_type, "jira.issue.created");
        assert_eq!(created.external_id, "site:site1:issue:PROJ-1:created");
        assert_eq!(created.payload["issueKey"], "PROJ-1");
        assert_eq!(created.payload["statusId"], "3");

        let moved = jira_issue_event(uid, "PROJ", "site1", Some("2"), &issue("3")).unwrap();
        assert_eq!(moved.event_type, "jira.issue.transitioned");
        assert!(moved.external_id.contains(":status:3:updated:"));

        assert!(jira_issue_event(uid, "PROJ", "site1", Some("3"), &issue("3")).is_none());
    }
}
