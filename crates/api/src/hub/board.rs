//! Board assembly: Jira tickets bucketed into mapped columns, enriched with
//! matched GitHub PRs/branches (Jira-key regex over branch names + PR titles;
//! manual `ticket_links` always win), Slack/notes/reminder badges, and the
//! orphan tray. Matching lives HERE and nowhere else.

use std::collections::{HashMap, HashSet};

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use wf_db::tables::{
    github_pat_connections, github_pull_requests, github_workflow_runs, jira_issues,
    jira_pat_connections, notes, reminders, slack_messages, ticket_links,
};
use wf_github::{GithubActor, GithubRepoBranches, PolledPullHead, PolledPullRequest};
use wf_jira::{JiraIssueSummary, JiraNamedIcon, JiraStatus, JiraUser};

use crate::error::AppError;
use crate::hub::types::{
    HubBoard, HubBoardContext, HubColumn, HubFavoriteWorkflow, HubLinkSuggestion, HubOrphanBranch,
    HubOrphanPr, HubOrphanTicket, HubOrphans, HubPrRef, HubRuns, HubTicketCard,
};
use crate::jira::routes::{default_board_mapping, JiraBoardMapping};
use crate::state::AppState;

/// All inputs the board is assembled from, fetched once up front.
struct BoardInputs {
    repos: Vec<String>,
    mapping: JiraBoardMapping,
    projects: Vec<String>,
    issues: Vec<JiraIssueSummary>,
    pulls: Vec<(String, PolledPullRequest)>,
    branch_prompts: Vec<GithubRepoBranches>,
    runs: HubRuns,
    generated_at: String,
    badges: Badges,
}

struct BoardScope {
    repos: Vec<String>,
    projects: Vec<String>,
    mapping: JiraBoardMapping,
    favorites: github_pat_connections::FavoritesMap,
    as_of: DateTimeWithTimeZone,
}

/// Per-ticket badge data sourced from our own tables (not GitHub/Jira).
struct Badges {
    favorites: github_pat_connections::FavoritesMap,
    manual: Vec<ticket_links::Model>,
    slack_unread: HashMap<String, i64>,
    note_keys: HashSet<String>,
    reminders_due: HashMap<String, i64>,
}

/// PR/branch matching results: keyed to tickets, or left in the orphan trays.
#[derive(Default)]
struct BoardMatches {
    ticket_prs: HashMap<String, Vec<HubPrRef>>,
    orphan_prs: Vec<HubOrphanPr>,
    ticket_branch: HashMap<String, String>,
    orphan_branches: Vec<HubOrphanBranch>,
}

/// Durable read-model load: no provider I/O and no process-local data cache.
pub async fn board(state: &AppState, user_id: Uuid) -> Result<HubBoard, AppError> {
    compute_board(state, user_id).await
}

async fn compute_board(state: &AppState, user_id: Uuid) -> Result<HubBoard, AppError> {
    let inputs = gather_inputs(state, user_id).await?;
    let matches = match_repo_activity(&inputs);
    Ok(assemble_board(&inputs, matches))
}

// ---- Inputs --------------------------------------------------------------

async fn gather_inputs(state: &AppState, user_id: Uuid) -> Result<BoardInputs, AppError> {
    let (scope, badges) = tokio::join!(load_scope(state, user_id), fetch_badges(state, user_id));
    let (mut scope, mut badges) = (scope?, badges?);
    if scope.projects.is_empty() {
        return Err(AppError::validation("No Jira projects selected."));
    }
    badges.favorites = std::mem::take(&mut scope.favorites);
    projected_inputs(state, user_id, scope, badges).await
}

async fn projected_inputs(
    state: &AppState,
    user_id: Uuid,
    scope: BoardScope,
    badges: Badges,
) -> Result<BoardInputs, AppError> {
    let (issues, pulls, runs) = tokio::join!(
        jira_issues::list_recent(&state.db, user_id, &scope.projects, 500),
        github_pull_requests::list_recent(&state.db, user_id, &scope.repos, 500),
        github_workflow_runs::list_recent(&state.db, user_id, &scope.repos, 200),
    );
    let (issues, pulls, runs) = (issues?, pulls?, runs?);
    let generated_at = projection_as_of(scope.as_of, &issues, &pulls, &runs);
    let hub_runs = crate::hub::runs::from_projection(runs, &badges.favorites);
    Ok(BoardInputs {
        repos: scope.repos,
        mapping: scope.mapping,
        projects: scope.projects,
        issues: issues.into_iter().map(issue_summary).collect(),
        pulls: pulls.into_iter().map(poll_pull).collect(),
        branch_prompts: vec![],
        runs: hub_runs,
        generated_at,
        badges,
    })
}

