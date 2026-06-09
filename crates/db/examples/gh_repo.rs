//! Validates the `github_pat_connections` SeaORM entity against the live schema
//! by selecting the real row through the entity (catches any column/type drift).
//!   set -a; . ./.env; set +a; cargo run -p wf-db --example gh_repo

use anyhow::Result;
use sea_orm::EntityTrait;
use wf_db::tables::github_pat_connections as gh;
use wf_db::{connect, ConnectOptions};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let db = connect(&std::env::var("DATABASE_URL")?, ConnectOptions::default()).await?;
    match gh::Entity::find().one(&db).await? {
        None => println!("no rows (entity deserialization not exercised)"),
        Some(r) => {
            println!("entity ↔ live schema OK ✓");
            println!("  user_id           : {}", r.user_id);
            println!("  github_login      : {}", r.github_login);
            println!("  github_user_id    : {}", r.github_user_id);
            println!("  token_kind        : {}", r.token_kind);
            println!("  validation_status : {}", r.validation_status);
            println!("  scope             : {:?}", r.scope);
            println!("  selected_repos    : {:?}", r.selected_repos);
            println!("  favorite_workflows: {:?}", r.favorite_workflows);
            println!("  dashboard_snapshot: {}", r.dashboard_snapshot.is_some());
            println!("  last_four         : {:?}", r.last_four);
            println!("  expires_at        : {:?}", r.expires_at);
            println!("  created_at        : {}", r.created_at);
        }
    }
    Ok(())
}
