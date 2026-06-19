# Workflow backend — Rust rewrite

## Tools must use
- Serena
- Sequential thinking
- context7
- graphify

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

When the user types `/graphify`, invoke the `skill` tool with `skill: "graphify"` before doing anything else.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- Dirty graphify-out/ files are expected after hooks or incremental updates; dirty graph files are not a reason to skip graphify. Only skip graphify if the task is about stale or incorrect graph output, or the user explicitly says not to use it.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).


## Build & verify
- Lint gate (CI): `cargo clippy --all --all-targets --locked -- -D warnings` — run this, not plain clippy.
- clippy.toml sets `too-many-lines-threshold = 25` — every function ≤25 lines; decompose into helpers up front.
- HTTP tests use wiremock: `GithubClient::with_base(token, mock_uri)` is the GitHub test seam; `JiraClient` is mockable via `JiraCreds.site_url`. Pin query params/body in matchers (unmatched mock = silent 404).
- Tests: `cargo test --workspace`. Build: `cargo build --workspace`.
- `#[cfg(test)] mod tests` must be the LAST item in a file (clippy `items-after-test-module`).
- Public types with a `new()` need a `Default` impl too (clippy `new_without_default` fails under `-D warnings`).
- Workspace crates: `wf-core`, `wf-db`, `wf-github`, `wf-jira`, `wf-slack`, `wf-api`, `wf-sync`, `migration` (the `wf-` prefix avoids the std `core` clash).

## Architecture
- `wf-core` — config, AES-256-GCM `TokenCipher`, RFC 9457 problem, auth types (no actix/db deps).
- `wf-db` — one directory per table under `src/tables/<table>/`: `entity.rs` (SeaORM schema), `crud.rs` (typed inputs + all CRUD), `mod.rs` (glob-re-exports both → callers use one import `wf_db::tables::<table>`). `connect()` disables the statement cache.
- `wf-github` / `wf-jira` — `reqwest` clients + domain logic for each integration.
- `wf-slack` — `reqwest` Slack client + ingestion logic (messages → `slack_messages`/`slack_connections` tables, synced in `wf-sync::slack`).
- `wf-api` — actix-web bin: `AppState` (DI), `AuthUser` extractor (Supabase JWKS), routes, middleware. Entry: `crates/api/src/main.rs`.
    - Route modules under `src/`: `ai/` (OpenAI assists), `hub/` (board aggregation, inbox, runs), `slack/`, `jira/`, `github/`, `notes/`.
    - Observability: `crates/api/src/telemetry.rs` (OTLP traces+metrics+logs) + `middleware/request_tracing.rs` (root span, trace propagation, `http.server.*` metrics). `main()` holds a `TelemetryGuard` and calls `shutdown()` after the server stops.

### Event backbone (A1)
- `events` + `sync_state` tables (sea-orm-migration DDL). Apply: `cargo run -p migration -- up` (session pooler only — see Database note).
- Tick logic in `wf-sync` (`run_tick`): reconcile scopes → claim due rows (`FOR UPDATE SKIP LOCKED`) → poll → normalize → insert.
- Pollers fetch page-1 DESC + filter client-side against the compound cursor — do NOT switch to absolute JQL `updated >=` filters: Jira interprets JQL timestamps in the *account's* timezone (correctness trap; see A1 spec appendix).
- The tick runs from an **in-process scheduler** (`crates/api/src/scheduler.rs`, spawned in `main()`): every `TICK_SCHEDULER_SECS` (default 120), first run at boot, logs target `tick.scheduler`. There is no HTTP trigger and no `INTERNAL_TICK_TOKEN`.
- Poll config envs (all have defaults): `POLL_INTERVAL_SECS` (120), `TICK_BATCH_SIZE` (50), `TICK_BUDGET_MS` (30000), `TICK_LEASE_SECS` (90), `TICK_SCHEDULER_SECS` (120).
- Live smoke: `cargo run -p wf-sync --example tick_smoke` (needs `.env` + connected user).
- Gated integration tests: `DATABASE_URL=... cargo test -p wf-sync --test tick_db -- --test-threads=1`.

