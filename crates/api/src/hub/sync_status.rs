//! Durable synchronization freshness exposed to consumers.

use std::collections::HashMap;

use sea_orm::prelude::Uuid;
use wf_db::tables::sync_state;

use crate::error::AppError;
use crate::hub::types::{HubSyncSource, HubSyncStatus};
use crate::state::AppState;

pub async fn status(state: &AppState, user_id: Uuid) -> Result<HubSyncStatus, AppError> {
    let rows = sync_state::list_user_scopes(&state.db, user_id).await?;
    let mut grouped: HashMap<String, Vec<sync_state::Model>> = HashMap::new();
    for row in rows {
        grouped.entry(row.source.clone()).or_default().push(row);
    }
    let mut sources: Vec<_> = grouped.into_iter().map(source_status).collect();
    sources.sort_by(|a, b| a.source.cmp(&b.source));
    Ok(HubSyncStatus { sources })
}

fn source_status((source, rows): (String, Vec<sync_state::Model>)) -> HubSyncSource {
    let now = chrono::Utc::now().fixed_offset();
    let running = rows.iter().any(|row| row.lease_until.is_some_and(|until| until > now));
    let pending = rows.iter().any(|row| row.last_polled_at.is_none());
    let last_error = rows.iter().find_map(|row| row.last_error.clone());
    let state = if running { "running" } else if last_error.is_some() { "failed" } else if pending { "pending" } else { "ready" };
    let as_of = rows
        .iter()
        .filter_map(|row| row.last_polled_at)
        .min()
        .map(|at| at.to_rfc3339());
    HubSyncSource {
        source,
        state: state.to_string(),
        as_of,
        last_error,
        scope_count: rows.len() as i64,
    }
}
