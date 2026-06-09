//! Ground-truth probe: list OPEN pull requests (head ref) per selected repo via
//! GitHub REST, to compare against what the branch-prompt filter qualifies.
//!   cargo run -p wf-db --example gh_open_prs
//! (dotenvy loads .env; do NOT `source ./.env` first.)

use anyhow::{Context, Result};
use reqwest::Method;
use sea_orm::EntityTrait;
use wf_core::crypto::{Sealed, TokenCipher};
use wf_db::tables::github_pat_connections as gh;
use wf_db::{connect, ConnectOptions};
use wf_github::GithubClient;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let db = connect(&std::env::var("DATABASE_URL")?, ConnectOptions::default()).await?;
    let row = gh::Entity::find().one(&db).await?.context("no PAT row")?;
    let token = decrypt_token(&row)?;
    let repos = selected_repos(&row);
    let client = GithubClient::new(token);

    for repo in &repos {
        let path = format!("/repos/{repo}/pulls?state=open&per_page=100");
        let resp = client.request(Method::GET, &path).send().await?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        let pulls = body.as_array().cloned().unwrap_or_default();
        println!("\n== {repo} :: {status} :: {} open PR(s) ==", pulls.len());
        for pr in &pulls {
            let num = pr.get("number").and_then(|v| v.as_i64()).unwrap_or(-1);
            let head = pr.get("head").and_then(|h| h.get("ref")).and_then(|v| v.as_str()).unwrap_or("?");
            let user = pr.get("user").and_then(|u| u.get("login")).and_then(|v| v.as_str()).unwrap_or("?");
            let created = pr.get("created_at").and_then(|v| v.as_str()).unwrap_or("?");
            println!("  #{num}  head={head}  by={user}  created={created}");
        }
    }
    Ok(())
}

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

fn selected_repos(row: &gh::Model) -> Vec<String> {
    row.selected_repos
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}
