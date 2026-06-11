//! Board assembly: Jira tickets bucketed into mapped columns, enriched with
//! matched GitHub PRs/branches (Jira-key regex over branch names + PR titles;
//! manual `ticket_links` always win), Slack/notes/reminder badges, and the
//! orphan tray. Matching lives HERE and nowhere else.

use std::collections::{HashMap, HashSet};

use futures::stream::{self, StreamExt};
use sea_orm::prelude::Uuid;
use wf_db::tables::{
    jira_pat_connections, notes, reminders, slack_messages, ticket_links,
};
use wf_github::{
    enrich_pull_requests, fetch_branch_prompts, fetch_workflows, list_pulls_page,
    GithubCheckState, GithubClient, GithubPullEnrichmentResult, GithubPullRef,
    GithubWorkflowSummary, PolledPullRequest,
};
use crate::error::AppError;
use crate::github::activity::require_pat;
use crate::hub::cache;
use crate::hub::types::{
    HubBoard, HubBoardContext, HubColumn, HubFavoriteWorkflow, HubLinkSuggestion, HubOrphanBranch,
    HubOrphanPr, HubOrphanTicket, HubOrphans, HubPrRef, HubTicketCard,
};
use crate::jira::routes::load_board_mapping;
use crate::state::AppState;

const MAX_REPOS: usize = 8;
const MAX_ENRICH: usize = 12;
const JQL_WINDOW: &str = "-14d";

