//! Board assembly: Jira tickets bucketed into mapped columns, enriched with
//! matched GitHub PRs/branches (Jira-key regex over branch names + PR titles;
//! manual `ticket_links` always win), Slack/notes/reminder badges, and the
//! orphan tray. Matching lives HERE and nowhere else.

use std::collections::{HashMap, HashSet};

use futures::stream::{self, StreamExt};
use sea_orm::prelude::Uuid;
use wf_db::tables::{
    github_pat_connections, jira_pat_connections, notes, reminders, slack_messages, ticket_links,
};
use wf_github::{
    enrich_pull_requests, fetch_branch_prompts, fetch_workflows, list_pulls_page, GithubCheckState,
    GithubClient, GithubPullRef, GithubRepoBranches, GithubRepoWorkflows, GithubWorkflowSummary,
    PolledPullRequest,
};
use wf_jira::JiraIssueSummary;

use crate::error::AppError;
use crate::github::activity::require_pat;
use crate::github::token_cache::CachedPat;
use crate::hub::cache;
use crate::hub::types::{
    HubBoard, HubBoardContext, HubColumn, HubFavoriteWorkflow, HubLinkSuggestion, HubOrphanBranch,
    HubOrphanPr, HubOrphanTicket, HubOrphans, HubPrRef, HubRuns, HubTicketCard,
};
use crate::jira::routes::{load_board_mapping, JiraBoardMapping};
use crate::state::AppState;

const MAX_REPOS: usize = 8;
const MAX_ENRICH: usize = 12;
const JQL_WINDOW: &str = "-14d";

