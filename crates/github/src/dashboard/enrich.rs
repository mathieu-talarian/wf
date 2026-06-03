//! Per-PR detail enrichment (port of `dashboard/{enrich,api,checks,readiness}.ts`).
//!
//! The dashboard query returns only search-derived fields; the heavy detail
//! (reviews, checks, mergeability, readiness) is fetched lazily per PR here via
//! REST. Every upstream call is best-effort: any failure degrades to a null /
//! empty default rather than failing the whole enrichment.

use std::collections::{HashMap, HashSet};

use futures::stream::{self, StreamExt};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::client::{GithubClient, RepoRef};
use crate::dashboard::types::{
    GithubApprovalState, GithubCheckState, GithubDashboardRepository, GithubPullEnrichmentResult,
    GithubPullRef, GithubPullRequestEnrichment, GithubRequestedReviewer,
    GithubRequestedReviewerKind, GithubRequiredCheck, GithubWorkflowRunSummary,
};

/// Encodes a single URL path segment, matching JS `encodeURIComponent` (leaves
/// the unreserved set `-_.!~*'()` intact, percent-encodes everything else
/// including `/`). Branch names may contain slashes, so segments must be encoded.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

fn enc(s: &str) -> String {
    utf8_percent_encode(s, COMPONENT).to_string()
}

// ---------------------------------------------------------------------------
// Upstream API shapes (port of the `effect/Schema` structs in api.ts/checks.ts).
// Deserialize is lenient: unknown fields are ignored, missing/null arrays default
// to empty, and `draft` defaults to false.
// ---------------------------------------------------------------------------

fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Deserialize)]
struct ApiActor {
    login: String,
    avatar_url: String,
    html_url: String,
}

#[derive(Deserialize)]
struct ApiTeam {
    name: String,
    slug: String,
    #[serde(default, deserialize_with = "null_default")]
    avatar_url: Option<String>,
    html_url: String,
}

#[derive(Deserialize)]
struct ApiRepository {
    full_name: String,
    html_url: String,
    private: bool,
    archived: bool,
    default_branch: String,
}

#[derive(Deserialize)]
struct ApiPullHead {
    #[serde(rename = "ref")]
    ref_: String,
    sha: String,
}

#[derive(Deserialize)]
struct ApiPullBase {
    #[serde(rename = "ref")]
    ref_: String,
    repo: ApiRepository,
}

#[derive(Deserialize)]
struct ApiPullDetail {
    #[serde(default)]
    draft: bool,
    mergeable: Option<bool>,
    mergeable_state: String,
    head: ApiPullHead,
    base: ApiPullBase,
    #[serde(default, deserialize_with = "null_default")]
    requested_reviewers: Vec<ApiActor>,
    #[serde(default, deserialize_with = "null_default")]
    requested_teams: Vec<ApiTeam>,
    review_comments: i64,
    additions: i64,
    deletions: i64,
    changed_files: i64,
}

#[derive(Deserialize)]
struct ApiReview {
    user: Option<ApiActor>,
    state: String,
}

