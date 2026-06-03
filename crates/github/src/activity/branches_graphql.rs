//! Branch→PR-prompt GraphQL: per-repo aliased query, decode, and the selection
//! filter (port of `branches-graphql.ts`). A branch "qualifies" as a prompt when
//! it is the user's own recent work that isn't the default branch and has no open
//! PR yet — i.e. a nudge to open one.

use serde::Deserialize;

use super::types::GithubBranchPrompt;

/// `owner`/`name` split out of a `full_name`, retaining the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCoord {
    pub owner: String,
    pub name: String,
    pub full_name: String,
}

pub fn to_coord(full_name: &str) -> Option<RepoCoord> {
    let mut it = full_name.splitn(2, '/');
    let owner = it.next().filter(|s| !s.is_empty())?;
    let name = it.next().filter(|s| !s.is_empty())?;
    Some(RepoCoord {
        owner: owner.to_string(),
        name: name.to_string(),
        full_name: full_name.to_string(),
    })
}

const REFS_PER_REPO: usize = 100;

fn repo_fields() -> String {
    format!(
        "nameWithOwner url\n  defaultBranchRef {{ name target {{ oid }} }}\n  refs(refPrefix: \"refs/heads/\", first: {REFS_PER_REPO}, orderBy: {{field: TAG_COMMIT_DATE, direction: DESC}}) {{\n    nodes {{\n      name\n      associatedPullRequests(states: OPEN, first: 1) {{ totalCount }}\n      target {{ ... on Commit {{\n        oid committedDate messageHeadline\n        author {{ user {{ login }} }}\n        committer {{ user {{ login }} }}\n      }} }}\n    }}\n  }}"
    )
}

pub fn build_branch_query(coords: &[RepoCoord]) -> String {
    let decls: Vec<String> =
        (0..coords.len()).map(|i| format!("$o{i}: String!, $n{i}: String!")).collect();
    let fields = repo_fields();
    let aliases: Vec<String> = (0..coords.len())
        .map(|i| format!("r{i}: repository(owner: $o{i}, name: $n{i}) {{ {fields} }}"))
        .collect();
    format!("query({}) {{\n{}\n}}", decls.join(", "), aliases.join("\n"))
}

pub fn branch_variables(coords: &[RepoCoord]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (i, coord) in coords.iter().enumerate() {
        map.insert(format!("o{i}"), serde_json::Value::String(coord.owner.clone()));
        map.insert(format!("n{i}"), serde_json::Value::String(coord.name.clone()));
    }
    serde_json::Value::Object(map)
}

