//! Database layer: connection management and per-table modules under `tables`
//! (each owns its SeaORM entity + CRUD operations).
//!
//! The one critical infra detail (migration plan §6.1): when `DATABASE_URL`
//! points at Supabase's **transaction pooler on port 6543** (Supavisor), it
//! rejects *named* prepared statements with `42P05 prepared statement already
//! exists`. SQLx's Postgres driver caches named prepared statements by default,
//! so we **disable the statement cache** (`statement_cache_capacity(0)`). With
//! the cache off, SQLx uses unnamed prepared statements, which are safe in both
//! transaction-pooling mode and on a direct/session connection (port 5432).
//! Keeping it disabled unconditionally means a single code path works whether
//! the deploy targets the pooler or a direct connection.

use std::str::FromStr;
use std::time::Duration;

use sea_orm::{DatabaseConnection, SqlxPostgresConnector};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use thiserror::Error;

pub mod tables;

// Re-export the SeaORM connection type so the api crate need not depend on
// sea-orm directly just to name it.
pub use sea_orm::DatabaseConnection as Db;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("invalid DATABASE_URL: {0}")]
    InvalidUrl(String),
    #[error("database connection failed: {0}")]
    Connect(String),
}

/// Connection tuning. Pool sizing is an open question against the pooler's own
/// limits (migration plan §19.4); these are conservative defaults.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout: Duration,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            max_connections: 10,
            min_connections: 1,
            acquire_timeout: Duration::from_secs(10),
        }
    }
}

/// Connects to Postgres through the transaction pooler with the prepared-
/// statement cache disabled, returning a pooled SeaORM connection.
pub async fn connect(
    database_url: &str,
    opts: ConnectOptions,
) -> Result<DatabaseConnection, DbError> {
    // Build PgConnectOptions explicitly so we can disable the statement cache —
    // SeaORM's own `ConnectOptions` does not expose this knob.
    let connect_options = PgConnectOptions::from_str(database_url)
        .map_err(|e| DbError::InvalidUrl(e.to_string()))?
        .statement_cache_capacity(0)
        .application_name("workflow-server");

    let pool = PgPoolOptions::new()
        .max_connections(opts.max_connections)
        .min_connections(opts.min_connections)
        .acquire_timeout(opts.acquire_timeout)
        .connect_with(connect_options)
        .await
        .map_err(|e| DbError::Connect(e.to_string()))?;

    Ok(SqlxPostgresConnector::from_sqlx_postgres_pool(pool))
}
