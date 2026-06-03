//! Base / info routes (migration plan §14.1): liveness `/health` and the
//! `/hello/:name` echo. Both unauthenticated. `/me` lands in Phase 2.

use actix_web::{web, HttpResponse};
use chrono::SecondsFormat;
use serde::Serialize;

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct HealthResponse {
    status: String,
    time: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct HelloResponse {
    greeting: String,
}

#[utoipa::path(
    get,
    path = "/api/health",
    operation_id = "getHealth",
    tag = "system",
    responses((status = 200, body = HealthResponse))
)]
/// `GET /api/health` → `{ status: "ok", time: ISO8601 }`.
pub(crate) async fn health() -> HttpResponse {
    // `Date#toISOString()` parity: millisecond precision, `Z` suffix.
    let time = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    HttpResponse::Ok().json(HealthResponse { status: "ok".to_string(), time })
}

#[utoipa::path(
    get,
    path = "/api/hello/{name}",
    operation_id = "getHello",
    tag = "system",
    params(("name" = String, Path, description = "Name to greet")),
    responses((status = 200, body = HelloResponse))
)]
/// `GET /api/hello/:name` → `{ greeting: "Hello, {name}!" }`.
pub(crate) async fn hello(name: web::Path<String>) -> HttpResponse {
    HttpResponse::Ok().json(HelloResponse {
        greeting: format!("Hello, {}!", name),
    })
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/health", web::get().to(health))
        .route("/hello/{name}", web::get().to(hello));
}