async fn load_scope(state: &AppState, user_id: Uuid) -> Result<BoardScope, AppError> {
    let (github, jira) = tokio::join!(
        github_pat_connections::select_row(&state.db, user_id),
        jira_pat_connections::select_row(&state.db, user_id),
    );
    Ok(scope_from_rows(github?, jira?))
}

fn scope_from_rows(
    github: Option<github_pat_connections::Model>,
    jira: Option<jira_pat_connections::Model>,
) -> BoardScope {
    let as_of = connection_as_of(github.as_ref(), jira.as_ref());
    let repos = github.as_ref().and_then(|row| json_strings(row.selected_repos.as_ref()));
    let projects = jira.as_ref().and_then(|row| json_strings(row.selected_projects.as_ref()));
    let mapping = jira.as_ref().and_then(mapping_of).unwrap_or_else(default_board_mapping);
    let favorites = github.as_ref().map(github_pat_connections::favorites_of).unwrap_or_default();
    BoardScope {
        repos: repos.unwrap_or_default(),
        projects: projects.unwrap_or_default(),
        mapping,
        favorites,
        as_of,
    }
}

fn json_strings(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    value.cloned().and_then(|json| serde_json::from_value(json).ok())
}

fn mapping_of(row: &jira_pat_connections::Model) -> Option<JiraBoardMapping> {
    row.board_mapping.clone().and_then(|json| serde_json::from_value(json).ok())
}

fn connection_as_of(
    github: Option<&github_pat_connections::Model>,
    jira: Option<&jira_pat_connections::Model>,
) -> DateTimeWithTimeZone {
    github
        .map(|row| row.updated_at)
        .into_iter()
        .chain(jira.map(|row| row.updated_at))
        .max()
        .unwrap_or_else(|| chrono::Utc::now().into())
}

fn projection_as_of(
    fallback: DateTimeWithTimeZone,
    issues: &[jira_issues::Model],
    pulls: &[github_pull_requests::Model],
    runs: &[github_workflow_runs::Model],
) -> String {
    issues
        .iter()
        .map(|row| row.synced_at)
        .chain(pulls.iter().map(|row| row.synced_at))
        .chain(runs.iter().map(|row| row.synced_at))
        .max()
        .unwrap_or(fallback)
        .to_rfc3339()
}

fn issue_summary(row: jira_issues::Model) -> JiraIssueSummary {
    JiraIssueSummary {
        key: row.issue_key,
        summary: row.summary,
        status: JiraStatus { name: row.status_name, category: row.status_category },
        issue_type: JiraNamedIcon { name: row.issue_type_name.unwrap_or_default(), icon_url: None },
        priority: row.priority_name.map(|name| JiraNamedIcon { name, icon_url: None }),
        assignee: row.assignee_name.map(|display_name| JiraUser {
            account_id: String::new(),
            display_name,
            avatar_url: None,
            email_address: None,
        }),
        project_key: row.project,
        updated: row.updated_at.to_rfc3339(),
        url: row.url,
    }
}

fn poll_pull(row: github_pull_requests::Model) -> (String, PolledPullRequest) {
    let repo = row.repo.clone();
    let user = row.author_login.map(|login| GithubActor { login });
    let pull = PolledPullRequest {
        number: row.number,
        state: row.state,
        title: Some(row.title),
        html_url: Some(row.url),
        draft: Some(row.draft),
        created_at: row.updated_at.with_timezone(&chrono::Utc),
        updated_at: row.updated_at.with_timezone(&chrono::Utc),
        closed_at: None,
        merged_at: None,
        user,
        assignees: actors(row.assignee_logins),
        requested_reviewers: actors(row.requested_reviewer_logins),
        head: row.head_ref.map(|ref_name| PolledPullHead { ref_name }),
    };
    (repo, pull)
}

fn actors(value: serde_json::Value) -> Vec<GithubActor> {
    serde_json::from_value::<Vec<String>>(value)
        .unwrap_or_default()
        .into_iter()
        .map(|login| GithubActor { login })
        .collect()
}

async fn fetch_badges(state: &AppState, user_id: Uuid) -> Result<Badges, AppError> {
    let (manual, slack, note_keys, reminders) = tokio::join!(
        ticket_links::list(&state.db, user_id),
        slack_messages::unread_counts(&state.db, user_id),
        notes::keys_with_notes(&state.db, user_id),
        reminders::due_counts(&state.db, user_id),
    );
    Ok(Badges {
        favorites: Default::default(),
        manual: manual?,
        slack_unread: slack?,
        note_keys: note_keys?.into_iter().collect(),
        reminders_due: reminders?,
    })
}

// ---- Matching ------------------------------------------------------------

