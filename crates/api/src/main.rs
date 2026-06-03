//! actix-web bootstrap: config → tracing → AppState → HttpServer.
//! Phase 1 skeleton (migration plan §16): CORS, request logging, RFC 9457
//! problem plumbing, and the unauthenticated `/health` + `/hello/:name` routes.

mod error;
mod middleware;
mod routes;
mod state;

use actix_cors::Cors;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer};
use tracing_subscriber::EnvFilter;
use wf_core::problem::ProblemDetails;
use wf_core::Config;

use crate::middleware::request_log::request_log;
use crate::state::AppState;

fn init_tracing(config: &Config) {
    // Prefer RUST_LOG when set; otherwise use the configured LOG_LEVEL.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.log_level.tracing_directive()));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

/// Default 404, emitted in the same `application/problem+json` envelope as
/// handler errors (migration plan §9: framework-level not-found).
async fn not_found(req: HttpRequest) -> HttpResponse {
    let instance = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string());
    let problem = ProblemDetails::new(404, "not-found", "Not Found", "Not Found")
        .with_instance(instance);
    HttpResponse::NotFound()
        .content_type("application/problem+json")
        .body(serde_json::to_string(&problem).unwrap_or_else(|_| "{}".to_string()))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let config = Config::load().expect("invalid environment configuration");
    init_tracing(&config);

    // Boot-time guard: the encryption key must decode to exactly 32 bytes
    // (migration plan §5). Fail fast, mirroring the TS `decodeKey` throw.
    config
        .encryption_key_bytes()
        .expect("GITHUB_TOKEN_ENCRYPTION_KEY must decode to 32 bytes");

    let port = config.port;
    let origins = config.cors_origins.clone();
    let state = web::Data::new(AppState::new(config));

    tracing::info!(
        target: "server.start",
        port,
        node_env = ?state.config.node_env,
        otel_enabled = state.config.otel_exporter_otlp_endpoint.is_some(),
    );

    HttpServer::new(move || {
        let mut cors = Cors::default()
            .supports_credentials()
            .allow_any_method()
            .allow_any_header();
        for origin in &origins {
            cors = cors.allowed_origin(origin);
        }

        App::new()
            .app_data(state.clone())
            .wrap(cors)
            .wrap(actix_web::middleware::from_fn(request_log))
            .service(web::scope("/api").configure(routes::configure))
            .default_service(web::route().to(not_found))
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