/// All inputs the board is assembled from, fetched once up front.
struct BoardInputs {
    pat: CachedPat,
    repos: Vec<String>,
    mapping: JiraBoardMapping,
    projects: Vec<String>,
    issues: Vec<JiraIssueSummary>,
    pulls: Vec<(String, PolledPullRequest)>,
    branch_prompts: Vec<GithubRepoBranches>,
    workflow_repos: Vec<GithubRepoWorkflows>,
    badges: Badges,
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

/// Stale-while-revalidate + single-flight, same shape as `hub::runs`: the board
/// fans out to GitHub *and* Jira, so a cold miss is the most expensive hub call —
/// serving stale + refreshing in the background keeps polls off the critical path.
pub async fn board(state: &AppState, user_id: Uuid) -> Result<HubBoard, AppError> {
    match cache::get_board_swr(user_id) {
        cache::Freshness::Fresh(v) => return Ok(v),
        cache::Freshness::Stale(v) => {
            spawn_refresh(state, user_id);
            return Ok(v);
        }
        cache::Freshness::Missing => {}
    }
    let lock = cache::refresh_lock("board", user_id);
    let _guard = lock.lock().await;
    if let Some(cached) = cache::get_board(user_id) {
        return Ok(cached);
    }
    let board = compute_board(state, user_id).await?;
    cache::put_board(user_id, &board);
    Ok(board)
}

/// Background revalidation, guarded so only one refresh per user runs at a time.
fn spawn_refresh(state: &AppState, user_id: Uuid) {
    let lock = cache::refresh_lock("board", user_id);
    let Ok(guard) = lock.try_lock_owned() else { return };
    let state = state.clone();
    // actix's runtime spawn (no `Send` bound): the board's enrich path is !Send,
    // and actix workers are current-thread, so this stays on the worker arbiter.
    actix_web::rt::spawn(async move {
        let _guard = guard;
        if let Ok(board) = compute_board(&state, user_id).await {
            cache::put_board(user_id, &board);
        }
    });
}

async fn compute_board(state: &AppState, user_id: Uuid) -> Result<HubBoard, AppError> {
    let inputs = gather_inputs(state, user_id).await?;
    let mut matches = match_repo_activity(&inputs);
    apply_check_states(&inputs.pat.token, &mut matches.ticket_prs).await;
    assemble_board(state, user_id, &inputs, matches).await
}

// ---- Inputs --------------------------------------------------------------

async fn gather_inputs(state: &AppState, user_id: Uuid) -> Result<BoardInputs, AppError> {
    let pat = require_pat(state, user_id).await?;
    let repos: Vec<String> = pat.selected_repos.iter().take(MAX_REPOS).cloned().collect();
    let mapping = load_board_mapping(state, user_id).await?;
    let projects = selected_projects(state, user_id).await?;
    if projects.is_empty() {
        return Err(AppError::validation("No Jira projects selected."));
    }
    let issues = fetch_issues(state, user_id, &projects).await?;
    let pulls = fetch_pulls(&pat.token, &repos).await;
    let branch_prompts = fetch_branch_prompts(&pat.token, &pat.login, &repos).await;
    let workflow_repos = fetch_workflows(&pat.token, &repos).await;
    let badges = fetch_badges(state, user_id).await?;
    Ok(BoardInputs {
        pat,
        repos,
        mapping,
        projects,
        issues,
        pulls,
        branch_prompts,
        workflow_repos,
        badges,
    })
}

async fn fetch_issues(
    state: &AppState,
    user_id: Uuid,
    projects: &[String],
) -> Result<Vec<JiraIssueSummary>, AppError> {
    let jql = format!(
        "project in ({}) AND updated >= {JQL_WINDOW} ORDER BY updated DESC",
        projects.join(",")
    );
    Ok(crate::jira::data::search(state, user_id, &jql, None).await?.issues)
}

async fn fetch_pulls(token: &str, repos: &[String]) -> Vec<(String, PolledPullRequest)> {
    let client = GithubClient::new(token);
    stream::iter(repos.to_vec())
        .map(|repo| {
            let client = &client;
            async move {
                let Some((owner, name)) = repo.split_once('/') else { return Vec::new() };
                let pulls = list_pulls_page(client, owner, name).await.unwrap_or_default();
                pulls.into_iter().map(|p| (repo.clone(), p)).collect::<Vec<_>>()
            }
        })
        .buffered(MAX_REPOS)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flatten()
        .collect()
}

async fn fetch_badges(state: &AppState, user_id: Uuid) -> Result<Badges, AppError> {
    let favorites = github_pat_connections::get_favorites(&state.db, user_id).await?;
    let manual = ticket_links::list(&state.db, user_id).await?;
    let slack_unread = slack_messages::unread_counts(&state.db, user_id).await?;
    let note_keys: HashSet<String> =
        notes::keys_with_notes(&state.db, user_id).await?.into_iter().collect();
    let reminders_due = reminders::due_counts(&state.db, user_id).await?;
    Ok(Badges { favorites, manual, slack_unread, note_keys, reminders_due })
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

// ---- Check states (bounded enrichment of matched open PRs) ---------------

async fn apply_check_states(token: &str, ticket_prs: &mut HashMap<String, Vec<HubPrRef>>) {
    let refs = enrich_refs(ticket_prs);
    let enriched = enrich_pull_requests(token, &refs).await;
    let check_by_pr: HashMap<(String, i64), String> = enriched
        .iter()
        .map(|e| {
            ((format!("{}/{}", e.owner, e.repo), e.number), aggregate_checks(&e.enrichment.required_checks))
        })
        .collect();
    for prs in ticket_prs.values_mut() {
        for pr in prs.iter_mut() {
            if let Some(check) = check_by_pr.get(&(pr.repo.clone(), pr.number)) {
                pr.check_state.clone_from(check);
            }
        }
    }
}

fn enrich_refs(ticket_prs: &HashMap<String, Vec<HubPrRef>>) -> Vec<GithubPullRef> {
    ticket_prs
        .values()
        .flatten()
        .filter(|pr| pr.state == "open" || pr.state == "draft")
        .take(MAX_ENRICH)
        .filter_map(|pr| {
            let (owner, name) = pr.repo.split_once('/')?;
            Some(GithubPullRef { owner: owner.to_string(), repo: name.to_string(), number: pr.number })
        })
        .collect()
}

// ---- Cards + columns -----------------------------------------------------

/// Read-only context threaded through per-issue card construction.
struct CardContext<'a> {
    inputs: &'a BoardInputs,
    runs: &'a Option<HubRuns>,
    workflows_by_repo: &'a HashMap<&'a str, &'a [GithubWorkflowSummary]>,
}

async fn assemble_board(
    state: &AppState,
    user_id: Uuid,
    inputs: &BoardInputs,
    mut matches: BoardMatches,
) -> Result<HubBoard, AppError> {
    let runs = crate::hub::runs::runs(state, user_id).await.ok();
    let workflows_by_repo = index_workflows(&inputs.workflow_repos);
    let ctx = CardContext { inputs, runs: &runs, workflows_by_repo: &workflows_by_repo };
    let mut columns = init_columns(&inputs.mapping);
    let mut orphan_tickets: Vec<HubOrphanTicket> = Vec::new();
    for issue in &inputs.issues {
        place_issue(&mut columns, &mut orphan_tickets, &mut matches, issue, &ctx);
    }
    let summaries: Vec<(&str, &str)> =
        inputs.issues.iter().map(|i| (i.key.as_str(), i.summary.as_str())).collect();
    apply_suggestions(&mut matches, &summaries);
    Ok(build_board(inputs, columns, orphan_tickets, matches))
}

fn index_workflows(workflow_repos: &[GithubRepoWorkflows]) -> HashMap<&str, &[GithubWorkflowSummary]> {
    workflow_repos
        .iter()
        .map(|w| (w.repo.as_str(), w.workflows.as_slice()))
        .collect()
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
    let summary = ctx.workflows_by_repo.get(repo.as_str())?.iter().find(|w| w.id == id)?;
    let id_s = id.to_string();
    let last_run = ctx
        .runs
        .as_ref()
        .and_then(|r| r.runs.iter().find(|p| p.repo == repo && p.workflow_id == id_s))
        .map(|p| p.started_at.clone());
    Some(HubFavoriteWorkflow {
        repo,
        workflow_id: id_s,
        workflow_name: summary.name.clone(),
        path: summary.path.clone(),
        last_run,
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
            generated_at: chrono::Utc::now().to_rfc3339(),
        },
    }
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
