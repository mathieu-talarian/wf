# Workflow backend — Rust rewrite

Rust port of the TypeScript backend. Faithful port; TS source-of-truth (read-only):
`../workflow/apps/server`. Spec: `2026-06-03-ts-to-rust-backend.md`.

## Build & verify
- Lint gate (CI): `cargo clippy --all --all-targets --locked -- -D warnings` — run this, not plain clippy.
- Tests: `cargo test --workspace`. Build: `cargo build --workspace`.
- `#[cfg(test)] mod tests` must be the LAST item in a file (clippy `items-after-test-module`).
- Public types with a `new()` need a `Default` impl too (clippy `new_without_default` fails under `-D warnings`).
- Workspace crates: `wf-core`, `wf-db`, `wf-github`, `wf-jira`, `wf-api` (the `wf-` prefix avoids the std `core` clash).

## Architecture
- `wf-core` — config, AES-256-GCM `TokenCipher`, RFC 9457 problem, auth types (no actix/db deps).
- `wf-db` — SeaORM entities + repositories; `connect()` (statement-cache disabled).
- `wf-github` / `wf-jira` — `reqwest` clients + domain logic for each integration.
- `wf-api` — actix-web bin: `AppState` (DI), `AuthUser` extractor (Supabase JWKS), routes, middleware. Entry: `crates/api/src/main.rs`.
  - Observability: `crates/api/src/telemetry.rs` (OTLP traces+metrics+logs) + `middleware/request_tracing.rs` (root span, trace propagation, `http.server.*` metrics). `main()` holds a `TelemetryGuard` and calls `shutdown()` after the server stops.

## Database (Supabase)
- `DATABASE_URL` must be the **session pooler** (`...pooler.supabase.com:5432`).
  The transaction pooler (6543) breaks SeaORM/sqlx with `42P05` even with `statement_cache_capacity(0)`; the direct host (`db.<ref>.supabase.co`) is IPv6-only.
- Raw SQL in sea-orm 2.0: `db.query_one_raw(stmt)` / `query_all_raw` (the generic `query_one` is for query-builders).

## Env & running
- `.env` is auto-loaded via dotenvy: `cargo run -p wf-api` works without sourcing. `.env` is gitignored.
- Live smoke harnesses (need `.env` + real data): `cargo run -p wf-db --example {phase0,gh_validate,gh_repo,gh_dashboard,gh_repo_write}`.

## Dependency feature gotchas
- `jsonwebtoken` → `features=["rust_crypto"]` (else runtime "CryptoProvider" panic).
- `reqwest` → `query` feature for `RequestBuilder::query`; TLS feature is `rustls`.
- `sqlx` → `runtime-tokio` + `tls-rustls-ring`. `getrandom` → `fill()` (not `getrandom()`).
- OpenTelemetry is 0.32: use `SdkTracerProvider` / `Resource::builder()` / `with_batch_exporter(exporter)` (no runtime arg). `opentelemetry-otlp` uses the **blocking** exporter (`reqwest-blocking-client`, no `rt-tokio` SDK feature).

## Deployment (Cloud Run)
- Two-container service (app + OTel Collector sidecar). Files: `Dockerfile`, `cloudbuild.yaml`, `service.yaml`, `otel-config.yaml`, `deploy/terraform/`. Full guide: `DEPLOYMENT.md`. Patterns ported from the read-only reference repo `../otlp`.
- `main()` connects to the DB *before* binding, so a container won't serve `/healthz` without a reachable `DATABASE_URL` (Secret Manager in prod).
- Toolchain pinned via `rust-toolchain.toml` (stable) so local/CI/Docker agree — avoids the clippy `E0514` toolchain-mismatch class.

## Conventions
- Response DTOs: `#[serde(rename_all = "camelCase")]`; timestamps ISO8601 millis+Z (`to_rfc3339_opts(SecondsFormat::Millis, true)`).
- Errors: return `AppError` → RFC 9457 `application/problem+json` (carries `instance`/`reason`).
- Tooling: prefer Serena MCP for code edits/reads; context7 for library docs.
