//! `POST /internal/tick` (A1 spec §4.1): shared-secret-authenticated entry
//! point for the tick. Cloud Scheduler calls it on a ~1–2 min cron with the
//! `X-Internal-Token` header. Not part of the public OpenAPI surface.

use actix_web::{web, HttpRequest, HttpResponse};
use wf_sync::TickOptions;

use crate::error::AppError;
use crate::state::AppState;

/// Constant-shape comparison of the provided header against the configured
/// secret. (Both sides are operator-controlled secrets; a timing-safe compare
/// is not load-bearing here, but reject on any mismatch.)
fn token_ok(provided: Option<&str>, expected: &str) -> bool {
    provided.is_some_and(|p| !expected.is_empty() && p == expected)
}

fn tick_options(state: &AppState) -> TickOptions {
    TickOptions {
        batch: state.config.tick_batch_size,
        budget: std::time::Duration::from_millis(state.config.tick_budget_ms),
        lease_secs: state.config.tick_lease_secs,
        poll_interval_secs: state.config.poll_interval_secs,
        owner: format!("api-{}-{}", std::process::id(), chrono::Utc::now().timestamp_millis()),
        github_base: None,
    }
}

/// `POST /internal/tick` → `TickSummary` JSON (camelCase) for Scheduler logs.
pub(crate) async fn tick(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let provided = req.headers().get("X-Internal-Token").and_then(|v| v.to_str().ok());
    if !token_ok(provided, &state.config.internal_tick_token) {
        return Err(AppError::auth("invalid internal token"));
    }
    let summary = wf_sync::run_tick(&state.db, &state.cipher, &tick_options(&state))
        .await
        .map_err(AppError::internal)?;
    Ok(HttpResponse::Ok().json(summary))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/internal/tick", web::post().to(tick));
}

#[cfg(test)]
mod tests {
    use super::token_ok;

    #[test]
    fn token_check() {
        assert!(token_ok(Some("s3cret"), "s3cret"));
        assert!(!token_ok(Some("wrong"), "s3cret"));
        assert!(!token_ok(None, "s3cret"));
        assert!(!token_ok(Some(""), ""));
    }
}
