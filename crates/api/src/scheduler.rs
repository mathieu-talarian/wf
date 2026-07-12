//! In-process tick scheduler: runs `wf_sync::run_tick` at boot (startup
//! reconciliation) and then every `TICK_SCHEDULER_SECS` (default 120) on the
//! actix runtime. Replaces the former `POST /internal/tick` + Cloud Scheduler
//! trigger. On Cloud Run (CPU throttled, scale-to-zero) this is deliberately
//! best-effort: ticks run at instance startup and while the instance is
//! serving traffic, and stall when it idles or scales to zero (see
//! DEPLOYMENT.md).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use actix_web::web;
use tokio::time::MissedTickBehavior;
use tokio::sync::Semaphore;
use wf_sync::TickOptions;

use crate::state::AppState;

static TICK_GATE: Semaphore = Semaphore::const_new(1);
static TICK_PENDING: AtomicBool = AtomicBool::new(false);

fn tick_options(state: &AppState) -> TickOptions {
    TickOptions {
        batch: state.config.tick_batch_size,
        budget: Duration::from_millis(state.config.tick_budget_ms),
        lease_secs: state.config.tick_lease_secs,
        poll_interval_secs: state.config.poll_interval_secs,
        concurrency: state.config.tick_concurrency as usize,
        owner: format!("api-sched-{}", uuid::Uuid::new_v4()),
        github_base: None,
    }
}

async fn run_once(state: &AppState) {
    TICK_PENDING.store(true, Ordering::Release);
    let Ok(_permit) = TICK_GATE.acquire().await else {
        return;
    };
    if !TICK_PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    execute_tick(state).await;
}

async fn execute_tick(state: &AppState) {
    let started = std::time::Instant::now();
    match wf_sync::run_tick(&state.db, &state.cipher, &tick_options(state)).await {
        Ok(s) => tracing::info!(
            target: "tick.scheduler",
            scopes_claimed = s.scopes_claimed,
            scopes_ok = s.scopes_ok,
            scopes_failed = s.scopes_failed,
            events_written = s.events_written,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "tick complete",
        ),
        Err(e) => tracing::warn!(target: "tick.scheduler", error = %e, "tick failed"),
    }
}

async fn run_loop(state: web::Data<AppState>, every: Duration) {
    let mut interval = tokio::time::interval(every);
    // After a stall, resume the cadence instead of firing back-to-back.
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        // The first tick resolves immediately: a baseline run at boot.
        interval.tick().await;
        run_once(&state).await;
    }
}

/// Spawns the scheduler onto the actix runtime. The task is detached: it
/// stops when the server process exits.
pub(crate) fn spawn(state: web::Data<AppState>) {
    let secs = state.config.tick_scheduler_secs;
    tracing::info!(target: "tick.scheduler", interval_secs = secs, "tick scheduler started");
    actix_web::rt::spawn(run_loop(state, Duration::from_secs(secs)));
}

/// Runs an immediate reconciliation/poll after a connection scope changes.
pub(crate) fn trigger(
    state: web::Data<AppState>,
    user_id: sea_orm::prelude::Uuid,
    source: &'static str,
) {
    actix_web::rt::spawn(async move {
        if let Err(error) =
            wf_db::tables::sync_state::mark_source_due(&state.db, user_id, source).await
        {
            tracing::debug!(%error, %user_id, source, "failed to prioritize sync scopes");
        }
        run_once(&state).await;
    });
}
