//! Live smoke (A1 spec §9): one tick against the real DB + providers.
//! Run: `cargo run -p wf-sync --example tick_smoke`

use std::time::Duration;

use wf_core::{Config, TokenCipher};
use wf_sync::TickOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cfg = Config::load()?;
    let db = wf_db::connect(&cfg.database_url, wf_db::ConnectOptions::default()).await?;
    let cipher = TokenCipher::new(&cfg.encryption_key_bytes()?);
    let opts = TickOptions {
        batch: cfg.tick_batch_size,
        budget: Duration::from_millis(cfg.tick_budget_ms),
        lease_secs: cfg.tick_lease_secs,
        poll_interval_secs: cfg.poll_interval_secs,
        concurrency: cfg.tick_concurrency as usize,
        owner: "tick_smoke".to_string(),
        github_base: None,
    };
    let summary = wf_sync::run_tick(&db, &cipher, &opts).await?;
    println!("tick summary: {summary:?}");
    println!("(first run per scope is the baseline — re-run after new activity to see events)");
    Ok(())
}
