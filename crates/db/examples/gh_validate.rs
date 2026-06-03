//! Live check of the GitHub reqwest client + validation state machine, using
//! the real PAT already stored in the DB. Decrypts the token, runs
//! `validate_token` against api.github.com, and prints the (non-sensitive)
//! result. Run with:
//!   set -a; . ./.env; set +a; cargo run -p wf-db --example gh_validate

use anyhow::{Context, Result};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use wf_core::crypto::{Sealed, TokenCipher};
use wf_db::{connect, ConnectOptions};
use wf_github::validate_token;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let database_url = std::env::var("DATABASE_URL")?;
    let enc_key_b64 = std::env::var("GITHUB_TOKEN_ENCRYPTION_KEY")?;

    let db = connect(&database_url, ConnectOptions::default()).await?;
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT github_login, access_token_ciphertext, access_token_iv, access_token_auth_tag \
             FROM github_pat_connections LIMIT 1",
        ))
        .await?
        .context("no github_pat_connections row — link a PAT first")?;

    let login: String = row.try_get("", "github_login")?;
    let sealed = Sealed {
        ciphertext: row.try_get("", "access_token_ciphertext")?,
        iv: row.try_get("", "access_token_iv")?,
        auth_tag: row.try_get("", "access_token_auth_tag")?,
    };
    let key = decode_key(&enc_key_b64)?;
    let token = TokenCipher::new(&key).open(&sealed)?;

    println!("Validating stored token for github_login={login} against api.github.com ...");
    match validate_token(&token).await {
        Ok(r) => {
            println!("VALID ✓");
            println!("  github_user_id : {}", r.github_user_id);
            println!("  login          : {}", r.login);
            println!("  token_kind     : {}", r.token_kind.as_str());
            println!("  scopes         : {:?}", r.scopes);
            println!("  expires_at     : {:?}", r.expires_at);
        }
        Err(e) => {
            println!("REJECTED: status={} http={:?} msg={}", e.status.as_str(), e.http_status, e.message);
        }
    }
    Ok(())
}

fn decode_key(b64: &str) -> Result<[u8; 32]> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes())?;
    raw.try_into()
        .map_err(|_| anyhow::anyhow!("GITHUB_TOKEN_ENCRYPTION_KEY must decode to 32 bytes"))
}
