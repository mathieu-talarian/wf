//! Base / info routes (migration plan §14.1): liveness `/health` and the
//! `/hello/:name` echo. Both unauthenticated. `/me` lands in Phase 2.

use actix_web::{web, HttpResponse};
use chrono::SecondsFormat;
use serde::Serialize;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    time: String,
}

#[derive(Serialize)]
struct HelloResponse {
    greeting: String,
}

/// `GET /api/health` → `{ status: "ok", time: ISO8601 }`.
async fn health() -> HttpResponse {
    // `Date#toISOString()` parity: millisecond precision, `Z` suffix.
    let time = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    HttpResponse::Ok().json(HealthResponse { status: "ok", time })
}

/// `GET /api/hello/:name` → `{ greeting: "Hello, {name}!" }`.
async fn hello(name: web::Path<String>) -> HttpResponse {
    HttpResponse::Ok().json(HelloResponse {
        greeting: format!("Hello, {}!", name),
    })
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/health", web::get().to(health))
        .route("/hello/{name}", web::get().to(hello));
}
