//! Live check of per-PR enrichment against api.github.com, using the real
//! stored PAT. Picks PR refs from the `mentioned` queue (or pass owner/repo/number).
//!   set -a; . ./.env; set +a; cargo run -p wf-db --example gh_pr_enrich
//!   set -a; . ./.env; set +a; cargo run -p wf-db --example gh_pr_enrich -- owner repo 123

use anyhow::{Context, Result};
use sea_orm::EntityTrait;
use wf_core::crypto::{Sealed, TokenCipher};
use wf_db::entities::github_pat_connections as gh;
use wf_db::{connect, ConnectOptions};
use wf_github::{
    enrich_pull_request, fetch_queue_pulls, parse_repo_ref, GithubPullRequestEnrichment,
    GithubQueueKey, RepoRef,
};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let db = connect(&std::env::var("DATABASE_URL")?, ConnectOptions::default()).await?;
    let row = gh::Entity::find().one(&db).await?.context("no PAT row")?;
    let token = decrypt_token(&row)?;
    let repos = selected_repos(&row);

    let Some((r, number)) = resolve_target(&token, &row.github_login, &repos).await? else {
        println!("no PR found to enrich (empty mentioned queue); pass owner repo number");
        return Ok(());
    };

    println!("enrich {}/{} #{number} ...", r.owner, r.repo);
    let e = enrich_pull_request(&token, &r, number).await;
    print_enrichment(&e);
    Ok(())
}

/// Decrypt the stored PAT using `GITHUB_TOKEN_ENCRYPTION_KEY`.
fn decrypt_token(row: &gh::Model) -> Result<String> {
    use base64::Engine;
    let enc = std::env::var("GITHUB_TOKEN_ENCRYPTION_KEY")?;
    let raw = base64::engine::general_purpose::STANDARD.decode(enc)?;
    let arr: [u8; 32] = raw.try_into().map_err(|_| anyhow::anyhow!("key not 32 bytes"))?;
    Ok(TokenCipher::new(&arr).open(&Sealed {
        ciphertext: row.access_token_ciphertext.clone(),
        iv: row.access_token_iv.clone(),
        auth_tag: row.access_token_auth_tag.clone(),
    })?)
}

/// Extract the stored `selected_repos` json array as `owner/repo` strings.
fn selected_repos(row: &gh::Model) -> Vec<String> {
    row.selected_repos
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// Resolve the PR to enrich: an explicit `owner repo number` from argv, or the
/// first PR in the mentioned queue.
async fn resolve_target(
    token: &str,
    login: &str,
    repos: &[String],
) -> Result<Option<(RepoRef, i64)>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 3 {
        return Ok(Some((
            RepoRef { owner: args[0].clone(), repo: args[1].clone() },
            args[2].parse().context("number must be an integer")?,
        )));
    }
    let queue = fetch_queue_pulls(token, login, repos, GithubQueueKey::Mentioned).await?;
    Ok(queue
        .pull_requests
        .first()
        .and_then(|pr| parse_repo_ref(&pr.repository.full_name).map(|r| (r, pr.number))))
}

/// Print the (non-sensitive) fields of a PR enrichment result.
fn print_enrichment(e: &GithubPullRequestEnrichment) {
    println!("  repository:   {}", e.repository.full_name);
    println!("  +{:?} -{:?}  files={:?}  draft={:?}", e.additions, e.deletions, e.changed_files, e.draft);
    println!("  headRef={:?} baseRef={:?}", e.head_ref, e.base_ref);
    println!("  approvalState: {:?}", e.approval_state);
    println!("  mergeable={:?} mergeableState={:?} behind={:?}", e.mergeable, e.mergeable_state, e.branch_behind);
    println!("  requestedReviewers: {}", e.requested_reviewers.len());
    println!("  requiredChecks: {}", e.required_checks.len());
    for c in e.required_checks.iter().take(8) {
        println!("    [{}] {} {:?}", if c.required { "req" } else { "   " }, c.name, c.state);
    }
    println!("  latestRun: {:?}", e.latest_run.as_ref().map(|r| (&r.status, &r.conclusion, r.run_number)));
    println!("  badges:   {:?}", e.readiness_badges);
    println!("  blockers: {:?}", e.blocker_summary);
}
