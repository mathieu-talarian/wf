//! GitHub dashboard data layer (port of `dashboard/graphql.ts`).
//!
//! One GraphQL request returns counts for every queue plus first-page nodes for
//! the *active* queue only (fetching nodes for all five ~doubles search cost);
//! inactive tabs load their PRs on demand via `fetch_queue_pulls`.

pub mod query;
pub mod types;

use std::collections::HashMap;

use reqwest::Method;
use serde::Deserialize;

use crate::client::GithubClient;
use crate::errors::GithubError;
use types::GithubRepoOption;
use query::{build_query, build_specs, chunk_repos, queue_bases, scoped_query, variables_of, Spec, ALIAS_BATCH};
use types::{
    DashboardData, GithubDashboardActor, GithubDashboardLabel, GithubDashboardRepository,
    GithubPullRequestBasic, GithubPullRequestQueue, GithubQueueCount, GithubQueueKey,
};

#[derive(Deserialize)]
struct GqlActor {
    login: String,
    #[serde(rename = "avatarUrl")]
    avatar_url: String,
    url: String,
}

#[derive(Deserialize)]
struct GqlRepo {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
    url: String,
    #[serde(rename = "isPrivate")]
    is_private: bool,
    #[serde(rename = "isArchived")]
    is_archived: bool,
    #[serde(rename = "defaultBranchRef")]
    default_branch_ref: Option<GqlRef>,
}
#[derive(Deserialize)]
struct GqlRef {
    name: String,
}
#[derive(Deserialize)]
struct GqlCountField {
    #[serde(rename = "totalCount")]
    total_count: i64,
}
#[derive(Deserialize)]
struct GqlNodes<T> {
    nodes: Vec<T>,
}
#[derive(Deserialize)]
struct GqlLabel {
    name: String,
    color: String,
}

#[derive(Deserialize)]
struct GqlPrNode {
    number: i64,
    title: String,
    url: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    comments: GqlCountField,
    author: Option<GqlActor>,
    repository: GqlRepo,
    assignees: GqlNodes<GqlActor>,
    labels: GqlNodes<GqlLabel>,
}

#[derive(Deserialize)]
struct GqlCount {
    #[serde(rename = "issueCount")]
    issue_count: i64,
}
#[derive(Deserialize)]
struct GqlPulls {
    #[serde(rename = "issueCount")]
    issue_count: i64,
    nodes: Vec<Option<GqlPrNode>>,
}

struct AliasResult {
    key: GithubQueueKey,
    issue_count: i64,
    nodes: Vec<GqlPrNode>,
}

fn decode_alias(key: GithubQueueKey, raw: serde_json::Value, with_nodes: bool) -> Result<AliasResult, GithubError> {
    if !with_nodes {
        let c: GqlCount = serde_json::from_value(raw).map_err(|e| GithubError::Api(e.to_string()))?;
        return Ok(AliasResult { key, issue_count: c.issue_count, nodes: vec![] });
    }
    let p: GqlPulls = serde_json::from_value(raw).map_err(|e| GithubError::Api(e.to_string()))?;
    Ok(AliasResult {
        key,
        issue_count: p.issue_count,
        nodes: p.nodes.into_iter().flatten().collect(),
    })
}

async fn run_batch(client: &GithubClient, specs: &[Spec]) -> Result<Vec<AliasResult>, GithubError> {
    let mut data = client.graphql(&build_query(specs), variables_of(specs)).await?;
    let mut out = Vec::with_capacity(specs.len());
    for (i, spec) in specs.iter().enumerate() {
        let raw = data
            .get_mut(format!("s{i}"))
            .map(serde_json::Value::take)
            .unwrap_or(serde_json::Value::Null);
        out.push(decode_alias(spec.key, raw, spec.with_nodes)?);
    }
    Ok(out)
}

async fn run_all(token: &str, specs: Vec<Spec>) -> Result<Vec<AliasResult>, GithubError> {
    let client = GithubClient::new(token);
    let mut results = Vec::new();
    // Batches of <=ALIAS_BATCH aliases (run sequentially; each is one /graphql
    // request, not subject to the /search throttle).
    for group in specs.chunks(ALIAS_BATCH) {
        results.extend(run_batch(&client, group).await?);
    }
    Ok(results)
}

const GHOST: &str = "ghost";

fn to_actor(actor: Option<GqlActor>) -> GithubDashboardActor {
    match actor {
        None => GithubDashboardActor { login: GHOST.to_string(), avatar_url: String::new(), url: String::new() },
        Some(a) => GithubDashboardActor { login: a.login, avatar_url: a.avatar_url, url: a.url },
    }
}

fn to_repository(repo: GqlRepo) -> GithubDashboardRepository {
    let actions_url = format!("{}/actions", repo.url);
    GithubDashboardRepository {
        full_name: repo.name_with_owner,
        url: repo.url,
        actions_url,
        is_private: repo.is_private,
        is_archived: repo.is_archived,
        default_branch: repo.default_branch_ref.map(|r| r.name).unwrap_or_default(),
    }
}