#[derive(Deserialize)]
struct ApiWorkflowRun {
    id: i64,
    name: Option<String>,
    status: String,
    conclusion: Option<String>,
    html_url: String,
    run_number: i64,
    event: String,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct ApiWorkflowRuns {
    workflow_runs: Vec<ApiWorkflowRun>,
}

#[derive(Deserialize)]
struct ApiStatus {
    context: String,
    state: String,
    target_url: Option<String>,
}

#[derive(Deserialize)]
struct ApiCombinedStatus {
    statuses: Vec<ApiStatus>,
}

#[derive(Deserialize)]
struct ApiCheckRun {
    name: String,
    status: String,
    conclusion: Option<String>,
    details_url: Option<String>,
    html_url: Option<String>,
}

#[derive(Deserialize)]
struct ApiCheckRuns {
    check_runs: Vec<ApiCheckRun>,
}

#[derive(Deserialize)]
struct ApiRequiredStatusChecks {
    contexts: Vec<String>,
}

// ---------------------------------------------------------------------------
// Best-effort REST fetches (port of api.ts + checks.ts; each swallows errors).
// ---------------------------------------------------------------------------

/// GETs `path` with `query`, returning `None` on transport error, non-2xx, or a
/// decode failure (mirrors the TS `try/catch` → null/empty pattern).
async fn get_json<T: DeserializeOwned>(
    client: &GithubClient,
    path: &str,
    query: &[(&str, &str)],
) -> Option<T> {
    let resp = client.request(Method::GET, path).query(query).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<T>().await.ok()
}

async fn fetch_detail(client: &GithubClient, r: &RepoRef, number: i64) -> Option<ApiPullDetail> {
    let path = format!("/repos/{}/{}/pulls/{number}", enc(&r.owner), enc(&r.repo));
    get_json(client, &path, &[]).await
}

async fn fetch_reviews(client: &GithubClient, r: &RepoRef, number: i64) -> Vec<ApiReview> {
    let path = format!("/repos/{}/{}/pulls/{number}/reviews", enc(&r.owner), enc(&r.repo));
    get_json::<Vec<ApiReview>>(client, &path, &[("per_page", "100")]).await.unwrap_or_default()
}

async fn fetch_latest_run(
    client: &GithubClient,
    r: &RepoRef,
    head_sha: &str,
) -> Option<GithubWorkflowRunSummary> {
    let path = format!("/repos/{}/{}/actions/runs", enc(&r.owner), enc(&r.repo));
    let runs: ApiWorkflowRuns =
        get_json(client, &path, &[("head_sha", head_sha), ("per_page", "1")]).await?;
    runs.workflow_runs.into_iter().next().map(to_run)
}

fn to_run(run: ApiWorkflowRun) -> GithubWorkflowRunSummary {
    GithubWorkflowRunSummary {
        id: run.id,
        name: run.name,
        status: run.status,
        conclusion: run.conclusion,
        url: run.html_url,
        run_number: run.run_number,
        event: run.event,
        created_at: run.created_at,
        updated_at: run.updated_at,
    }
}

async fn fetch_required_contexts(client: &GithubClient, r: &RepoRef, branch: &str) -> Vec<String> {
    let path = format!(
        "/repos/{}/{}/branches/{}/protection/required_status_checks",
        enc(&r.owner),
        enc(&r.repo),
        enc(branch)
    );
    get_json::<ApiRequiredStatusChecks>(client, &path, &[])
        .await
        .map(|c| c.contexts)
        .unwrap_or_default()
}

async fn fetch_statuses(client: &GithubClient, r: &RepoRef, sha: &str) -> Vec<ApiStatus> {
    let path = format!("/repos/{}/{}/commits/{}/status", enc(&r.owner), enc(&r.repo), enc(sha));
    get_json::<ApiCombinedStatus>(client, &path, &[]).await.map(|s| s.statuses).unwrap_or_default()
}

async fn fetch_check_runs(client: &GithubClient, r: &RepoRef, sha: &str) -> Vec<ApiCheckRun> {
    let path = format!("/repos/{}/{}/commits/{}/check-runs", enc(&r.owner), enc(&r.repo), enc(sha));
    get_json::<ApiCheckRuns>(client, &path, &[("per_page", "50")])
        .await
        .map(|c| c.check_runs)
        .unwrap_or_default()
}

async fn fetch_required_checks(
    client: &GithubClient,
    r: &RepoRef,
    detail: &ApiPullDetail,
) -> Vec<GithubRequiredCheck> {
    let (contexts, statuses, runs) = futures::join!(
        fetch_required_contexts(client, r, &detail.base.ref_),
        fetch_statuses(client, r, &detail.head.sha),
        fetch_check_runs(client, r, &detail.head.sha),
    );
    let mut checks: Vec<GithubRequiredCheck> = statuses.iter().map(to_status_check).collect();
    checks.extend(runs.iter().map(to_run_check));
    select_checks(&contexts, checks)
}

// ---------------------------------------------------------------------------
// Check normalization + selection (port of checks.ts).
// ---------------------------------------------------------------------------

fn normalize_check_state(state: &str) -> GithubCheckState {
    use GithubCheckState::*;
    match state {
        "success" => Success,
        "pending" | "queued" | "in_progress" => Pending,
        "error" | "startup_failure" => Failure,
        "failure" => Failure,
        "cancelled" => Cancelled,
        "timed_out" => TimedOut,
        "action_required" => ActionRequired,
        "skipped" => Skipped,
        "neutral" => Neutral,
        _ => Unknown,
    }
}

fn to_status_check(status: &ApiStatus) -> GithubRequiredCheck {
    GithubRequiredCheck {
        name: status.context.clone(),
        state: normalize_check_state(&status.state),
        url: status.target_url.clone(),
        required: false,
    }
}

fn to_run_check(run: &ApiCheckRun) -> GithubRequiredCheck {
    let raw = if run.status == "completed" {
        run.conclusion.as_deref().unwrap_or("unknown")
    } else {
        run.status.as_str()
    };
    GithubRequiredCheck {
        name: run.name.clone(),
        state: normalize_check_state(raw),
        url: run.details_url.clone().or_else(|| run.html_url.clone()),
        required: false,
    }
}

fn missing_checks(
    contexts: &[String],
    selected: &[GithubRequiredCheck],
) -> Vec<GithubRequiredCheck> {
    contexts
        .iter()
        .filter(|name| !selected.iter().any(|c| &c.name == *name))
        .map(|name| GithubRequiredCheck {
            name: name.clone(),
            state: GithubCheckState::Pending,
            url: None,
            required: true,
        })
        .collect()
}

fn select_checks(
    contexts: &[String],
    checks: Vec<GithubRequiredCheck>,
) -> Vec<GithubRequiredCheck> {
    if contexts.is_empty() {
        return checks.into_iter().take(12).collect();
    }
    let required: HashSet<&str> = contexts.iter().map(String::as_str).collect();
    let selected: Vec<GithubRequiredCheck> =
        checks.into_iter().filter(|c| required.contains(c.name.as_str())).collect();
    let missing = missing_checks(contexts, &selected);
    selected
        .into_iter()
        .chain(missing)
        .map(|mut c| {
            c.required = required.contains(c.name.as_str());
            c
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Readiness derivation (port of readiness.ts).
// ---------------------------------------------------------------------------

fn to_user_reviewer(actor: &ApiActor) -> GithubRequestedReviewer {
    GithubRequestedReviewer {
        name: actor.login.clone(),
        avatar_url: Some(actor.avatar_url.clone()),
        url: actor.html_url.clone(),
        kind: GithubRequestedReviewerKind::User,
    }
}

fn to_team_reviewer(team: &ApiTeam) -> GithubRequestedReviewer {
    let name = if team.name.is_empty() { team.slug.clone() } else { team.name.clone() };
    GithubRequestedReviewer {
        name,
        avatar_url: team.avatar_url.clone(),
        url: team.html_url.clone(),
        kind: GithubRequestedReviewerKind::Team,
    }
}

fn requested_reviewers(detail: Option<&ApiPullDetail>) -> Vec<GithubRequestedReviewer> {
    match detail {
        None => vec![],
        Some(d) => d
            .requested_reviewers
            .iter()
            .map(to_user_reviewer)
            .chain(d.requested_teams.iter().map(to_team_reviewer))
            .collect(),
    }
}

fn approval_state(reviews: &[ApiReview], reviewer_count: usize) -> GithubApprovalState {
    const DECISIVE: [&str; 3] = ["APPROVED", "CHANGES_REQUESTED", "DISMISSED"];
    let mut latest: HashMap<&str, &str> = HashMap::new();
    for review in reviews {
        if let Some(user) = &review.user {
            if DECISIVE.contains(&review.state.as_str()) {
                latest.insert(user.login.as_str(), review.state.as_str());
            }
        }
    }
    let states: HashSet<&str> = latest.values().copied().collect();
    if states.contains("CHANGES_REQUESTED") {
        GithubApprovalState::ChangesRequested
    } else if states.contains("APPROVED") {
        GithubApprovalState::Approved
    } else if reviewer_count > 0 {
        GithubApprovalState::ReviewRequired
    } else {
        GithubApprovalState::Unknown
    }
}

fn check_blockers(checks: &[GithubRequiredCheck]) -> Vec<String> {
    use GithubCheckState::*;
    let failing = checks
        .iter()
        .find(|c| matches!(c.state, Failure | Cancelled | TimedOut | ActionRequired));
    let pending = checks.iter().find(|c| c.required && c.state == Pending);
    let mut out = Vec::new();
    if let Some(f) = failing {
        out.push(format!("Failing check: {}", f.name));
    }
    if let Some(p) = pending {
        out.push(format!("Pending required check: {}", p.name));
    }
    out
}

fn blocker_summary(
    detail: Option<&ApiPullDetail>,
    state: GithubApprovalState,
    checks: &[GithubRequiredCheck],
) -> Vec<String> {
    let mut blockers = Vec::new();
    if detail.map(|d| d.draft) == Some(true) {
        blockers.push("Draft PR".to_string());
    }
    if state == GithubApprovalState::ChangesRequested {
        blockers.push("Changes requested".to_string());
    }
    if state == GithubApprovalState::ReviewRequired {
        blockers.push("Review still requested".to_string());
    }
    if detail.map(|d| d.mergeable_state.as_str()) == Some("behind") {
        blockers.push("Branch is behind".to_string());
    }
    if detail.map(|d| d.mergeable) == Some(Some(false)) {
        blockers.push("Merge conflicts detected".to_string());
    }
    blockers.extend(check_blockers(checks));
    blockers
}

fn readiness_badges(
    detail: Option<&ApiPullDetail>,
    state: GithubApprovalState,
    blockers: &[String],
) -> Vec<String> {
    let mut badges = Vec::new();
    if detail.map(|d| d.draft) == Some(true) {
        badges.push("draft".to_string());
    }
    if state == GithubApprovalState::Approved {
        badges.push("approved".to_string());
    }
    if state == GithubApprovalState::ChangesRequested {
        badges.push("changes requested".to_string());
    }
    if detail.map(|d| d.mergeable_state.as_str()) == Some("behind") {
        badges.push("behind".to_string());
    }
    if !blockers.is_empty() {
        badges.push("blocked".to_string());
    }
    if blockers.is_empty() && detail.is_some() {
        badges.push("ready".to_string());
    }
    badges
}

/// The eight readiness fields shared between the degraded and detailed builds
/// (port of the object spread returned by `toReadiness`).
struct Readiness {
    approval_state: GithubApprovalState,
    requested_reviewers: Vec<GithubRequestedReviewer>,
    mergeable: Option<bool>,
    mergeable_state: Option<String>,
    branch_behind: Option<bool>,
    required_checks: Vec<GithubRequiredCheck>,
    readiness_badges: Vec<String>,
    blocker_summary: Vec<String>,
}

fn to_readiness(
    detail: Option<&ApiPullDetail>,
    reviews: &[ApiReview],
    required_checks: Vec<GithubRequiredCheck>,
) -> Readiness {
    let reviewers = requested_reviewers(detail);
    let state = approval_state(reviews, reviewers.len());
    let blockers = blocker_summary(detail, state, &required_checks);
    let badges = readiness_badges(detail, state, &blockers);
    Readiness {
        approval_state: state,
        requested_reviewers: reviewers,
        mergeable: detail.and_then(|d| d.mergeable),
        mergeable_state: detail.map(|d| d.mergeable_state.clone()),
        branch_behind: detail.map(|d| d.mergeable_state == "behind"),
        required_checks,
        readiness_badges: badges,
        blocker_summary: blockers,
    }
}

// ---------------------------------------------------------------------------
// Repository mappers (port of client.ts `toRepository` / `repoFromFullName`).
// ---------------------------------------------------------------------------

fn to_repository(repo: &ApiRepository) -> GithubDashboardRepository {
    GithubDashboardRepository {
        full_name: repo.full_name.clone(),
        url: repo.html_url.clone(),
        actions_url: format!("{}/actions", repo.html_url),
        is_private: repo.private,
        is_archived: repo.archived,
        default_branch: repo.default_branch.clone(),
    }
}

fn repo_from_full_name(full_name: &str) -> GithubDashboardRepository {
    let url = format!("https://github.com/{full_name}");
    let actions_url = format!("{url}/actions");
    GithubDashboardRepository {
        full_name: full_name.to_string(),
        url,
        actions_url,
        is_private: false,
        is_archived: false,
        default_branch: String::new(),
    }
}

// ---------------------------------------------------------------------------
// Enrichment assembly (port of enrich.ts).
// ---------------------------------------------------------------------------

fn degraded_enrichment(r: &RepoRef) -> GithubPullRequestEnrichment {
    let readiness = to_readiness(None, &[], vec![]);
    GithubPullRequestEnrichment {
        repository: repo_from_full_name(&format!("{}/{}", r.owner, r.repo)),
        review_comments: None,
        additions: None,
        deletions: None,
        changed_files: None,
        draft: None,
        head_ref: None,
        base_ref: None,
        head_sha: None,
        latest_run: None,
        approval_state: readiness.approval_state,
        requested_reviewers: readiness.requested_reviewers,
        mergeable: readiness.mergeable,
        mergeable_state: readiness.mergeable_state,
        branch_behind: readiness.branch_behind,
        required_checks: readiness.required_checks,
        readiness_badges: readiness.readiness_badges,
        blocker_summary: readiness.blocker_summary,
    }
}

fn detail_enrichment(
    detail: ApiPullDetail,
    latest_run: Option<GithubWorkflowRunSummary>,
    reviews: Vec<ApiReview>,
    required_checks: Vec<GithubRequiredCheck>,
) -> GithubPullRequestEnrichment {
    let repository = to_repository(&detail.base.repo);
    let readiness = to_readiness(Some(&detail), &reviews, required_checks);
    GithubPullRequestEnrichment {
        repository,
        review_comments: Some(detail.review_comments),
        additions: Some(detail.additions),
        deletions: Some(detail.deletions),
        changed_files: Some(detail.changed_files),
        draft: Some(detail.draft),
        head_ref: Some(detail.head.ref_.clone()),
        base_ref: Some(detail.base.ref_.clone()),
        head_sha: Some(detail.head.sha.clone()),
        latest_run,
        approval_state: readiness.approval_state,
        requested_reviewers: readiness.requested_reviewers,
        mergeable: readiness.mergeable,
        mergeable_state: readiness.mergeable_state,
        branch_behind: readiness.branch_behind,
        required_checks: readiness.required_checks,
        readiness_badges: readiness.readiness_badges,
        blocker_summary: readiness.blocker_summary,
    }
}

async fn enrich_with(
    client: &GithubClient,
    r: &RepoRef,
    number: i64,
) -> GithubPullRequestEnrichment {
    // Reviews ride alongside the detail fetch (need only ref + number).
    let (detail, reviews) = futures::join!(fetch_detail(client, r, number), fetch_reviews(client, r, number));
    let Some(detail) = detail else {
        return degraded_enrichment(r);
    };
    // run + checks gate on the detail (head sha, base ref).
    let (latest_run, required_checks) = futures::join!(
        fetch_latest_run(client, r, &detail.head.sha),
        fetch_required_checks(client, r, &detail),
    );
    detail_enrichment(detail, latest_run, reviews, required_checks)
}

/// Enrich a single PR (port of `enrichPullRequest`).
pub async fn enrich_pull_request(
    token: &str,
    r: &RepoRef,
    number: i64,
) -> GithubPullRequestEnrichment {
    let client = GithubClient::new(token);
    enrich_with(&client, r, number).await
}

/// Bounded concurrency so a wide viewport doesn't trip GitHub's secondary rate
/// limits; one shared client for the whole batch (port of `enrichPullRequests`).
const POOL: usize = 8;

/// Enrich a batch of PRs, preserving input order (port of `enrichPullRequests`).
pub async fn enrich_pull_requests(
    token: &str,
    refs: &[GithubPullRef],
) -> Vec<GithubPullEnrichmentResult> {
    let client = GithubClient::new(token);
    stream::iter(refs)
        .map(|pull| {
            let client = &client;
            async move {
                let r = RepoRef { owner: pull.owner.clone(), repo: pull.repo.clone() };
                let enrichment = enrich_with(client, &r, pull.number).await;
                GithubPullEnrichmentResult {
                    owner: pull.owner.clone(),
                    repo: pull.repo.clone(),
                    number: pull.number,
                    enrichment,
                }
            }
        })
        .buffered(POOL)
        .collect()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, state: GithubCheckState, required: bool) -> GithubRequiredCheck {
        GithubRequiredCheck { name: name.to_string(), state, url: None, required }
    }

    #[test]
    fn normalize_maps_states() {
        assert_eq!(normalize_check_state("success"), GithubCheckState::Success);
        assert_eq!(normalize_check_state("queued"), GithubCheckState::Pending);
        assert_eq!(normalize_check_state("startup_failure"), GithubCheckState::Failure);
        assert_eq!(normalize_check_state("cancelled"), GithubCheckState::Cancelled);
        assert_eq!(normalize_check_state("timed_out"), GithubCheckState::TimedOut);
        assert_eq!(normalize_check_state("weird"), GithubCheckState::Unknown);
    }

    #[test]
    fn select_checks_caps_at_12_when_no_required() {
        let checks: Vec<_> =
            (0..20).map(|i| check(&format!("c{i}"), GithubCheckState::Success, false)).collect();
        let out = select_checks(&[], checks);
        assert_eq!(out.len(), 12);
        assert!(out.iter().all(|c| !c.required));
    }

    #[test]
    fn select_checks_marks_required_and_fills_missing() {
        let contexts = vec!["build".to_string(), "lint".to_string()];
        let checks = vec![check("build", GithubCheckState::Success, false)];
        let out = select_checks(&contexts, checks);
        // "build" selected + "lint" synthesized as a pending required check.
        assert_eq!(out.len(), 2);
        let build = out.iter().find(|c| c.name == "build").unwrap();
        assert!(build.required);
        let lint = out.iter().find(|c| c.name == "lint").unwrap();
        assert!(lint.required);
        assert_eq!(lint.state, GithubCheckState::Pending);
    }

    #[test]
    fn approval_state_latest_review_wins() {
        let reviews = vec![
            ApiReview {
                user: Some(ApiActor {
                    login: "a".into(),
                    avatar_url: String::new(),
                    html_url: String::new(),
                }),
                state: "APPROVED".into(),
            },
            ApiReview {
                user: Some(ApiActor {
                    login: "a".into(),
                    avatar_url: String::new(),
                    html_url: String::new(),
                }),
                state: "CHANGES_REQUESTED".into(),
            },
        ];
        assert_eq!(approval_state(&reviews, 1), GithubApprovalState::ChangesRequested);
        assert_eq!(approval_state(&[], 0), GithubApprovalState::Unknown);
        assert_eq!(approval_state(&[], 2), GithubApprovalState::ReviewRequired);
    }

    #[test]
    fn readiness_badges_ready_when_unblocked() {
        let badges = readiness_badges(None, GithubApprovalState::Unknown, &[]);
        // detail is None → never "ready".
        assert!(!badges.contains(&"ready".to_string()));
    }
}