pub async fn board(state: &AppState, user_id: Uuid) -> Result<HubBoard, AppError> {
    if let Some(cached) = cache::get_board(user_id) {
        return Ok(cached);
    }

    // ---- Inputs ----------------------------------------------------------
    let pat = require_pat(state, user_id).await?;
    let repos: Vec<String> = pat.selected_repos.iter().take(MAX_REPOS).cloned().collect();
    let mapping = load_board_mapping(state, user_id).await?;
    let projects = selected_projects(state, user_id).await?;
    if projects.is_empty() {
        return Err(AppError::validation("No Jira projects selected."));
    }

    let jql = format!(
        "project in ({}) AND updated >= {JQL_WINDOW} ORDER BY updated DESC",
        projects.join(",")
    );
    let issues = crate::jira::data::search(state, user_id, &jql, None).await?.issues;

    let client = GithubClient::new(&pat.token);
    let pulls: Vec<(String, PolledPullRequest)> = stream::iter(repos.clone())
        .map(|repo| {
            let client = &client;
            async move {
                let Some((owner, name)) = repo.split_once('/') else {
                    return Vec::new();
                };
                list_pulls_page(client, owner, name)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| (repo.clone(), p))
                    .collect::<Vec<_>>()
            }
        })
        .buffered(4)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flatten()
        .collect();

    let branch_prompts = fetch_branch_prompts(&pat.token, &pat.login, &repos).await;
    let workflow_repos = fetch_workflows(&pat.token, &repos).await;
    let favorites = wf_db::tables::github_pat_connections::get_favorites(&state.db, user_id)
        .await?;
    let manual = ticket_links::list(&state.db, user_id).await?;
    let slack_unread = slack_messages::unread_counts(&state.db, user_id).await?;
    let note_keys: HashSet<String> =
        notes::keys_with_notes(&state.db, user_id).await?.into_iter().collect();
    let reminders_due = reminders::due_counts(&state.db, user_id).await?;

    // ---- Matching --------------------------------------------------------
    let key_regex = project_key_regex(&projects);
    let known_keys: HashSet<&str> = issues.iter().map(|i| i.key.as_str()).collect();
    let manual_pr: HashMap<(String, i64), String> = manual
        .iter()
        .filter_map(|l| l.pr_number.map(|n| ((l.repo.clone(), n), l.ticket_key.clone())))
        .collect();
    let manual_branch: HashMap<(String, String), String> = manual
        .iter()
        .filter_map(|l| l.branch.clone().map(|b| ((l.repo.clone(), b), l.ticket_key.clone())))
        .collect();

    let match_key = |text: &str| -> Option<String> {
        key_regex
            .find(text)
            .map(|m| m.as_str().to_string())
            .filter(|k| known_keys.contains(k.as_str()))
    };

    let mut ticket_prs: HashMap<String, Vec<HubPrRef>> = HashMap::new();
    let mut orphan_prs: Vec<HubOrphanPr> = Vec::new();
    for (repo, pull) in &pulls {
        let branch = pull.head.as_ref().map(|h| h.ref_name.clone()).unwrap_or_default();
        let title = pull.title.clone().unwrap_or_default();
        let matched = manual_pr
            .get(&(repo.clone(), pull.number))
            .cloned()
            .or_else(|| match_key(&branch))
            .or_else(|| match_key(&title));
        let pr = HubPrRef {
            repo: repo.clone(),
            number: pull.number,
            title: title.clone(),
            state: pr_state(pull),
            check_state: "none".to_string(),
            branch: branch.clone(),
            url: pull.html_url.clone().unwrap_or_default(),
        };
        match matched {
            Some(key) => ticket_prs.entry(key).or_default().push(pr),
            None if pr.state == "open" || pr.state == "draft" => {
                orphan_prs.push(HubOrphanPr {
                    repo: repo.clone(),
                    number: pull.number,
                    title,
                    branch,
                    url: pr.url.clone(),
                    suggestion: None,
                });
            }
            None => {}
        }
    }

    let mut ticket_branch: HashMap<String, String> = HashMap::new();
    let mut orphan_branches: Vec<HubOrphanBranch> = Vec::new();
    for repo_branches in &branch_prompts {
        for prompt in &repo_branches.branches {
            let repo = repo_branches.repo_full_name.clone();
            let matched = manual_branch
                .get(&(repo.clone(), prompt.name.clone()))
                .cloned()
                .or_else(|| match_key(&prompt.name));
            match matched {
                Some(key) => {
                    ticket_branch.entry(key).or_insert_with(|| prompt.name.clone());
                }
                None => orphan_branches
                    .push(HubOrphanBranch { repo, name: prompt.name.clone(), suggestion: None }),
            }
        }
    }

    // ---- Check states (bounded enrichment of matched open PRs) -----------
    let enrich_refs: Vec<GithubPullRef> = ticket_prs
        .values()
        .flatten()
        .filter(|pr| pr.state == "open" || pr.state == "draft")
        .take(MAX_ENRICH)
        .filter_map(|pr| {
            let (owner, name) = pr.repo.split_once('/')?;
            Some(GithubPullRef {
                owner: owner.to_string(),
                repo: name.to_string(),
                number: pr.number,
            })
        })
        .collect();
    let enriched: Vec<GithubPullEnrichmentResult> =
        enrich_pull_requests(&pat.token, &enrich_refs).await;
    let check_by_pr: HashMap<(String, i64), String> = enriched
        .iter()
        .map(|e| {
            let repo = format!("{}/{}", e.owner, e.repo);
            ((repo, e.number), aggregate_checks(&e.enrichment.required_checks))
        })
        .collect();
    for prs in ticket_prs.values_mut() {
        for pr in prs.iter_mut() {
            if let Some(check) = check_by_pr.get(&(pr.repo.clone(), pr.number)) {
                pr.check_state.clone_from(check);
            }
        }
    }

    // ---- Cards + columns ---------------------------------------------------
    let runs = crate::hub::runs::runs(state, user_id).await.ok();
    let workflows_by_repo: HashMap<&str, &[GithubWorkflowSummary]> = workflow_repos
        .iter()
        .map(|w| (w.repo_full_name.as_str(), w.workflows.as_slice()))
        .collect();

    let last_column = mapping.columns.len().saturating_sub(1);
    let mut columns: Vec<HubColumn> = mapping
        .columns
        .iter()
        .map(|c| HubColumn {
            id: c.id.clone(),
            title: c.title.clone(),
            jira_statuses: c.jira_statuses.clone(),
            tickets: vec![],
        })
        .collect();
    let mut orphan_tickets: Vec<HubOrphanTicket> = Vec::new();

    for issue in &issues {
        let prs = ticket_prs.remove(&issue.key).unwrap_or_default();
        let branch = ticket_branch
            .remove(&issue.key)
            .or_else(|| prs.first().map(|p| p.branch.clone()));
        let column_index = mapping
            .columns
            .iter()
            .position(|c| c.jira_statuses.iter().any(|s| s.eq_ignore_ascii_case(&issue.status.name)))
            .unwrap_or(0);
        let check_state = aggregate_pr_states(&prs);
        let card_repo = prs
            .first()
            .map(|p| p.repo.clone())
            .or_else(|| repos.first().cloned());
        let favorite_workflow = card_repo.and_then(|repo| {
            let ids = favorites.get(&repo)?;
            let id = *ids.first()?;
            let summary = workflows_by_repo.get(repo.as_str())?.iter().find(|w| w.id == id)?;
            let last_run = runs.as_ref().and_then(|r| {
                r.runs.iter().find(|p| p.repo == repo && p.workflow_id == id).cloned()
            });
            Some(HubFavoriteWorkflow {
                repo,
                workflow_id: id,
                workflow_name: summary.name.clone(),
                path: summary.path.clone(),
                last_run,
            })
        });

        if prs.is_empty()
            && branch.is_none()
            && column_index != 0
            && column_index != last_column
        {
            orphan_tickets.push(HubOrphanTicket {
                key: issue.key.clone(),
                summary: issue.summary.clone(),
                status: issue.status.name.clone(),
            });
        }

        let card = HubTicketCard {
            key: issue.key.clone(),
            summary: issue.summary.clone(),
            status: issue.status.name.clone(),
            assignee: issue.assignee.clone(),
            updated_at: issue.updated.clone(),
            bug_count: 0,
            ready: !prs.is_empty() && check_state == "success",
            branch,
            check_state,
            prs,
            deployment: None,
            slack_unread: *slack_unread.get(&issue.key).unwrap_or(&0),
            notes_count: i64::from(note_keys.contains(&issue.key)),
            reminders_due: *reminders_due.get(&issue.key).unwrap_or(&0),
            favorite_workflow,
        };
        columns[column_index].tickets.push(card);
    }

    // ---- Heuristic link suggestions ---------------------------------------
    let summaries: Vec<(&str, &str)> =
        issues.iter().map(|i| (i.key.as_str(), i.summary.as_str())).collect();
    for orphan in &mut orphan_prs {
        orphan.suggestion = suggest(&format!("{} {}", orphan.title, orphan.branch), &summaries);
    }
    for orphan in &mut orphan_branches {
        orphan.suggestion = suggest(&orphan.name, &summaries);
    }

    let board = HubBoard {
        columns,
        orphans: HubOrphans {
            prs: orphan_prs,
            branches: orphan_branches,
            tickets: orphan_tickets,
        },
        context: HubBoardContext {
            jira_project_key: projects.first().cloned(),
            repos,
            generated_at: chrono::Utc::now().to_rfc3339(),
        },
    };
    cache::put_board(user_id, &board);
    Ok(board)
}

