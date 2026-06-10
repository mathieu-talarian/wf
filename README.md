# Workflow backend (`wf`)

Rust (actix-web) backend for **Workflow** — a GitHub + Jira developer dashboard. This is a 1:1 port of the original TypeScript/Bun server (`../workflow/apps/server`), preserving the JSON contract endpoint-for-endpoint, plus a new poll-driven **event backbone** that the original never had.

**Start here:**
- [`docs/FUNCTIONAL.md`](docs/FUNCTIONAL.md) — **what the app does**, every endpoint described from the user's point of view (what you're trying to accomplish, what you send, what you get back).
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — the technical reference: crates, tables, env vars, deployment, operations.

## What the server does

- **Auth** — verifies Supabase-issued JWTs (ES256, via JWKS) on every `/api/me/**` route and upserts the user on first contact.
- **GitHub integration** — connect a personal access token (sealed with AES-256-GCM), then: PR dashboard with stale-while-revalidate caching, PR enrichment, branch/workflow/environment browsing, and write actions (dispatch workflow, create/merge/close PR, favorites).
- **Jira integration** — connect Jira Cloud credentials, then: multi-queue dashboard, JQL search, issue detail, and write actions (transition, comment, assign, worklog, create/edit issue).
- **Activity feed** — a `POST /internal/tick` endpoint (called by Cloud Scheduler every 2 min) polls GitHub/Jira for each connected user's scopes and normalizes activity into an `events` table (A1); `GET /api/me/events` serves it back as a filtered, cursor-paged feed (B-lite) powering the web app's `/activity` page.
- **Observability** — OpenTelemetry traces, metrics, and logs exported over gRPC OTLP to a collector sidecar on Cloud Run.

## Quickstart

```bash
# Prereqs: Rust (pinned by rust-toolchain.toml), a Supabase project, a .env file (see below).
cargo run -p wf-api          # starts on http://localhost:3000 (.env auto-loaded)

curl localhost:3000/api/health          # {"status":"ok","time":"..."}
curl localhost:3000/api/openapi.json    # full OpenAPI 3.1 spec (49 operations)
```

Minimal `.env` (full reference in [docs/ARCHITECTURE.md §6](docs/ARCHITECTURE.md#6-configuration--environment-variables)):

```bash
DATABASE_URL=postgres://...pooler.supabase.com:5432/postgres   # MUST be the session pooler (5432)
SUPABASE_URL=https://<project>.supabase.co
GITHUB_TOKEN_ENCRYPTION_KEY=<base64 of exactly 32 random bytes>
INTERNAL_TICK_TOKEN=<any secret string>
```

> ⚠️ `DATABASE_URL` must use the Supabase **session pooler** (`...pooler.supabase.com:5432`). The transaction pooler (6543) breaks SeaORM/sqlx with `42P05`; the direct host is IPv6-only.

## Build, test, lint

```bash
cargo build --workspace
cargo test --workspace                                          # ~135 unit/integration test fns
cargo clippy --all --all-targets --locked -- -D warnings        # the CI lint gate — run exactly this
cargo run -p migration -- up                                    # apply DB migrations (session pooler only)
```

Optional gated/live verification (need `.env` + real data):

```bash
DATABASE_URL=... cargo test -p wf-sync --test tick_db -- --test-threads=1   # DB-gated tick tests
cargo run -p wf-sync --example tick_smoke                                   # live tick against prod-shaped data
cargo run -p wf-db --example gh_dashboard                                   # live GitHub dashboard harness
python3 scripts/parity.py                                                   # TS-vs-Rust response parity diff
```

## Workspace layout

| Crate | Purpose |
|---|---|
| `crates/core` (`wf-core`) | Config, AES-256-GCM `TokenCipher`, RFC 9457 problem types, auth types. No actix/db deps. |
| `crates/db` (`wf-db`) | SeaORM connection + one directory per table (`entity.rs` / `crud.rs` / `mod.rs`). Live smoke examples. |
| `crates/github` (`wf-github`) | GitHub REST + GraphQL client, dashboard queries, PR enrichment, activity reads/writes, polling. |
| `crates/jira` (`wf-jira`) | Jira Cloud client, JQL/search, issue reads/writes, ADF handling, polling. |
| `crates/api` (`wf-api`) | The actix-web binary: routes, auth extractor, caches, middleware, telemetry, OpenAPI. |
| `crates/sync` (`wf-sync`) | Event-backbone engine: cursors, normalizers, the `run_tick` routine. |
| `crates/migration` | sea-orm-migration DDL for `events` + `sync_state`. |

Other key files:

- `2026-06-03-ts-to-rust-backend.md` — the migration spec (source of truth for the port).
- `PLAN.md` — execution checklist + status snapshot.
- `DEPLOYMENT.md` — Cloud Run deploy guide (two-container service with OTel Collector sidecar).
- `docs/superpowers/specs/` + `docs/superpowers/plans/` — feature specs/plans (currently: activity feed UI).
- `Dockerfile`, `cloudbuild.yaml`, `service.yaml`, `otel-config.yaml`, `deploy/terraform/` — deploy artifacts.
- `CLAUDE.md` — working conventions and dependency gotchas for AI-assisted development.

## API at a glance

All application routes live under `/api`; auth is `Authorization: Bearer <Supabase JWT>` except where noted.

| Area | Endpoints |
|---|---|
| System (public) | `GET /api/health`, `GET /api/hello/{name}`, `GET /healthz` (probe, root), `GET /api/openapi.json` |
| User | `GET /api/me` |
| GitHub | 22 routes under `/api/me/github/**` (connection, dashboard, repos, PRs, branches, workflows, environments, favorites) |
| Jira | 23 routes under `/api/me/jira/**` (connection, dashboard, queues, search, issue reads/writes, metadata) |
| Activity | `GET /api/me/events` (cursor-paged feed with `source` / `typePrefix` / `scopeKey` filters) |
| Internal | `POST /internal/tick` (root-level, `X-Internal-Token` auth, not in OpenAPI) |

The complete route catalog with request/response shapes is in [docs/ARCHITECTURE.md §4](docs/ARCHITECTURE.md#4-http-api--complete-route-catalog), and machine-readable at `/api/openapi.json`.

## Status & roadmap

Backend parity with the TS server is **complete** (all 48 original endpoints). The A1 event backbone is **deployed and live-verified**, the **B-lite** `GET /api/me/events` read endpoint is shipped, and the web client's `/activity` feed UI is built on it. Next increments:

- **B (full)** — SSE/live updates for the events feed.
- **A2** — GitHub/Jira webhooks to cut event latency.
- **C** — PR↔issue linking and rules.
