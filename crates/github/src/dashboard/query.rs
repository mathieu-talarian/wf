//! Aliased single-GraphQL search builder (port of `dashboard/queries.ts`).
//!
//! GitHub's REST `/search/issues` is throttled and reads repeated `repo:` as
//! AND; GraphQL `search` reads them as OR and isn't subject to the 2s search
//! throttle. So all queue searches ride one (or a few) `/graphql` request(s) as
//! aliased `search` fields.

use super::types::GithubQueueKey;

/// Repos OR'd per search (query length is capped).
const REPO_CHUNK: usize = 5;
/// Aliases per GraphQL request before splitting into concurrent requests.
pub const ALIAS_BATCH: usize = 20;
const PULLS_PER_PAGE: usize = 30;

pub struct QueueBase {
    pub key: GithubQueueKey,
    pub label: &'static str,
    pub base: String,
}

/// The five dashboard queues and their search bases (port of `QUEUE_BASES`).
pub fn queue_bases(login: &str) -> Vec<QueueBase> {
    vec![
        QueueBase { key: GithubQueueKey::Assigned, label: "Assigned", base: format!("is:pr is:open assignee:{login}") },
        QueueBase { key: GithubQueueKey::ReviewRequested, label: "Review requested", base: format!("is:pr is:open review-requested:{login}") },
        QueueBase { key: GithubQueueKey::Authored, label: "Authored", base: format!("is:pr is:open author:{login}") },
        QueueBase { key: GithubQueueKey::Mentioned, label: "Mentioned", base: format!("is:pr is:open involves:{login}") },
        QueueBase { key: GithubQueueKey::FailingCi, label: "Failing CI", base: format!("is:pr is:open involves:{login} status:failure") },
    ]
}

/// Splits repos into OR-able chunks; `[]` → one empty chunk (org-wide search).
pub fn chunk_repos(repos: &[String]) -> Vec<Vec<String>> {
    if repos.is_empty() {
        return vec![vec![]];
    }
    repos.chunks(REPO_CHUNK).map(|c| c.to_vec()).collect()
}

pub fn scoped_query(base: &str, repos: &[String]) -> String {
    if repos.is_empty() {
        base.to_string()
    } else {
        let quals: Vec<String> = repos.iter().map(|r| format!("repo:{r}")).collect();
        format!("{base} {}", quals.join(" "))
    }
}

#[derive(Clone)]
pub struct Spec {
    pub key: GithubQueueKey,
    pub query: String,
    pub with_nodes: bool,
}

/// One spec per (queue × repo-chunk); `with_nodes` decides count-only vs nodes.
pub fn build_specs(login: &str, repos: &[String], with_nodes: impl Fn(GithubQueueKey) -> bool) -> Vec<Spec> {
    let chunks = chunk_repos(repos);
    let mut specs = Vec::new();
    for queue in queue_bases(login) {
        for chunk in &chunks {
            specs.push(Spec {
                key: queue.key,
                query: scoped_query(&queue.base, chunk),
                with_nodes: with_nodes(queue.key),
            });
        }
    }
    specs
}

const PR_FIELDS: &str = "... on PullRequest { \
number title url createdAt updatedAt \
comments { totalCount } \
author { login avatarUrl url } \
repository { nameWithOwner url isPrivate isArchived defaultBranchRef { name } } \
assignees(first: 10) { nodes { login avatarUrl url } } \
labels(first: 20) { nodes { name color } } \
}";

fn clause_for(with_nodes: bool) -> String {
    if with_nodes {
        format!(", first: {PULLS_PER_PAGE}) {{ issueCount nodes {{ {PR_FIELDS} }} }}")
    } else {
        ") { issueCount }".to_string()
    }
}

/// Assembles the aliased GraphQL query string for a batch of specs.
pub fn build_query(specs: &[Spec]) -> String {
    let decls: Vec<String> = (0..specs.len()).map(|i| format!("$q{i}: String!")).collect();
    let fields: Vec<String> = specs
        .iter()
        .enumerate()
        .map(|(i, spec)| format!("s{i}: search(query: $q{i}, type: ISSUE{}", clause_for(spec.with_nodes)))
        .collect();
    format!("query({}) {{\n{}\n}}", decls.join(", "), fields.join("\n"))
}

pub fn variables_of(specs: &[Spec]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (i, spec) in specs.iter().enumerate() {
        map.insert(format!("q{i}"), serde_json::Value::String(spec.query.clone()));
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_repos_in_fives() {
        let repos: Vec<String> = (0..12).map(|i| format!("o/r{i}")).collect();
        let chunks = chunk_repos(&repos);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 5);
        assert_eq!(chunks[2].len(), 2);
        assert_eq!(chunk_repos(&[]), vec![Vec::<String>::new()]);
    }

    #[test]
    fn scoped_query_ors_repos() {
        assert_eq!(scoped_query("is:pr", &[]), "is:pr");
        assert_eq!(
            scoped_query("is:pr", &["a/b".into(), "c/d".into()]),
            "is:pr repo:a/b repo:c/d"
        );
    }

    #[test]
    fn builds_aliased_query_and_variables() {
        let specs = build_specs("me", &["a/b".into()], |k| k == GithubQueueKey::Assigned);
        assert_eq!(specs.len(), 5); // one chunk × 5 queues
        let q = build_query(&specs);
        assert!(q.contains("s0: search(query: $q0, type: ISSUE"));
        assert!(q.contains("$q0: String!"));
        // Active queue (assigned) has nodes; others are count-only.
        assert!(q.contains("issueCount nodes"));
        let vars = variables_of(&specs);
        assert_eq!(vars["q0"], "is:pr is:open assignee:me repo:a/b");
    }
}