async fn selected_projects(state: &AppState, user_id: Uuid) -> Result<Vec<String>, AppError> {
    let row = jira_pat_connections::select_row(&state.db, user_id).await?;
    Ok(row
        .and_then(|r| r.selected_projects)
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .unwrap_or_default())
}

/// `\b(?:KEY1|KEY2)-\d+\b` over the selected project keys.
fn project_key_regex(projects: &[String]) -> regex::Regex {
    let alternation =
        projects.iter().map(|p| regex::escape(p)).collect::<Vec<_>>().join("|");
    regex::Regex::new(&format!(r"\b(?:{alternation})-\d+\b"))
        .unwrap_or_else(|_| regex::Regex::new(r"\b[A-Z][A-Z0-9]+-\d+\b").expect("fallback regex"))
}

fn pr_state(pull: &PolledPullRequest) -> String {
    if pull.merged_at.is_some() {
        "merged".to_string()
    } else if pull.state == "closed" {
        "closed".to_string()
    } else if pull.draft.unwrap_or(false) {
        "draft".to_string()
    } else {
        "open".to_string()
    }
}

fn aggregate_checks(checks: &[wf_github::GithubRequiredCheck]) -> String {
    if checks.is_empty() {
        return "none".to_string();
    }
    let mut running = false;
    for check in checks {
        match check.state {
            GithubCheckState::Failure
            | GithubCheckState::TimedOut
            | GithubCheckState::Cancelled
            | GithubCheckState::ActionRequired => return "failed".to_string(),
            GithubCheckState::Pending | GithubCheckState::Unknown => running = true,
            GithubCheckState::Success | GithubCheckState::Skipped | GithubCheckState::Neutral => {}
        }
    }
    if running { "running".to_string() } else { "success".to_string() }
}

/// Aggregate of the open PRs' check states (failed > running > success > none).
fn aggregate_pr_states(prs: &[HubPrRef]) -> String {
    let open: Vec<&str> = prs
        .iter()
        .filter(|p| p.state == "open" || p.state == "draft")
        .map(|p| p.check_state.as_str())
        .collect();
    if open.contains(&"failed") {
        "failed".to_string()
    } else if open.contains(&"running") {
        "running".to_string()
    } else if !open.is_empty() && open.iter().all(|s| *s == "success") {
        "success".to_string()
    } else {
        "none".to_string()
    }
}

/// Cheap word-overlap suggestion: ≥2 shared tokens (len > 3) between the
/// orphan's text and a ticket summary.
fn suggest(text: &str, summaries: &[(&str, &str)]) -> Option<HubLinkSuggestion> {
    let tokens: HashSet<String> = tokenize(text);
    let (key, score) = summaries
        .iter()
        .map(|(key, summary)| {
            let candidate = tokenize(summary);
            (*key, tokens.intersection(&candidate).count())
        })
        .max_by_key(|(_, score)| *score)?;
    (score >= 2).then(|| HubLinkSuggestion {
        ticket_key: key.to_string(),
        confidence: (0.3 + 0.1 * score as f64).min(0.7),
        source: "heuristic".to_string(),
    })
}

fn tokenize(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 3)
        .map(str::to_lowercase)
        .collect()
}