#[derive(Debug, Clone, Deserialize)]
struct GqlUserLogin {
    login: String,
}
#[derive(Debug, Clone, Deserialize)]
struct GqlGitActor {
    user: Option<GqlUserLogin>,
}
#[derive(Debug, Clone, Deserialize)]
struct GqlCommit {
    oid: Option<String>,
    #[serde(rename = "committedDate")]
    committed_date: Option<String>,
    #[serde(rename = "messageHeadline")]
    message_headline: Option<String>,
    author: Option<GqlGitActor>,
    committer: Option<GqlGitActor>,
}
#[derive(Debug, Clone, Deserialize)]
struct GqlTotalCount {
    #[serde(rename = "totalCount")]
    total_count: i64,
}
#[derive(Debug, Clone, Deserialize)]
struct GqlRefNode {
    name: String,
    #[serde(rename = "associatedPullRequests")]
    associated_pull_requests: GqlTotalCount,
    target: Option<GqlCommit>,
}
#[derive(Debug, Clone, Deserialize)]
struct GqlOid {
    oid: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
struct GqlDefaultBranchRef {
    name: String,
    target: Option<GqlOid>,
}
#[derive(Debug, Clone, Deserialize)]
struct GqlRefs {
    nodes: Vec<Option<GqlRefNode>>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct GqlBranchRepo {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
    url: String,
    #[serde(rename = "defaultBranchRef")]
    default_branch_ref: Option<GqlDefaultBranchRef>,
    refs: Option<GqlRefs>,
}

impl GqlBranchRepo {
    pub fn name_with_owner(&self) -> &str {
        &self.name_with_owner
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn default_branch(&self) -> &str {
        self.default_branch_ref.as_ref().map(|r| r.name.as_str()).unwrap_or("")
    }
}

/// Decodes one aliased `r{i}` node; unknown fields ignored (serde default).
pub fn decode_repo(raw: serde_json::Value) -> Result<GqlBranchRepo, serde_json::Error> {
    serde_json::from_value(raw)
}

struct FilterCtx<'a> {
    default_branch: &'a str,
    default_oid: Option<&'a str>,
    login: &'a str,
    cutoff_ms: i64,
}

fn commit_login(commit: &GqlCommit) -> Option<&str> {
    commit
        .author
        .as_ref()
        .and_then(|a| a.user.as_ref())
        .or_else(|| commit.committer.as_ref().and_then(|c| c.user.as_ref()))
        .map(|u| u.login.as_str())
}

fn qualifies(node: &GqlRefNode, ctx: &FilterCtx) -> bool {
    let Some(commit) = node.target.as_ref() else {
        return false;
    };
    let (Some(committed_date), Some(oid)) = (commit.committed_date.as_deref(), commit.oid.as_deref())
    else {
        return false;
    };
    if committed_date.is_empty() {
        return false;
    }
    if node.name == ctx.default_branch {
        return false;
    }
    if node.associated_pull_requests.total_count > 0 {
        return false;
    }
    if ctx.default_oid == Some(oid) {
        return false;
    }
    if commit_login(commit) != Some(ctx.login) {
        return false;
    }
    parse_iso_millis(committed_date).map(|ms| ms >= ctx.cutoff_ms).unwrap_or(false)
}

/// `Date.parse` equivalent for the ISO8601 timestamps GitHub returns.
fn parse_iso_millis(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp_millis())
}

fn to_prompt(node: &GqlRefNode, repo_url: &str, default_branch: &str) -> GithubBranchPrompt {
    let commit = node.target.as_ref();
    GithubBranchPrompt {
        name: node.name.clone(),
        last_commit_date: commit.and_then(|c| c.committed_date.clone()).unwrap_or_default(),
        last_commit_message: commit.and_then(|c| c.message_headline.clone()).unwrap_or_default(),
        compare_url: format!("{repo_url}/compare/{default_branch}...{}?expand=1", node.name),
    }
}

pub fn select_branches(repo: &GqlBranchRepo, login: &str, cutoff_ms: i64) -> Vec<GithubBranchPrompt> {
    let default_branch = repo.default_branch();
    let ctx = FilterCtx {
        default_branch,
        default_oid: repo
            .default_branch_ref
            .as_ref()
            .and_then(|r| r.target.as_ref())
            .and_then(|t| t.oid.as_deref()),
        login,
        cutoff_ms,
    };
    repo.refs
        .as_ref()
        .map(|r| r.nodes.as_slice())
        .unwrap_or(&[])
        .iter()
        .flatten()
        .filter(|node| qualifies(node, &ctx))
        .map(|node| to_prompt(node, &repo.url, default_branch))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_json(default_branch: &str, default_oid: &str, nodes: serde_json::Value) -> GqlBranchRepo {
        decode_repo(serde_json::json!({
            "nameWithOwner": "me/repo",
            "url": "https://github.com/me/repo",
            "defaultBranchRef": { "name": default_branch, "target": { "oid": default_oid } },
            "refs": { "nodes": nodes },
        }))
        .unwrap()
    }

    fn node(name: &str, login: &str, oid: &str, date: &str, open_prs: i64) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "associatedPullRequests": { "totalCount": open_prs },
            "target": {
                "oid": oid,
                "committedDate": date,
                "messageHeadline": "msg",
                "author": { "user": { "login": login } },
                "committer": null,
            },
        })
    }

    #[test]
    fn to_coord_splits_owner_and_name() {
        assert_eq!(to_coord("a/b").unwrap().owner, "a");
        assert_eq!(to_coord("a/b").unwrap().name, "b");
        assert!(to_coord("noslash").is_none());
        assert!(to_coord("/b").is_none());
    }

    #[test]
    fn selects_only_own_recent_branchless_prs() {
        let cutoff = parse_iso_millis("2024-01-01T00:00:00Z").unwrap();
        let repo = repo_json(
            "main",
            "defaultoid",
            serde_json::json!([
                node("feature", "me", "abc", "2024-06-01T00:00:00Z", 0), // qualifies
                node("main", "me", "x", "2024-06-01T00:00:00Z", 0),       // default branch
                node("has-pr", "me", "def", "2024-06-01T00:00:00Z", 1),   // open PR
                node("not-mine", "other", "ghi", "2024-06-01T00:00:00Z", 0), // other author
                node("stale", "me", "jkl", "2020-01-01T00:00:00Z", 0),    // too old
                node("is-default-oid", "me", "defaultoid", "2024-06-01T00:00:00Z", 0), // == default head
            ]),
        );
        let out = select_branches(&repo, "me", cutoff);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "feature");
        assert_eq!(
            out[0].compare_url,
            "https://github.com/me/repo/compare/main...feature?expand=1"
        );
    }
}
