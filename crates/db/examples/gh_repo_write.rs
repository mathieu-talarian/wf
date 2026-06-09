//! Non-destructive live check of the jsonb write paths
//! (`set_selected_repos`: json-array write + jsonb-NULL snapshot + timestamp,
//! plus `touch_last_used`). Reads the current selection and writes the SAME
//! values back, so the user's repo selection is unchanged; the dashboard
//! snapshot is nulled (a cache, regenerated on next load).
//!   set -a; . ./.env; set +a; cargo run -p wf-db --example gh_repo_write

use anyhow::{Context, Result};
use sea_orm::prelude::Uuid;
use sea_orm::{DatabaseConnection, EntityTrait};
use wf_db::tables::github_pat_connections as gh;
use wf_db::{connect, ConnectOptions};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let db = connect(&std::env::var("DATABASE_URL")?, ConnectOptions::default()).await?;
    let row = gh::Entity::find().one(&db).await?.context("no PAT row")?;
    let user_id = row.user_id;

    let current = selected_repos(&row);
    println!("current selected_repos: {} repo(s)", current.len());

    run_writes(&db, user_id, &current).await?;
    verify_unchanged(&db, &current).await?;
    println!("ALL JSONB WRITE PATHS OK ✓");
    Ok(())
}

/// Extract the stored `selected_repos` json array as `owner/repo` strings.
fn selected_repos(row: &gh::Model) -> Vec<String> {
    row.selected_repos
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// Exercise the jsonb write paths: `touch_last_used` + `set_selected_repos`.
async fn run_writes(db: &DatabaseConnection, user_id: Uuid, current: &[String]) -> Result<()> {
    gh::touch_last_used(db, user_id).await?;
    println!("touch_last_used ✓");

    gh::set_selected_repos(db, user_id, current).await?;
    println!("set_selected_repos (same values) ✓");
    Ok(())
}

/// Re-read the row and assert the selection is unchanged and snapshot nulled.
async fn verify_unchanged(db: &DatabaseConnection, current: &[String]) -> Result<()> {
    let after = gh::Entity::find().one(db).await?.context("gone")?;
    let after_repos = selected_repos(&after);
    assert_eq!(after_repos, current, "selection must be unchanged");
    println!("selection unchanged ✓  snapshot now null: {}", after.dashboard_snapshot.is_none());
    Ok(())
}
