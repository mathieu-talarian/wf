//! Migration CLI. Run against the **session pooler (:5432)** DATABASE_URL
//! (spec §7.1): `cargo run -p migration -- up` / `-- status`.

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    sea_orm_migration::cli::run_cli(migration::Migrator).await;
}