/// Resolves a Jira key from free text, gated to keys we actually fetched.
struct KeyMatcher<'a> {
    regex: &'a regex::Regex,
    known: &'a HashSet<&'a str>,
}

impl KeyMatcher<'_> {
    fn match_key(&self, text: &str) -> Option<String> {
        self.regex
            .find(text)
            .map(|m| m.as_str().to_string())
            .filter(|k| self.known.contains(k.as_str()))
    }
}

fn match_repo_activity(inputs: &BoardInputs) -> BoardMatches {
    let key_regex = project_key_regex(&inputs.projects);
    let known: HashSet<&str> = inputs.issues.iter().map(|i| i.key.as_str()).collect();
    let matcher = KeyMatcher { regex: &key_regex, known: &known };
    let mut m = BoardMatches::default();
    match_pulls(&mut m, &inputs.pulls, &matcher, &manual_pr_map(&inputs.badges.manual));
    match_branches(&mut m, &inputs.branch_prompts, &matcher, &manual_branch_map(&inputs.badges.manual));
    m
}

fn manual_pr_map(manual: &[ticket_links::Model]) -> HashMap<(String, i64), String> {
    manual
        .iter()
        .filter_map(|l| l.pr_number.map(|n| ((l.repo.clone(), n), l.ticket_key.clone())))
        .collect()
}

fn manual_branch_map(manual: &[ticket_links::Model]) -> HashMap<(String, String), String> {
    manual
        .iter()
        .filter_map(|l| l.branch.clone().map(|b| ((l.repo.clone(), b), l.ticket_key.clone())))
        .collect()
}

fn match_pulls(
    m: &mut BoardMatches,
    pulls: &[(String, PolledPullRequest)],
    matcher: &KeyMatcher,
    manual_pr: &HashMap<(String, i64), String>,
) {
    for (repo, pull) in pulls {
        let pr = pr_ref(repo, pull);
        let matched = manual_pr
            .get(&(repo.clone(), pull.number))
            .cloned()
            .or_else(|| matcher.match_key(&pr.branch))
            .or_else(|| matcher.match_key(&pr.title));
        match matched {
            Some(key) => m.ticket_prs.entry(key).or_default().push(pr),
            None if pr.state == "open" || pr.state == "draft" => {
                m.orphan_prs.push(orphan_pr(repo, pull, &pr));
            }
            None => {}
        }
    }
}

fn pr_ref(repo: &str, pull: &PolledPullRequest) -> HubPrRef {
    HubPrRef {
        repo: repo.to_string(),
        number: pull.number,
        title: pull.title.clone().unwrap_or_default(),
        state: pr_state(pull),
        check_state: "none".to_string(),
        branch: pull.head.as_ref().map(|h| h.ref_name.clone()).unwrap_or_default(),
        url: pull.html_url.clone().unwrap_or_default(),
    }
}

fn orphan_pr(repo: &str, pull: &PolledPullRequest, pr: &HubPrRef) -> HubOrphanPr {
    HubOrphanPr {
        repo: repo.to_string(),
        number: pull.number,
        title: pr.title.clone(),
        branch: pr.branch.clone(),
        url: pr.url.clone(),
        suggestion: None,
    }
}

fn match_branches(
    m: &mut BoardMatches,
    branch_prompts: &[GithubRepoBranches],
    matcher: &KeyMatcher,
    manual_branch: &HashMap<(String, String), String>,
) {
    for repo_branches in branch_prompts {
        let repo = &repo_branches.repo_full_name;
        for prompt in &repo_branches.branches {
            let matched = manual_branch
                .get(&(repo.clone(), prompt.name.clone()))
                .cloned()
                .or_else(|| matcher.match_key(&prompt.name));
            match matched {
                Some(key) => {
                    m.ticket_branch.entry(key).or_insert_with(|| prompt.name.clone());
                }
                None => m.orphan_branches.push(HubOrphanBranch {
                    repo: repo.clone(),
                    name: prompt.name.clone(),
                    suggestion: None,
                }),
            }
        }
    }
}

// ---- Cards + columns -----------------------------------------------------

/// Read-only context threaded through per-issue card construction.
struct CardContext<'a> {
    inputs: &'a BoardInputs,
}

fn assemble_board(inputs: &BoardInputs, mut matches: BoardMatches) -> HubBoard {
    let ctx = CardContext { inputs };
    let mut columns = init_columns(&inputs.mapping);
    let mut orphan_tickets: Vec<HubOrphanTicket> = Vec::new();
    for issue in &inputs.issues {
        place_issue(&mut columns, &mut orphan_tickets, &mut matches, issue, &ctx);
    }
    let summaries: Vec<(&str, &str)> =
        inputs.issues.iter().map(|i| (i.key.as_str(), i.summary.as_str())).collect();
    apply_suggestions(&mut matches, &summaries);
    build_board(inputs, columns, orphan_tickets, matches)
}

