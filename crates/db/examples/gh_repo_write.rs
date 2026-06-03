//! Non-destructive live check of the jsonb write paths
//! (`set_selected_repos`: json-array write + jsonb-NULL snapshot + timestamp,
//! plus `touch_last_used`). Reads the current selection and writes the SAME
//! values back, so the user's repo selection is unchanged; the dashboard
//! snapshot is nulled (a cache, regenerated on next load).
//!   set -a; . ./.env; set +a; cargo run -p wf-db --example gh_repo_write

use anyhow::{Context, Result};
use sea_orm::EntityTrait;
use wf_db::entities::github_pat_connections as gh;
use wf_db::repositories::github_pat;
use wf_db::{connect, ConnectOptions};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let db = connect(&std::env::var("DATABASE_URL")?, ConnectOptions::default()).await?;
    let row = gh::Entity::find().one(&db).await?.context("no PAT row")?;
    let user_id = row.user_id;

    let current: Vec<String> = row
        .selected_repos
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    println!("current selected_repos: {} repo(s)", current.len());

    github_pat::touch_last_used(&db, user_id).await?;
    println!("touch_last_used ✓");

    github_pat::set_selected_repos(&db, user_id, &current).await?;
    println!("set_selected_repos (same values) ✓");

    let after = gh::Entity::find().one(&db).await?.context("gone")?;
    let after_repos: Vec<String> = after
        .selected_repos
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    assert_eq!(after_repos, current, "selection must be unchanged");
    println!("selection unchanged ✓  snapshot now null: {}", after.dashboard_snapshot.is_none());
    println!("ALL JSONB WRITE PATHS OK ✓");
    Ok(())
}