fn to_basic(node: GqlPrNode) -> GithubPullRequestBasic {
    GithubPullRequestBasic {
        repository: to_repository(node.repository),
        number: node.number,
        title: node.title,
        url: node.url,
        author: to_actor(node.author),
        assignees: node.assignees.nodes.into_iter().map(|a| to_actor(Some(a))).collect(),
        labels: node.labels.nodes.into_iter().map(|l| GithubDashboardLabel { name: l.name, color: l.color }).collect(),
        comments: node.comments.total_count,
        created_at: node.created_at,
        updated_at: node.updated_at,
    }
}

/// Dedup by PR url, preserving first-seen order with last-seen value (matches
/// the TS `Map` semantics).
fn dedup(nodes: Vec<GqlPrNode>) -> Vec<GithubPullRequestBasic> {
    let mut order: Vec<String> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut values: Vec<GithubPullRequestBasic> = Vec::new();
    for node in nodes {
        let url = node.url.clone();
        let basic = to_basic(node);
        if let Some(&i) = index.get(&url) {
            values[i] = basic;
        } else {
            index.insert(url.clone(), values.len());
            order.push(url);
            values.push(basic);
        }
    }
    let _ = order;
    values
}

fn sum_for(results: &[AliasResult], key: GithubQueueKey) -> i64 {
    results.iter().filter(|r| r.key == key).map(|r| r.issue_count).sum()
}

fn to_count(results: &[AliasResult], key: GithubQueueKey, label: &str) -> GithubQueueCount {
    GithubQueueCount {
        key,
        label: label.to_string(),
        total_count: sum_for(results, key),
        incomplete_results: false,
    }
}

fn to_queue(results: Vec<AliasResult>, key: GithubQueueKey, label: &str) -> GithubPullRequestQueue {
    let total_count = sum_for(&results, key);
    let nodes: Vec<GqlPrNode> = results.into_iter().filter(|r| r.key == key).flat_map(|r| r.nodes).collect();
    GithubPullRequestQueue {
        key,
        label: label.to_string(),
        total_count,
        incomplete_results: false,
        pull_requests: dedup(nodes),
    }
}

/// Counts for every queue + first-page nodes for `active_key`.
pub async fn fetch_dashboard(
    token: &str,
    login: &str,
    repos: &[String],
    active_key: GithubQueueKey,
) -> Result<DashboardData, GithubError> {
    let specs = build_specs(login, repos, |k| k == active_key);
    let results = run_all(token, specs).await?;
    let bases = queue_bases(login);
    let queues = bases.iter().map(|b| to_count(&results, b.key, b.label)).collect();
    let queue_pulls = match bases.iter().find(|b| b.key == active_key) {
        Some(b) => vec![to_queue(results, b.key, b.label)],
        None => vec![],
    };
    Ok(DashboardData { queues, queue_pulls })
}

/// A single queue's PRs (port of `fetchQueuePulls`).
pub async fn fetch_queue_pulls(
    token: &str,
    login: &str,
    repos: &[String],
    key: GithubQueueKey,
) -> Result<GithubPullRequestQueue, GithubError> {
    let bases = queue_bases(login);
    let Some(base) = bases.iter().find(|b| b.key == key) else {
        return Ok(GithubPullRequestQueue {
            key,
            label: format!("{key:?}"),
            total_count: 0,
            incomplete_results: false,
            pull_requests: vec![],
        });
    };
    let specs: Vec<Spec> = chunk_repos(repos)
        .into_iter()
        .map(|chunk| Spec { key, query: scoped_query(&base.base, &chunk), with_nodes: true })
        .collect();
    let results = run_all(token, specs).await?;
    Ok(to_queue(results, key, base.label))
}

#[derive(Deserialize)]
struct ApiOwnedRepo {
    full_name: String,
    private: bool,
    archived: bool,
}

/// Repos the user can select (port of `client.ts#listRepositories` /
/// `api.ts#listOwnedRepos`): first 3 pages of `GET /user/repos`, 100/page,
/// sorted by pushed, across owned/collaborator/org-member affiliations.
pub async fn list_repositories(token: &str) -> Result<Vec<GithubRepoOption>, GithubError> {
    let client = GithubClient::new(token);
    let mut out = Vec::new();
    for page in ["1", "2", "3"] {
        let resp = client
            .request(Method::GET, "/user/repos")
            .query(&[
                ("per_page", "100"),
                ("sort", "pushed"),
                ("page", page),
                ("affiliation", "owner,collaborator,organization_member"),
            ])
            .send()
            .await
            .map_err(|e| GithubError::Api(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(GithubError::Api(format!("repos HTTP {}", resp.status().as_u16())));
        }
        let repos: Vec<ApiOwnedRepo> = resp.json().await.map_err(|e| GithubError::Api(e.to_string()))?;
        out.extend(repos.into_iter().map(|r| GithubRepoOption {
            full_name: r.full_name,
            is_private: r.private,
            is_archived: r.archived,
        }));
    }
    Ok(out)
}
