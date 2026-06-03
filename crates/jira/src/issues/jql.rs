//! JQL builders for the dashboard queues (port of `issues/jql.ts`). User/
//! system-controlled literals (accountId, project keys) always pass through
//! `quote_jql_string` so they cannot break out of the string literal and alter
//! the query (JQL injection).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JiraQueueKey {
    Assigned,
    PreviouslyMine,
    Reported,
    Watching,
    ActiveSprint,
}

impl JiraQueueKey {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "assigned" => Self::Assigned,
            "previously_mine" => Self::PreviouslyMine,
            "reported" => Self::Reported,
            "watching" => Self::Watching,
            "active_sprint" => Self::ActiveSprint,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct QueueJqlCtx {
    pub account_id: String,
    pub selected_projects: Vec<String>,
}

pub fn quote_jql_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

const ORDER: &str = "ORDER BY updated DESC";

fn base_clause(key: JiraQueueKey, account_id: &str) -> String {
    match key {
        JiraQueueKey::Assigned => {
            "assignee = currentUser() AND resolution = Unresolved".to_string()
        }
        JiraQueueKey::PreviouslyMine => format!(
            "assignee WAS {} AND (assignee != currentUser() OR assignee IS EMPTY) AND resolution = Unresolved",
            quote_jql_string(account_id)
        ),
        JiraQueueKey::Reported => {
            "reporter = currentUser() AND resolution = Unresolved".to_string()
        }
        JiraQueueKey::Watching => {
            "watcher = currentUser() AND resolution = Unresolved".to_string()
        }
        JiraQueueKey::ActiveSprint => {
            "assignee = currentUser() AND sprint IN openSprints()".to_string()
        }
    }
}

fn project_clause(projects: &[String]) -> String {
    if projects.is_empty() {
        return String::new();
    }
    let list: Vec<String> = projects.iter().map(|p| quote_jql_string(p)).collect();
    format!(" AND project IN ({})", list.join(", "))
}

pub fn build_queue_jql(key: JiraQueueKey, ctx: &QueueJqlCtx) -> String {
    format!(
        "{}{} {}",
        base_clause(key, &ctx.account_id),
        project_clause(&ctx.selected_projects),
        ORDER
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(projects: &[&str]) -> QueueJqlCtx {
        QueueJqlCtx {
            account_id: "5b10a2".to_string(),
            selected_projects: projects.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn quotes_plain_value() {
        assert_eq!(quote_jql_string("ABC"), "\"ABC\"");
    }

    #[test]
    fn escapes_embedded_double_quotes() {
        assert_eq!(quote_jql_string("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn escapes_backslashes_before_quotes() {
        assert_eq!(quote_jql_string("a\\b"), "\"a\\\\b\"");
    }

    #[test]
    fn builds_assigned() {
        assert_eq!(
            build_queue_jql(JiraQueueKey::Assigned, &ctx(&[])),
            "assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC"
        );
    }

    #[test]
    fn builds_previously_mine_with_quoted_account_id() {
        assert_eq!(
            build_queue_jql(JiraQueueKey::PreviouslyMine, &ctx(&[])),
            "assignee WAS \"5b10a2\" AND (assignee != currentUser() OR assignee IS EMPTY) AND resolution = Unresolved ORDER BY updated DESC"
        );
    }

    #[test]
    fn builds_reported() {
        assert_eq!(
            build_queue_jql(JiraQueueKey::Reported, &ctx(&[])),
            "reporter = currentUser() AND resolution = Unresolved ORDER BY updated DESC"
        );
    }

    #[test]
    fn builds_watching() {
        assert_eq!(
            build_queue_jql(JiraQueueKey::Watching, &ctx(&[])),
            "watcher = currentUser() AND resolution = Unresolved ORDER BY updated DESC"
        );
    }

    #[test]
    fn builds_active_sprint() {
        assert_eq!(
            build_queue_jql(JiraQueueKey::ActiveSprint, &ctx(&[])),
            "assignee = currentUser() AND sprint IN openSprints() ORDER BY updated DESC"
        );
    }

    #[test]
    fn scopes_by_selected_projects_before_order_by() {
        assert_eq!(
            build_queue_jql(JiraQueueKey::Assigned, &ctx(&["ABC", "XYZ"])),
            "assignee = currentUser() AND resolution = Unresolved AND project IN (\"ABC\", \"XYZ\") ORDER BY updated DESC"
        );
    }

    #[test]
    fn neutralizes_malicious_project_key() {
        let jql = build_queue_jql(JiraQueueKey::Assigned, &ctx(&["X\") OR (\"1\"=\"1"]));
        assert!(jql.contains("project IN (\"X\\\") OR (\\\"1\\\"=\\\"1\")"));
        assert!(!jql.contains(") OR (\"1\"=\"1\""));
    }
}