## Database (Supabase)
- `DATABASE_URL` must be the **session pooler** (`...pooler.supabase.com:5432`).
  The transaction pooler (6543) breaks SeaORM/sqlx with `42P05` even with `statement_cache_capacity(0)`; the direct host (`db.<ref>.supabase.co`) is IPv6-only.
- Raw SQL in sea-orm 2.0: `db.query_one_raw(stmt)` / `query_all_raw` (the generic `query_one` is for query-builders).
- `sync_state.cursor`: `CURSOR` is a Postgres reserved word — always write it quoted (`"cursor"`) in raw SQL/DDL (SeaORM entities quote automatically).
- `events.payload` keys `issueKey`/`statusId` are load-bearing: queried by SQL (`payload->>'issueKey'`) in `events/crud.rs` — renaming them in `wf-sync::normalize` breaks the Jira prev-status lookup.

## Env & running
- `.env` is auto-loaded via dotenvy: `cargo run -p wf-api` works without sourcing. `.env` is gitignored.
- `OPENAI_API_KEY` powers the AI assists (`gpt-5-mini` via `async-openai`); when absent the AI endpoints no-op rather than erroring (`core/config.rs`, `api/src/ai/`).
- Live smoke harnesses (need `.env` + real data): `cargo run -p wf-db --example {phase0,gh_validate,gh_repo,gh_dashboard,gh_repo_write}`.
- Web client: `../workflow` (React 19 + TanStack Router/Query + Mantine, Orval-generated client). Sync API types there: `yarn api:spec && yarn api:gen`. Its gates are `yarn type`/`lint`/`build` — it has NO test framework; don't add one.

## Dependency feature gotchas
- `jsonwebtoken` → `features=["rust_crypto"]` (else runtime "CryptoProvider" panic).
- `reqwest` → `query` feature for `RequestBuilder::query`; TLS feature is `rustls`.
- `sqlx` → `runtime-tokio` + `tls-rustls-ring`. `getrandom` → `fill()` (not `getrandom()`).
- `Uuid::new_v4()` needs an explicit `uuid = { version = "1", features = ["v4"] }` — sea-orm's re-export doesn't enable `v4`.
- OpenTelemetry is 0.32: use `SdkTracerProvider` / `Resource::builder()` / `with_batch_exporter(exporter)` (no runtime arg). `opentelemetry-otlp` exports over **gRPC** (`grpc-tonic`) to the collector on `:4317` (`.with_tonic()`, base endpoint, no `/v1/*` path). tonic needs a Tokio runtime — the `#[actix_web::main]` entrypoint provides it — but the default batch processor still needs no `rt-tokio` SDK feature.

## Deployment (Cloud Run)
- Two-container service (app + OTel Collector sidecar). Files: `Dockerfile`, `cloudbuild.yaml`, `service.yaml`, `otel-config.yaml`, `deploy/terraform/`. Full guide: `DEPLOYMENT.md`. Patterns ported from the read-only reference repo `../otlp`.
- `main()` connects to the DB *before* binding, so a container won't serve `/healthz` without a reachable `DATABASE_URL` (Secret Manager in prod).
- Toolchain pinned via `rust-toolchain.toml` (stable) so local/CI/Docker agree — avoids the clippy `E0514` toolchain-mismatch class.

## Conventions
- Response DTOs: `#[serde(rename_all = "camelCase")]`; timestamps ISO8601 millis+Z (`to_rfc3339_opts(SecondsFormat::Millis, true)`).
- Errors: return `AppError` → RFC 9457 `application/problem+json` (carries `instance`/`reason`).
- DB writes: every `ActiveModel` is built inside that table's `crud.rs` (the only construction site); callers pass a typed input struct (e.g. `UpsertPatInput`) and never touch `ActiveModel`. New table → new `src/tables/<table>/` with the same `entity.rs`/`crud.rs`/`mod.rs` trio.
- Tooling: prefer Serena MCP for code edits/reads; context7 for library docs.
