//! Live check of the GraphQL dashboard query against api.github.com, using the
//! real stored PAT + its selected repos.
//!   set -a; . ./.env; set +a; cargo run -p wf-db --example gh_dashboard

use anyhow::{Context, Result};
use sea_orm::EntityTrait;
use wf_core::crypto::{Sealed, TokenCipher};
use wf_db::entities::github_pat_connections as gh;
use wf_db::{connect, ConnectOptions};
use wf_github::{fetch_dashboard, GithubQueueKey};

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

    let repos: Vec<String> = row
        .selected_repos
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    println!("fetch_dashboard login={} repos={} active=mentioned ...", row.github_login, repos.len());
    let data = fetch_dashboard(&token, &row.github_login, &repos, GithubQueueKey::Mentioned).await?;
    println!("counts:");
    for q in &data.queues {
        println!("  {:<16} {}", q.label, q.total_count);
    }
    if let Some(active) = data.queue_pulls.first() {
        println!("active queue '{}' returned {} PR node(s):", active.label, active.pull_requests.len());
        for pr in active.pull_requests.iter().take(5) {
            println!("  #{:<5} {}  [{}]", pr.number, pr.title, pr.repository.full_name);
        }
    }
    Ok(())
}
