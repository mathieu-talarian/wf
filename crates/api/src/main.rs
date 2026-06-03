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

use std::sync::Arc;

use actix_cors::Cors;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer};
use tracing::Level;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, Layer};
use wf_core::problem::ProblemDetails;
use wf_core::{Config, TokenCipher};

use crate::auth::JwksVerifier;
use crate::middleware::request_log::request_log;
use crate::state::AppState;

/// Builds an OTLP (http/protobuf) batch tracer when `OTEL_EXPORTER_OTLP_ENDPOINT`
/// is set (spec §12; mirrors `@logtape/otel`). Returns `None` on misconfig so the
/// server still starts.
fn build_otel_tracer(endpoint: &str, service_name: &str) -> Option<opentelemetry_sdk::trace::Tracer> {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        .ok()?;
    let provider = opentelemetry_sdk::trace::TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_resource(opentelemetry_sdk::Resource::new(vec![KeyValue::new(
            "service.name",
            service_name.to_string(),
        )]))
        .build();
    let tracer = provider.tracer("wf-api");
    opentelemetry::global::set_tracer_provider(provider);
    Some(tracer)
}

/// Tracing init with the spec §12 stream split: info/debug/trace → stdout,
/// warn/error → stderr. `RUST_LOG` still overrides the `LOG_LEVEL` directive.
/// When `OTEL_EXPORTER_OTLP_ENDPOINT` is set, also exports spans via OTLP.
fn init_tracing(config: &Config) {
    let directive = config.log_level.tracing_directive();
    let make_filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(directive));

    let stdout_layer = fmt::layer()
        .with_target(true)
        .with_writer(std::io::stdout)
        .with_filter(filter_fn(|m| !matches!(*m.level(), Level::WARN | Level::ERROR)))
        .with_filter(make_filter());
    let stderr_layer = fmt::layer()
        .with_target(true)
        .with_writer(std::io::stderr)
        .with_filter(filter_fn(|m| matches!(*m.level(), Level::WARN | Level::ERROR)))
        .with_filter(make_filter());

    // `Option<Layer>` is itself a `Layer` (no-op when None).
    let otel_layer = config
        .otel_exporter_otlp_endpoint
        .as_deref()
        .and_then(|ep| build_otel_tracer(ep, &config.otel_service_name))
        .map(|tracer| tracing_opentelemetry::layer().with_tracer(tracer));

    tracing_subscriber::registry()
        .with(stdout_layer)
        .with(stderr_layer)
        .with(otel_layer)
        .init();
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
    init_tracing(&config);

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
