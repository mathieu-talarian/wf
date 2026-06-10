//! One-off SQL runner over `DATABASE_URL` (handles the session-pooler quirks
//! psql trips on). Each row prints as one JSON object.
//!
//! Usage: cargo run -p wf-db --example sql -- "SELECT * FROM events LIMIT 5"

use sea_orm::{ConnectionTrait, DbBackend, Statement};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let query = std::env::args()
        .nth(1)
        .expect("usage: cargo run -p wf-db --example sql -- \"<query>\"");
    let db = wf_db::connect(&std::env::var("DATABASE_URL")?, wf_db::ConnectOptions::default())
        .await?;
    let wrapped = format!("SELECT row_to_json(t)::text AS j FROM ({query}) t");
    let rows = db
        .query_all_raw(Statement::from_string(DbBackend::Postgres, wrapped))
        .await?;
    for row in &rows {
        println!("{}", row.try_get::<String>("", "j")?);
    }
    eprintln!("({} rows)", rows.len());
    Ok(())
}
