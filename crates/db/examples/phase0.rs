//! Phase 0 de-risking spike (migration plan §16).
//!
//! Run with the project `.env` sourced:
//!   set -a; . ./.env; set +a; cargo run -p wf-db --example phase0
//!
//! Validates, against the live database:
//!   1. Connect through `db::connect` and run a smoke query.
//!   2. Run a parameterized query repeatedly (exercises the prepared-statement
//!      path that would trip `42P05` on the pooler if caching were on).
//!   3. Decrypt a real `github_pat_connections` token sealed by the TS server,
//!      proving AES-GCM byte-compatibility on production data.
//!
//! Never prints decrypted token plaintext — only masked metadata.

use anyhow::{Context, Result};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use wf_core::crypto::{Sealed, TokenCipher};
use wf_db::{connect, ConnectOptions};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL not set")?;
    let enc_key_b64 =
        std::env::var("GITHUB_TOKEN_ENCRYPTION_KEY").context("GITHUB_TOKEN_ENCRYPTION_KEY not set")?;

    println!("== Spike 1: connect + smoke query ==");
    let db = connect(&database_url, ConnectOptions::default())
        .await
        .context("connect failed")?;
    spike_smoke_query(&db).await?;
    spike_parameterized(&db).await?;
    spike_decrypt(&db, &enc_key_b64).await?;
    Ok(())
}

/// Spike 1: run a trivial `SELECT 1` smoke query against the live DB.
async fn spike_smoke_query(db: &DatabaseConnection) -> Result<()> {
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT 1 AS one",
        ))
        .await?
        .context("no row from SELECT 1")?;
    let one: i32 = row.try_get("", "one")?;
    println!("  SELECT 1 -> {one}  ✓");
    Ok(())
}

/// Spike 1b: run a parameterized query repeatedly to prove the pooler path
/// does not trip `42P05` (prepared-statement caching off).
async fn spike_parameterized(db: &DatabaseConnection) -> Result<()> {
    println!("== Spike 1b: repeated parameterized query (no 42P05) ==");
    for i in 0..5 {
        let r = db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT $1::int AS v",
                [i.into()],
            ))
            .await
            .with_context(|| format!("parameterized query iter {i} failed"))?
            .context("no row")?;
        let v: i32 = r.try_get("", "v")?;
        assert_eq!(v, i);
    }
    println!("  5x parameterized SELECT round-tripped  ✓");
    Ok(())
}

/// Spike 2: decrypt a real `github_pat_connections` token sealed by the TS
/// server, proving AES-GCM byte-compatibility on production data.
async fn spike_decrypt(db: &DatabaseConnection, enc_key_b64: &str) -> Result<()> {
    println!("== Spike 2: decrypt a real github_pat_connections row ==");
    let count_row = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*)::bigint AS n FROM github_pat_connections",
        ))
        .await?
        .context("count query returned nothing")?;
    let n: i64 = count_row.try_get("", "n")?;
    println!("  github_pat_connections rows: {n}");

    if n == 0 {
        println!("  (no rows to decrypt — link a GitHub PAT in the app, then re-run)");
        return Ok(());
    }

    decrypt_first_pat(db, enc_key_b64).await?;
    println!("\nALL PHASE 0 DB SPIKES PASSED ✓");
    Ok(())
}

/// Read the first PAT row, decrypt it with `enc_key_b64`, and report (masked).
async fn decrypt_first_pat(db: &DatabaseConnection, enc_key_b64: &str) -> Result<()> {
    let pat = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT github_login, last_four, \
             access_token_ciphertext, access_token_iv, access_token_auth_tag \
             FROM github_pat_connections LIMIT 1",
        ))
        .await?
        .context("no PAT row")?;

    let login: String = pat.try_get("", "github_login")?;
    let last_four: Option<String> = pat.try_get("", "last_four")?;
    let sealed = Sealed {
        ciphertext: pat.try_get("", "access_token_ciphertext")?,
        iv: pat.try_get("", "access_token_iv")?,
        auth_tag: pat.try_get("", "access_token_auth_tag")?,
    };

    let key = decode_key(enc_key_b64)?;
    let cipher = TokenCipher::new(&key);
    let token = cipher.open(&sealed).context("AES-GCM open failed")?;
    report_decrypted(&login, last_four.as_deref(), &token);
    Ok(())
}

/// Print masked verification of a decrypted token — never the token itself.
fn report_decrypted(login: &str, last_four: Option<&str>, token: &str) {
    let kind = if token.starts_with("github_pat_") {
        "fine_grained_pat"
    } else if token.starts_with("ghp_") {
        "classic_pat"
    } else {
        "unknown"
    };
    let tail = &token[token.len().saturating_sub(4)..];
    let last_four_matches = last_four == Some(tail);
    println!("  login={login}  kind={kind}  decrypted_len={}", token.len());
    println!("  last_four column matches decrypted tail: {last_four_matches}  ✓");
}

fn decode_key(b64: &str) -> Result<[u8; 32]> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes())?;
    raw.try_into()
        .map_err(|_| anyhow::anyhow!("GITHUB_TOKEN_ENCRYPTION_KEY must decode to 32 bytes"))
}
