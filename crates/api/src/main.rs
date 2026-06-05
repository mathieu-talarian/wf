//! actix-web bootstrap: config → tracing → db → AppState → HttpServer
//! (migration plan §16). Phase 2 adds the Supabase JWKS verifier, the token
//! cipher, the DB connection, and `GET /me`.

mod auth;
mod dto;
mod error;
mod github;
mod jira;
mod middleware;
mod openapi;
mod routes;
mod state;
mod telemetry;

use std::sync::Arc;

use actix_cors::Cors;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer};
use wf_core::problem::ProblemDetails;
use wf_core::{Config, TokenCipher};

use crate::auth::JwksVerifier;
use crate::middleware::request_tracing::RequestTracing;
use crate::state::AppState;

/// Liveness/startup probe for Cloud Run (`service.yaml` points its probes here).
/// Unauthenticated and does no I/O (no DB), so it stays green independent of
/// downstream health and never blocks on the connection pool.
async fn healthz() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain; charset=utf-8")
        .body("ok")
}

/// Default 404, emitted in the same `application/problem+json` envelope as
/// handler errors (migration plan §9: framework-level not-found).
async fn not_found(req: HttpRequest) -> HttpResponse {
    let instance = req.uri().path_and_query().map(|pq| pq.as_str().to_string());
    // Parity with Elysia's framework 404: the `detail` is the error message,
    // which for an unmatched route is the literal "NOT_FOUND".
    let problem = ProblemDetails::new(404, "not-found", "Not Found", "NOT_FOUND").with_instance(instance);
    HttpResponse::NotFound()
        .content_type("application/problem+json")
        .body(serde_json::to_string(&problem).unwrap_or_else(|_| "{}".to_string()))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Load `.env` (walks up from cwd) so local runs need no manual sourcing.
    // Real environment variables already set always take precedence.
    let _ = dotenvy::dotenv();

    let config = Config::load().expect("invalid environment configuration");
    let telemetry_guard = telemetry::init(&config).expect("failed to initialise OpenTelemetry");

    // Boot-time guard: the encryption key must decode to exactly 32 bytes
    // (migration plan §5), mirroring the TS `decodeKey` throw.
    let key = config
        .encryption_key_bytes()
        .expect("GITHUB_TOKEN_ENCRYPTION_KEY must decode to 32 bytes");

    let db = wf_db::connect(&config.database_url, wf_db::ConnectOptions::default())
        .await
        .expect("database connection failed");

    let jwks = Arc::new(JwksVerifier::new(
        &config.supabase_url,
        &config.supabase_jwt_audience,
    ));
    let cipher = Arc::new(TokenCipher::new(&key));

    let port = config.port;
    let origins = config.cors_origins.clone();

    tracing::info!(
        target: "server.start",
        port,
        node_env = ?config.node_env,
        otel_enabled = config.otel_exporter_otlp_endpoint.is_some(),
    );

    let state = web::Data::new(AppState {
        config: Arc::new(config),
        db,
        jwks,
        cipher,
        token_cache: Arc::new(crate::github::token_cache::TokenCache::default()),
        dashboard_cache: Arc::new(crate::github::dashboard_cache::DashboardCache::default()),
    });

    let server = HttpServer::new(move || {
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
            .wrap(RequestTracing::new())
            .route("/healthz", web::get().to(healthz))
            .service(web::scope("/api").configure(routes::configure))
            .default_service(web::route().to(not_found))
    })
    .bind(("0.0.0.0", port))?
    .run();

    let result = server.await;
    // Flush pending traces/metrics/logs before the process exits.
    telemetry_guard.shutdown();
    result
}