fn init_columns(mapping: &JiraBoardMapping) -> Vec<HubColumn> {
    mapping
        .columns
        .iter()
        .map(|c| HubColumn {
            id: c.id.clone(),
            title: c.title.clone(),
            jira_statuses: c.jira_statuses.clone(),
            tickets: vec![],
        })
        .collect()
}

fn place_issue(
    columns: &mut [HubColumn],
    orphan_tickets: &mut Vec<HubOrphanTicket>,
    matches: &mut BoardMatches,
    issue: &JiraIssueSummary,
    ctx: &CardContext,
) {
    let prs = matches.ticket_prs.remove(&issue.key).unwrap_or_default();
    let branch = matches
        .ticket_branch
        .remove(&issue.key)
        .or_else(|| prs.first().map(|p| p.branch.clone()));
    let column_index = column_index(&ctx.inputs.mapping, &issue.status.name);
    let last_column = ctx.inputs.mapping.columns.len().saturating_sub(1);
    if is_orphan_ticket(&prs, &branch, column_index, last_column) {
        orphan_tickets.push(orphan_ticket(issue));
    }
    columns[column_index].tickets.push(build_card(issue, prs, branch, ctx));
}

fn column_index(mapping: &JiraBoardMapping, status_name: &str) -> usize {
    mapping
        .columns
        .iter()
        .position(|c| c.jira_statuses.iter().any(|s| s.eq_ignore_ascii_case(status_name)))
        .unwrap_or(0)
}

fn is_orphan_ticket(
    prs: &[HubPrRef],
    branch: &Option<String>,
    column_index: usize,
    last_column: usize,
) -> bool {
    prs.is_empty() && branch.is_none() && column_index != 0 && column_index != last_column
}

fn orphan_ticket(issue: &JiraIssueSummary) -> HubOrphanTicket {
    HubOrphanTicket {
        key: issue.key.clone(),
        summary: issue.summary.clone(),
        status: issue.status.name.clone(),
    }
}

fn build_card(
    issue: &JiraIssueSummary,
    prs: Vec<HubPrRef>,
    branch: Option<String>,
    ctx: &CardContext,
) -> HubTicketCard {
    let check_state = aggregate_pr_states(&prs);
    let card_repo = prs.first().map(|p| p.repo.clone()).or_else(|| ctx.inputs.repos.first().cloned());
    let favorite_workflow = resolve_favorite(card_repo, ctx);
    let b = &ctx.inputs.badges;
    HubTicketCard {
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
        slack_unread: *b.slack_unread.get(&issue.key).unwrap_or(&0),
        notes_count: i64::from(b.note_keys.contains(&issue.key)),
        reminders_due: *b.reminders_due.get(&issue.key).unwrap_or(&0),
        favorite_workflow,
    }
}

fn resolve_favorite(card_repo: Option<String>, ctx: &CardContext) -> Option<HubFavoriteWorkflow> {
    let repo = card_repo?;
    let ids = ctx.inputs.badges.favorites.get(&repo)?;
    let id = *ids.first()?;
    let id_s = id.to_string();
    let last_run = ctx
        .inputs
        .runs
        .runs
        .iter()
        .find(|pill| pill.repo == repo && pill.workflow_id == id_s);
    Some(HubFavoriteWorkflow {
        repo,
        workflow_id: id_s,
        workflow_name: last_run
            .map(|pill| pill.workflow_name.clone())
            .unwrap_or_else(|| format!("Workflow {id}")),
        path: String::new(),
        last_run: last_run.map(|pill| pill.started_at.clone()),
    })
}

// ---- Heuristic link suggestions + final assembly -------------------------

fn apply_suggestions(matches: &mut BoardMatches, summaries: &[(&str, &str)]) {
    for orphan in &mut matches.orphan_prs {
        orphan.suggestion = suggest(&format!("{} {}", orphan.title, orphan.branch), summaries);
    }
    for orphan in &mut matches.orphan_branches {
        orphan.suggestion = suggest(&orphan.name, summaries);
    }
}

fn build_board(
    inputs: &BoardInputs,
    columns: Vec<HubColumn>,
    orphan_tickets: Vec<HubOrphanTicket>,
    matches: BoardMatches,
) -> HubBoard {
    HubBoard {
        columns,
        orphans: HubOrphans {
            prs: matches.orphan_prs,
            branches: matches.orphan_branches,
            tickets: orphan_tickets,
        },
        context: HubBoardContext {
            jira_project_key: inputs.projects.first().cloned(),
            repos: inputs.repos.clone(),
            generated_at: inputs.generated_at.clone(),
        },
    }
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
