//! NON-DESTRUCTIVE live check of the write path + error mapping: dispatch a
//! bogus workflow id and merge a bogus PR number, both of which GitHub rejects
//! (404/422) without creating anything. Confirms `GithubError::Write` carries the
//! upstream status through.
//!   cargo run -p wf-db --example gh_write_probe
//! (Do NOT `source ./.env` first — dotenvy loads it.)

use std::collections::HashMap;

use anyhow::{Context, Result};
use sea_orm::EntityTrait;
use wf_core::crypto::{Sealed, TokenCipher};
use wf_db::entities::github_pat_connections as gh;
use wf_db::{connect, ConnectOptions};
use wf_github::{dispatch_workflow, merge_pull, GithubError, GithubMergeMethod, RepoRef};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let db = connect(&std::env::var("DATABASE_URL")?, ConnectOptions::default()).await?;
    let row = gh::Entity::find().one(&db).await?.context("no PAT row")?;

    let enc = std::env::var("GITHUB_TOKEN_ENCRYPTION_KEY")?;
    let token = {
        use base64::Engine;
        let raw = base64::engine::general_purpose::STANDARD.decode(enc)?;
        let arr: [u8; 32] = raw.try_into().map_err(|_| anyhow::anyhow!("key not 32 bytes"))?;
        TokenCipher::new(&arr).open(&Sealed {
            ciphertext: row.access_token_ciphertext.clone(),
            iv: row.access_token_iv.clone(),
            auth_tag: row.access_token_auth_tag.clone(),
        })?
    };

    let full = row
        .selected_repos
        .as_ref()
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .context("no selected repo to probe against")?
        .to_string();
    let (owner, repo) = full.split_once('/').context("bad repo full name")?;
    let r = RepoRef { owner: owner.to_string(), repo: repo.to_string() };
    println!("probing writes against {full} (no resources will be created)\n");

    println!("dispatch bogus workflow id 999999999 on main:");
    match dispatch_workflow(&token, &r, 999_999_999, "main", &HashMap::new()).await {
        Ok(()) => println!("  unexpectedly OK"),
        Err(GithubError::Write { status, message }) => println!("  Write {{ status: {status}, message: {message:?} }}"),
        Err(e) => println!("  other error: {e}"),
    }

    println!("\nmerge bogus PR number 999999999:");
    match merge_pull(&token, &r, 999_999_999, GithubMergeMethod::Squash).await {
        Ok(res) => println!("  unexpectedly OK: {res:?}"),
        Err(GithubError::Write { status, message }) => println!("  Write {{ status: {status}, message: {message:?} }}"),
        Err(e) => println!("  other error: {e}"),
    }

    Ok(())
}
