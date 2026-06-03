//! Read-only check that the `jira_pat_connections` entity maps to the live
//! Supabase table. Prints the row (without secrets) or notes its absence.
//!   cargo run -p wf-db --example jira_row

use anyhow::Result;
use sea_orm::EntityTrait;
use wf_db::entities::jira_pat_connections as jira;
use wf_db::{connect, ConnectOptions};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let db = connect(&std::env::var("DATABASE_URL")?, ConnectOptions::default()).await?;

    match jira::Entity::find().one(&db).await? {
        None => println!("(no jira_pat_connections row — entity maps; select succeeded)"),
        Some(row) => {
            println!("jira connection row (entity ↔ live schema OK):");
            println!("  site_url:          {}", row.site_url);
            println!("  account_id:        {}", row.account_id);
            println!("  email:             {}", row.email);
            println!("  display_name:      {}", row.display_name);
            println!("  cloud_id:          {:?}", row.cloud_id);
            println!("  selected_projects: {:?}", row.selected_projects);
            println!("  validation_status: {}", row.validation_status);
            println!("  last_four:         {:?}", row.last_four);
            println!("  token ciphertext:  {} chars (not shown)", row.api_token_ciphertext.len());
        }
    }
    Ok(())
}
