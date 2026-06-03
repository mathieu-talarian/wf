//! Non-destructive live check of favorites DB ops: snapshot the current map,
//! set a probe value (verifying merge + de-dupe + persistence), then restore the
//! original exactly.
//!   cargo run -p wf-db --example gh_favorites

use anyhow::{Context, Result};
use sea_orm::EntityTrait;
use wf_db::entities::github_pat_connections as gh;
use wf_db::repositories::github_pat::{get_favorites, set_repo_favorites};
use wf_db::{connect, ConnectOptions};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let db = connect(&std::env::var("DATABASE_URL")?, ConnectOptions::default()).await?;
    let row = gh::Entity::find().one(&db).await?.context("no PAT row")?;
    let user_id = row.user_id;

    let original = get_favorites(&db, user_id).await?;
    println!("original favorites: {original:?}");

    let repo = row
        .selected_repos
        .as_ref()
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .context("no selected repo to probe with")?
        .to_string();
    let original_ids = original.get(&repo).cloned().unwrap_or_default();

    println!("\nset {repo} = [1, 2, 2, 3] (expect deduped [1, 2, 3]):");
    let after = set_repo_favorites(&db, user_id, &repo, &[1, 2, 2, 3]).await?;
    assert_eq!(after.get(&repo), Some(&vec![1, 2, 3]), "merge/dedupe mismatch");
    println!("  -> {after:?}");

    let reread = get_favorites(&db, user_id).await?;
    assert_eq!(reread.get(&repo), Some(&vec![1, 2, 3]), "not persisted");
    println!("  persisted OK");

    println!("\nrestoring {repo} to original {original_ids:?} ...");
    let restored = set_repo_favorites(&db, user_id, &repo, &original_ids).await?;
    assert_eq!(restored, original, "restore did not match original");
    println!("  restored: {restored:?}");
    println!("\nfavorites round-trip OK (DB unchanged)");
    Ok(())
}
