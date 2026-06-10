# Workflow backend — complete server reference

This document is the exhaustive inventory of what the server has and does: every crate, route, table, environment variable, background process, cache, and operational procedure. The companion quick-start lives in the root [`README.md`](../README.md).

- **Spec (source of truth for the port):** [`2026-06-03-ts-to-rust-backend.md`](../2026-06-03-ts-to-rust-backend.md)
- **Execution checklist:** [`PLAN.md`](../PLAN.md)
- **Deploy guide:** [`DEPLOYMENT.md`](../DEPLOYMENT.md)
- **Machine-readable API:** `GET /api/openapi.json` (OpenAPI 3.1, generated code-first with utoipa)

---

## 1. Overview

`wf` is the Rust rewrite of the Workflow TypeScript/Bun backend (`../workflow/apps/server`). It is an actix-web HTTP service that:

1. authenticates users via **Supabase JWTs** (ES256, verified against the project JWKS),
2. stores **GitHub PATs** and **Jira Cloud API tokens** sealed with AES-256-GCM,
3. proxies and aggregates GitHub + Jira data into dashboard/queue/detail endpoints with the exact JSON contract of the original TS server (48 operations),
4. runs a **poll-driven event backbone** ("A1"): a tick routine that polls each connected user's GitHub/Jira scopes on a schedule and normalizes activity into an `events` table,
5. exports **traces, metrics, and logs** via OpenTelemetry OTLP (gRPC) to a collector sidecar.

Deployment target: **Google Cloud Run** (project `workflow-497713`, region `europe-west1`), two containers per service (app + OTel Collector), DB on **Supabase Postgres**.

---

## 2. System architecture

```
                          ┌────────────────────────────── Cloud Run service ──────────────────────────────┐
 Browser (web app)        │  ┌──────────────── wf-api ────────────────┐   ┌─────────────────────────┐     │
 ──Bearer JWT──► /api/** ─┼─►│ CORS → RequestTracing middleware        │   │ OTel Collector sidecar  │     │
                          │  │  → AuthUser extractor (JWKS verify)     │──►│ gRPC OTLP :4317         │──►  │ Cloud Trace /
 Cloud Scheduler          │  │  → route handler                        │   └─────────────────────────┘     │ Monitoring /
 ──X-Internal-Token──►    │  │      ├─ wf-db (SeaORM → Supabase 5432)  │                                   │ Logging
   POST /internal/tick ───┼─►│      ├─ wf-github (REST + GraphQL)      │                                   │
                          │  │      ├─ wf-jira (Jira Cloud REST)       │                                   │
 Cloud Run probes ──────► │  │      └─ wf-sync (tick engine)           │                                   │
   GET /healthz           │  └─────────────────────────────────────────┘                                   │
                          └────────────────────────────────────────────────────────────────────────────────┘
```

Boot order in `crates/api/src/main.rs`: load `.env` (dotenvy) → parse `Config` (fail fast) → init telemetry → validate the 32-byte encryption key → **connect to the DB before binding** (a container never serves `/healthz` without a reachable `DATABASE_URL`) → build `AppState` → bind `0.0.0.0:$PORT` → on shutdown, flush telemetry via `TelemetryGuard::shutdown()`.

### `AppState` (dependency container, `crates/api/src/state.rs`)

| Field | Type | Role |
|---|---|---|
| `config` | `Arc<Config>` | Typed env config (see §6) |
| `db` | `Db` | SeaORM connection (statement cache disabled) |
| `jwks` | `Arc<JwksVerifier>` | Supabase JWKS fetch + ES256 JWT verification |
| `cipher` | `Arc<TokenCipher>` | AES-256-GCM seal/open for stored tokens |
| `token_cache` | `Arc<TokenCache>` | In-memory cache of decrypted GitHub tokens |
| `dashboard_cache` | `Arc<DashboardCache>` | Stale-while-revalidate GitHub dashboard cache |

---

## 3. Workspace crates

Seven crates; the `wf-` prefix avoids the std `core` name clash. All opt into the workspace clippy lints (`too_many_lines` is enforced; `#[cfg(test)] mod tests` must be the **last** item in a file).

### 3.1 `wf-core` (`crates/core`) — pure domain, no actix/db deps

| Module | Contents |
|---|---|
| `config.rs` | `Config` struct + `Config::load()` / `from_map()` (unit-testable, fail-fast). `encryption_key_bytes()` enforces base64 → exactly 32 bytes at boot. |
| `crypto.rs` | `TokenCipher` — AES-256-GCM with 12-byte random IV and detached 16-byte auth tag; produces `Sealed { ciphertext, iv, auth_tag }`, each base64. Byte-compatible with the Node implementation (proven against a real DB row in Phase 0). |
| `problem.rs` | `ProblemDetails` — RFC 9457 `application/problem+json` envelope (`status`, `slug`, `title`, `detail`, optional `instance` and `reason`). |
| `auth.rs` | `AuthedUser { id, email, name, avatar_url }` (from Supabase `user_metadata`) + `AuthError`. |

### 3.2 `wf-db` (`crates/db`) — persistence

- `connect(url, options)` builds the SeaORM connection **with the statement cache disabled** (required for the Supabase pooler).
- Raw SQL in sea-orm 2.0: use `db.query_one_raw(stmt)` / `query_all_raw` (the generic `query_one` is for query-builders).
- **Table layout convention:** one directory per table under `src/tables/<table>/` containing `entity.rs` (SeaORM schema), `crud.rs` (typed input structs + all CRUD — the **only** place `ActiveModel`s are constructed), and `mod.rs` (glob re-exports both). Callers import `wf_db::tables::<table>` and never touch `ActiveModel`.
- Tables: `users`, `github_pat_connections`, `jira_pat_connections`, `events`, `sync_state` (schemas in §7).
- `examples/` — live smoke harnesses run with `cargo run -p wf-db --example <name>` (need `.env` + real data): `phase0`, `gh_validate`, `gh_repo`, `gh_dashboard`, `gh_repo_write`, `gh_activity`, `gh_favorites`, `gh_open_prs`, `gh_pr_enrich`, `gh_write_probe`, `jira_row`.

### 3.3 `wf-github` (`crates/github`) — GitHub client + domain logic

| Module | Contents |
|---|---|
| `client.rs` | `GithubClient` (reqwest, rustls) — REST + GraphQL, token auth, configurable base URL (test seam for wiremock). |
| `validate.rs` | PAT validation (identity, scopes/permissions, expiry). |
| `errors.rs` | Error taxonomy mapped to API `reason`s. |
| `types.rs` | Shared GitHub DTOs. |
| `poll.rs` | A1 polling: `PolledPullRequest`, `PolledWorkflowRun` fetchers used by the tick. |
| `dashboard/` | `query.rs` (search/list queries), `enrich.rs` (bounded-concurrency PR enrichment), `types.rs`. |
| `activity/` | `branches.rs` + `branches_graphql.rs` (branch → PR-prompt discovery; **GraphQL must stay single-repo per request** — batching repos triggers GitHub `RESOURCE_LIMITS_EXCEEDED` and silently returns empty), `pulls.rs`, `workflows.rs`, `environments.rs`, `inputs.rs` (workflow-dispatch input YAML parsing), `write.rs` (dispatch/create/merge/close), `types.rs`. |

### 3.4 `wf-jira` (`crates/jira`) — Jira Cloud client + domain logic

| Module | Contents |
|---|---|
| `client.rs` | `JiraClient` with `JiraCreds` (site URL + email + API token, Basic auth). |
| `site_url.rs` | Site-URL normalization/validation. |
| `validate.rs`, `status.rs`, `errors.rs`, `types.rs` | Credential validation, status/category mapping, error taxonomy, DTOs. |
| `poll.rs` | A1 polling for Jira issue activity. ⚠️ Jira **JQL timestamp predicates evaluate in the account's timezone, not UTC** — cursor logic accounts for this. |
| `issues/` | `search.rs` + `jql.rs` (JQL building/search), `reads.rs`, `writes.rs` (transition/comment/assign/worklog/create/edit), `dashboard.rs` (multi-queue), `mappers.rs`, `fields.rs`, `adf.rs` (Atlassian Document Format handling). |
| `examples/` | `jira_validate`, `jira_smoke` (live verification harnesses). |

### 3.5 `wf-api` (`crates/api`) — the HTTP binary

| Module | Contents |
|---|---|
| `main.rs` | Bootstrap (see §2), `/healthz`, framework-level 404 in problem+json. |
| `state.rs` | `AppState` (see §2). |
| `auth.rs` | `JwksVerifier` + `AuthUser` actix extractor: validates `Authorization: Bearer` against Supabase JWKS (ES256), checks audience. |
| `error.rs` | `AppError` → RFC 9457 responses with stable slugs + `reason`. |
| `dto.rs` | Shared response DTOs (`#[serde(rename_all = "camelCase")]`). |
| `routes/` | `health.rs` (`/health`, `/hello/{name}`), `me.rs` (`GET /me` — verifies JWT, upserts user), `events.rs` (`GET /me/events` — keyset-paged feed read), `internal.rs` (`POST /internal/tick`). |
| `github/` | `routes.rs` (22 routes), `pat.rs` (connect/validate/disconnect), `dashboard.rs` + `dashboard_cache.rs` (SWR snapshot cache; the dashboard route also sets **opportunistic sync priority hints** by marking the user's sync scopes overdue), `token_cache.rs`, `activity.rs`, `summary.rs`. |
| `jira/` | `routes.rs` (23 routes), `pat.rs`, `data.rs`, `actions.rs`, `summary.rs`. |
| `middleware/request_tracing.rs` | Root span per request, W3C trace-context propagation (continues Cloud Run's `traceparent`), `http.server.*` metrics. |
| `telemetry.rs` | OTel SDK init (traces + metrics + logs) + `TelemetryGuard` (see §11). |
| `openapi.rs` | utoipa aggregation → `GET /api/openapi.json` with `bearer` (JWT) security scheme. |

### 3.6 `wf-sync` (`crates/sync`) — event-backbone engine

A library so `wf-api` and a future `wf-worker` share identical logic.

| Module | Contents |
|---|---|
| `tick.rs` | `run_tick(db, cipher, options)` → `TickSummary { scopesClaimed, scopesOk, scopesFailed, eventsWritten }`. `TickOptions { batch, budget, lease_secs, poll_interval_secs, owner, github_base }` (owner is `api-<uuid-v4>`; `github_base` is the wiremock test seam). |
| `cursor.rs` | `GithubCursor` / `JiraCursor` — per-scope incremental cursors (serialized into `sync_state.cursor`). |
| `normalize.rs` | Maps polled GitHub PRs/workflow runs and Jira issue activity into normalized `events` rows (incl. PR-event timestamp fallback and Jira event-kind classification). |
| `examples/tick_smoke.rs` | Live harness. `tests/tick_db.rs` — env-gated integration tests (see §14). |

### 3.7 `migration` (`crates/migration`) — DDL

sea-orm-migration binary: `m0001_create_events`, `m0002_create_sync_state`. Apply with `cargo run -p migration -- up` — **session pooler only** (§7.1). The three legacy tables (`users`, `*_pat_connections`) predate this repo and are managed in Supabase directly.

---

## 4. HTTP API — complete route catalog

Conventions (all routes): responses are camelCase JSON; timestamps are ISO-8601 millisecond UTC (`2026-06-10T06:00:00.000Z`); errors are RFC 9457 `application/problem+json`. Exact request/response schemas: `GET /api/openapi.json` (42 paths / 49 operations / 82 schemas).

### 4.1 Root-level (not under `/api`, not in the OpenAPI spec)

| Method & path | Auth | Purpose |
|---|---|---|
| `GET /healthz` | none | Cloud Run liveness/startup probe. Plain-text `ok`; **no I/O** (never blocks on the DB pool). |
| `POST /internal/tick` | `X-Internal-Token: $INTERNAL_TICK_TOKEN` | Runs one sync tick (§8). Returns `TickSummary` JSON for Scheduler logs. Empty configured token ⇒ deny-all. |

### 4.2 System + user (`/api`)

| Method & path | Auth | Purpose |
|---|---|---|
| `GET /api/health` | none | `{ status: "ok", time: ISO8601 }`. |
| `GET /api/hello/{name}` | none | Echo: `{ greeting: "Hello, <name>" }`. |
| `GET /api/me` | Bearer JWT | Verifies the Supabase JWT, **upserts the user row**, returns the user profile. |
| `GET /api/openapi.json` | none | The OpenAPI 3.1 document. |

Anything else → `404` problem+json with `detail: "NOT_FOUND"` and the request path as `instance`.

### 4.3 GitHub (`/api/me/github/**`, all Bearer JWT) — 22 routes

Connection lifecycle:

| Method & path | Purpose |
|---|---|
| `GET /api/me/github` | Connection status summary (login, scopes, validation state, selected repos, last-four — never the token). |
| `POST /api/me/github/token` | Connect: validate a PAT against GitHub, seal it (AES-256-GCM), upsert the connection row. |
| `POST /api/me/github/token/validate` | Re-validate the stored PAT; updates `validation_status` / `validation_error`. |
| `DELETE /api/me/github` | Disconnect: delete the connection row. |

Reads:

| Method & path | Purpose |
|---|---|
| `GET /api/me/github/dashboard` | The PR dashboard. Serves the persisted snapshot **stale-while-revalidate** (in-memory + `dashboard_snapshot` jsonb), revalidates in the background, and opportunistically marks the user's sync scopes overdue (priority hint for the next tick). |
| `GET /api/me/github/queue` | Work queue view. |
| `GET /api/me/github/repos` | Repos selectable for the dashboard. |
| `GET /api/me/github/pull` | Single PR detail. |
| `POST /api/me/github/pulls/enrich` | Batch-enrich PRs (reviews, checks, mergeability) with bounded concurrency. |
| `GET /api/me/github/branches` | Branch → "open a PR" prompts across selected repos (GraphQL, one repo per request — see §3.3; branches whose PRs merged are filtered out). |
| `GET /api/me/github/workflows` | Workflows across selected repos. |
| `GET /api/me/github/workflow/inputs` | Parses a workflow file's `workflow_dispatch` inputs (YAML). |
| `GET /api/me/github/workflow/runs` | Recent runs for a workflow. |
| `GET /api/me/github/repo/branches` | Branches of one repo (e.g. PR base/head pickers). |
| `GET /api/me/github/repo/environments` | Environments of one repo. |
| `GET /api/me/github/favorites` | Favorite workflows. |

Writes:

| Method & path | Purpose |
|---|---|
| `PUT /api/me/github/repos` | Set selected repos (`selected_repos` jsonb). |
| `PUT /api/me/github/favorites` | Set favorite workflows (`favorite_workflows` jsonb). |
| `POST /api/me/github/workflow/dispatch` | Dispatch a workflow run with inputs. |
| `POST /api/me/github/pulls` | Create a pull request. |
| `POST /api/me/github/pull/merge` | Merge a pull request. |
| `POST /api/me/github/pull/close` | Close a pull request. |

### 4.4 Jira (`/api/me/jira/**`, all Bearer JWT) — 23 routes

Connection lifecycle:

| Method & path | Purpose |
|---|---|
| `GET /api/me/jira` | Connection status summary (site, account, validation state, selected projects — never the token). |
| `POST /api/me/jira/token` | Connect: validate site URL + email + API token against Jira Cloud, seal the token, upsert. |
| `POST /api/me/jira/token/validate` | Re-validate stored credentials. |
| `DELETE /api/me/jira` | Disconnect. |
| `PUT /api/me/jira/projects` | Set selected projects (`selected_projects` jsonb). |

Reads:

| Method & path | Purpose |
|---|---|
| `GET /api/me/jira/dashboard` | Multi-queue dashboard (assigned / mentioned / recent etc.). |
| `GET /api/me/jira/queue` | Page a single queue. |
| `POST /api/me/jira/search` | JQL search. |
| `GET /api/me/jira/issue` | Issue detail (ADF body rendered/mapped). |
| `GET /api/me/jira/projects` | Browse projects. |
| `GET /api/me/jira/issuetypes` | Issue types for a project. |
| `GET /api/me/jira/boards` | Boards. |
| `GET /api/me/jira/sprint/issues` | Issues in a sprint. |
| `GET /api/me/jira/issue/transitions` | Available transitions for an issue. |
| `GET /api/me/jira/users` | Assignable user lookup. |
| `GET /api/me/jira/createmeta` | Create-screen field metadata. |
| `GET /api/me/jira/editmeta` | Edit-screen field metadata. |

Writes:

| Method & path | Purpose |
|---|---|
| `POST /api/me/jira/issue/transition` | Transition an issue. |
| `POST /api/me/jira/issue/comment` | Add a comment. |
| `POST /api/me/jira/issue/assign` | Assign an issue. |
| `POST /api/me/jira/issue/worklog` | Log work. |
| `POST /api/me/jira/issue` | Create an issue. |
| `PUT /api/me/jira/issue` | Edit an issue. |

### 4.5 Activity feed (`/api/me/events`, Bearer JWT) — 1 route

| Method & path | Purpose |
|---|---|
| `GET /api/me/events` | The user's normalized events, newest-first, **keyset-paginated over `events.id`** (`before` cursor ← response `nextBefore`; `limit` 1–100, default 50). Filters: `source` (`github`/`jira`), `typePrefix` (literal prefix match — LIKE wildcards `%`/`_`/`\` are escaped in `wf-db`), `scopeKey`. Backs the web client's `/activity` page. Contract pinned by a wire-key test (`type`/`occurredAt`, not `eventType`). |

---

## 5. Authentication & security

- **User auth:** every `/api/me/**` handler takes the `AuthUser` extractor, which verifies the `Authorization: Bearer` JWT against the Supabase project's **JWKS** (ES256) and checks `aud` (`SUPABASE_JWT_AUDIENCE`, default `authenticated`). The JWT's `sub` is the `users.id` (uuid).
- **Token storage:** GitHub PATs and Jira API tokens are sealed with **AES-256-GCM** (`TokenCipher`): 12-byte random IV, detached 16-byte auth tag, each part base64-encoded into its own DB column (`*_ciphertext`, `*_iv`, `*_auth_tag`). The key is `GITHUB_TOKEN_ENCRYPTION_KEY` (base64 of exactly 32 bytes — boot fails otherwise). Plaintext tokens are never logged or returned; status endpoints expose only `last_four`.
- **Internal auth:** `POST /internal/tick` compares `X-Internal-Token` to `INTERNAL_TICK_TOKEN` with `==`. The prefix-match timing leak is **documented and accepted for v1** (HTTPS, low-value oracle); OIDC between Scheduler and Cloud Run is the documented hardening path. An empty configured token denies all requests.
- **CORS:** allow-listed origins from `CORS_ORIGINS` (CSV), credentials supported, any method/header.
- **TLS/crypto stack:** rustls everywhere (reqwest, sqlx `tls-rustls-ring`); JWT via `jsonwebtoken` with the `aws_lc_rs` crypto provider feature (a provider feature is mandatory — without one, verification panics at runtime).

---

## 6. Configuration — environment variables

Parsed fail-fast at boot by `wf_core::Config` (`crates/core/src/config.rs`). Empty strings are treated as absent. `.env` is auto-loaded via dotenvy (real env vars win); `.env` is gitignored.

| Variable | Required | Default | Meaning |
|---|---|---|---|
| `DATABASE_URL` | ✅ | — | Supabase Postgres. **Must be the session pooler** `...pooler.supabase.com:5432` (§7.1). |
| `SUPABASE_URL` | ✅ | — | `https://<project>.supabase.co` — JWKS source. |
| `GITHUB_TOKEN_ENCRYPTION_KEY` | ✅ | — | Base64 of exactly 32 bytes; AES-256-GCM key for **all** stored tokens (GitHub + Jira). |
| `INTERNAL_TICK_TOKEN` | ✅ | — | Shared secret for `POST /internal/tick`. |
| `PORT` | | `3000` | Listen port (positive integer). |
| `CORS_ORIGINS` | | `http://localhost:5173` | CSV of allowed origins. |
| `NODE_ENV` | | `development` | `development` \| `production` \| `test`. |
| `LOG_LEVEL` | | `info` | `trace`\|`debug`\|`info`\|`warning`\|`error`\|`fatal` → tracing env-filter. |
| `SUPABASE_JWT_AUDIENCE` | | `authenticated` | Expected JWT `aud`. |
| `WEB_APP_URL` | | `http://localhost:5173` | Web app base URL (links in responses). |
| `OTEL_SERVICE_NAME` | | `workflow-server` | OTel resource service name. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | | unset | gRPC OTLP collector endpoint (e.g. `http://localhost:4317`). **Unset ⇒ no exporters built; logs pretty-print to stdout.** |
| `POLL_INTERVAL_SECS` | | `120` | Per-scope re-poll interval (must be > 0, like all four tick vars). |
| `TICK_BATCH_SIZE` | | `50` | Max scopes claimed per tick. |
| `TICK_BUDGET_MS` | | `30000` | Wall-clock budget per tick. |
| `TICK_LEASE_SECS` | | `90` | Scope lease duration; expired leases are reclaimable. |

(The `.env` file also carries non-server values used by tooling/harnesses: `DATABASE_PASSWORD`, `GOOGLE_CLIENT_ID`/`GOOGLE_CLIENT_SECRET`.)

---

## 7. Database (Supabase Postgres)

### 7.1 Connection rules (hard-won)

- Use the **session pooler** (`...pooler.supabase.com:5432`). The transaction pooler (6543) breaks SeaORM/sqlx with `42P05 prepared statement already exists` even with `statement_cache_capacity(0)`; the direct host (`db.<ref>.supabase.co`) is IPv6-only.
- `wf_db::connect()` disables the statement cache.
- Migrations (`cargo run -p migration -- up`) must also use the session pooler.
- `sync_state.cursor` is a **reserved word** — SeaORM quotes it automatically; hand-written SQL must quote `"cursor"` too.

### 7.2 Schema

**`users`** — Supabase user mirror, upserted by `GET /api/me`.

| Column | Type | Notes |
|---|---|---|
| `id` | uuid PK | The Supabase auth user id (not auto-generated). |
| `email` | text | |
| `name`, `avatar_url` | text, nullable | From JWT `user_metadata`. |
| `created_at`, `updated_at` | timestamptz | |

**`github_pat_connections`** — one per user (PK `user_id`, FK → users ON DELETE CASCADE).

| Column | Type | Notes |
|---|---|---|
| `user_id` | uuid PK/FK | |
| `github_user_id` | bigint | |
| `github_login` | text | |
| `access_token_ciphertext` / `_iv` / `_auth_tag` | text | Sealed PAT (§5). |
| `token_kind` | text | classic vs fine-grained. |
| `scope` | text, nullable | Classic-PAT scopes. |
| `permissions` | jsonb, nullable | Fine-grained permissions. |
| `selected_repos` | jsonb, nullable | Dashboard repo selection. |
| `favorite_workflows` | jsonb, nullable | |
| `dashboard_snapshot` | jsonb, nullable | Persisted SWR snapshot (§9). |
| `last_four` | text, nullable | Display-only token suffix. |
| `expires_at`, `last_validated_at`, `last_used_at` | timestamptz, nullable | |
| `validation_status` | text | + `validation_error` (nullable). |
| `created_at`, `updated_at` | timestamptz | |

**`jira_pat_connections`** — same pattern (PK `user_id`, cascade).

| Column | Type | Notes |
|---|---|---|
| `user_id` | uuid PK/FK | |
| `site_url` | text | Normalized Jira Cloud site. |
| `cloud_id` | text, nullable | |
| `account_id`, `email`, `display_name` | text | The Jira identity. |
| `api_token_ciphertext` / `_iv` / `_auth_tag` | text | Sealed API token. |
| `selected_projects` | jsonb, nullable | |
| `last_four`, `last_validated_at`, `last_used_at`, `validation_status`, `validation_error`, `created_at`, `updated_at` | | As above. |

**`events`** (migration `m0001`) — the normalized activity stream.

| Column | Type | Notes |
|---|---|---|
| `id` | bigint identity PK | **The feed cursor** (keyset pagination key). |
| `user_id` | uuid | Owner of the feed item. |
| `source` | text | `github` \| `jira`. |
| `type` | text | Event kind (Rust field is `event_type`; `type` is a keyword). |
| `external_id` | text | Dedup key per scope (insert is idempotent). |
| `scope_key` | text | e.g. repo full-name or Jira project. |
| `actor`, `title`, `url` | text, nullable | Display fields. |
| `occurred_at` | timestamptz | When it happened at the source. |
| `payload` | jsonb | Full normalized payload. |
| `ingested_at` | timestamptz | When the tick wrote it. |

**`sync_state`** (migration `m0002`) — one row per pollable scope; composite PK `(user_id, source, scope_key, entity_kind)`.

| Column | Type | Notes |
|---|---|---|
| `cursor` | text, nullable | Serialized `GithubCursor`/`JiraCursor` (reserved word — quote it). |
| `last_polled_at` | timestamptz, nullable | |
| `next_poll_at` | timestamptz | Due-time; claiming selects rows `next_poll_at <= now()`. |
| `consecutive_errors` | int | Drives exponential backoff. |
| `last_error` | text, nullable | |
| `lease_owner` | text, nullable | `api-<uuid-v4>` of the claiming tick. |
| `lease_until` | timestamptz, nullable | Lease expiry (`TICK_LEASE_SECS`). |

---

## 8. Event backbone (A1) — the tick

`wf_sync::run_tick` (shared by `POST /internal/tick` and any future worker):

1. **Reconcile scopes** — derive the set of pollable scopes from current connections (selected repos × entity kinds for GitHub; selected projects for Jira) and upsert/remove `sync_state` rows.
2. **Claim due rows** — `SELECT ... FOR UPDATE SKIP LOCKED` on rows with `next_poll_at <= now()` and no live lease, up to `TICK_BATCH_SIZE`; stamp `lease_owner`/`lease_until`. Safe to run concurrently.
3. **Poll each scope** — GitHub PRs / workflow runs, Jira issue activity, from the stored cursor (Jira cursors compensate for JQL evaluating timestamps in the **account timezone**, not UTC).
4. **Normalize + insert** — map to `events` rows; inserts are idempotent on `(user_id, source, scope_key, external_id)`-style dedup, so re-polls never duplicate.
5. **Advance or back off** — success: store new cursor, `next_poll_at = now() + POLL_INTERVAL_SECS`, clear errors. Failure: increment `consecutive_errors`, exponential backoff, record `last_error`. The whole tick stops when `TICK_BUDGET_MS` is exhausted; abandoned leases expire and are reclaimed by a later tick.

**Scheduling:** Cloud Scheduler job `wf-tick` POSTs `/internal/tick` every 2 minutes with the `X-Internal-Token` header (setup commands in `DEPLOYMENT.md`). The GitHub dashboard route additionally marks that user's scopes overdue, so an active user's next tick refreshes them first.

**Verification tooling:** `cargo run -p wf-sync --example tick_smoke` (live); `DATABASE_URL=... cargo test -p wf-sync --test tick_db -- --test-threads=1` (gated integration tests using wiremock for GitHub — note they redirect GitHub calls for **real scopes** in that DB, so point them at a dev database).

---

## 9. Caching

- **`TokenCache`** (`crates/api/src/github/token_cache.rs`) — in-memory cache of decrypted GitHub tokens, avoiding a DB read + AES open per request.
- **`DashboardCache`** (`crates/api/src/github/dashboard_cache.rs`) — stale-while-revalidate for the GitHub dashboard: requests get the last snapshot immediately (also persisted in `github_pat_connections.dashboard_snapshot` so it survives restarts) while a background refresh runs; the response indicates staleness so the web app can re-fetch.

Both are process-local (`Arc` in `AppState`); Cloud Run scale-out means per-instance caches, which is acceptable because the DB snapshot is the durable layer.

---

## 10. Errors & response conventions

- Handlers return `AppError`, rendered as RFC 9457 `application/problem+json`: `{ status, slug, title, detail, instance?, reason? }`. Stable slugs and `reason` values preserve the TS contract (verified by `scripts/parity.py`).
- Framework-level 404s use the same envelope with `detail: "NOT_FOUND"`.
- Success DTOs: `#[serde(rename_all = "camelCase")]`; timestamps via `to_rfc3339_opts(SecondsFormat::Millis, true)` → `...T...Z` with milliseconds (matches JS `Date#toISOString()`).

---

## 11. Observability

- **Stack:** OpenTelemetry 0.32 (`SdkTracerProvider`, `Resource::builder()`, batch exporters). Export is **gRPC OTLP** (`grpc-tonic`) to `OTEL_EXPORTER_OTLP_ENDPOINT` (base endpoint, no `/v1/*` path) — on Cloud Run that's the collector sidecar on `localhost:4317`.
- **Signals:** traces (root span per request via `RequestTracing`, continuing inbound `traceparent`), metrics (`http.server.*`, incl. `http_server_request_duration_seconds` with custom histogram buckets), logs (`tracing` events bridged via `opentelemetry-appender-tracing`).
- **Local:** with `OTEL_EXPORTER_OTLP_ENDPOINT` unset, no exporters are built and logs pretty-print to stdout.
- **Cloud:** the sidecar (config in `otel-config.yaml`, stored in Secret Manager) fans out to Cloud Trace, Cloud Monitoring (Managed Prometheus, "workflow-backend — RED" dashboard, latency SLO/alerts via Terraform `enable_alerts=true`), and Cloud Logging.
- `main()` holds a `TelemetryGuard` and calls `shutdown()` after the server stops so pending batches flush.

---

## 12. Deployment (summary — full guide in `DEPLOYMENT.md`)

- **Cloud Run two-container service** (`service.yaml`): `wf-api` + OTel Collector sidecar. Probes hit `/healthz`.
- **Build:** `Dockerfile` (multi-stage, cargo-chef + sccache, slim runtime **with `ca-certificates`**); `cloudbuild.yaml` builds, pushes, renders `service.yaml`, and `gcloud run services replace`s.
- **Secrets (Secret Manager):** `wf-database-url`, `wf-github-token-encryption-key`, `wf-internal-tick-token` — Terraform creates the containers, you add the values once. Non-secret config (`SUPABASE_URL`, `CORS_ORIGINS`, `WEB_APP_URL`, `SUPABASE_JWT_AUDIENCE`) comes from Cloud Build substitutions.
- **Terraform (`deploy/terraform/`):** APIs, Artifact Registry, secrets, IAM, sccache bucket, monitoring dashboard + SLO/alerts. All resources are `wf-`named to avoid colliding with the sibling `otlp` reference repo in the same project.
- **Toolchain:** pinned in `rust-toolchain.toml` so local/CI/Docker agree (avoids clippy `E0514` mismatches).
- **Outstanding ops step (as of 2026-06-10):** after the next deploy, add the `wf-internal-tick-token` secret value and create the `wf-tick` Cloud Scheduler job (commands in `DEPLOYMENT.md` §Tick scheduling).

---

## 13. Local development

```bash
cargo run -p wf-api                      # .env auto-loaded; http://localhost:3000
cargo run -p migration -- up             # apply DDL (session pooler!)
```

Live harnesses (need `.env` + a connected user):

| Command | Verifies |
|---|---|
| `cargo run -p wf-db --example phase0` | DB connect + crypto round-trip on a real row. |
| `cargo run -p wf-db --example gh_validate` / `gh_repo` / `gh_dashboard` / `gh_repo_write` / `gh_activity` / `gh_favorites` / `gh_open_prs` / `gh_pr_enrich` / `gh_write_probe` | Each GitHub feature slice against the live API. |
| `cargo run -p wf-db --example jira_row` · `cargo run -p wf-jira --example jira_validate` / `jira_smoke` | Jira row access / credential validation / live smoke. |
| `cargo run -p wf-sync --example tick_smoke` | One live tick end-to-end. |
| `python3 scripts/parity.py` | Response parity against the running TS server (needs both servers + a JWT). |

---

## 14. Testing & quality gates

- **Unit/integration tests:** `cargo test --workspace` (~121 test functions, inline `#[cfg(test)] mod tests` as the last item per file).
- **Gated DB tests:** `DATABASE_URL=... cargo test -p wf-sync --test tick_db -- --test-threads=1` — real Postgres + wiremock GitHub; single-threaded because they share `sync_state`.
- **Lint gate (CI):** `cargo clippy --all --all-targets --locked -- -D warnings`. Notable enforced lints: `too_many_lines` (threshold in `clippy.toml`), `items-after-test-module`, `new_without_default` (public types with `new()` need `Default`).
- **Definition of done** per change (from `PLAN.md`): faithful port from the TS source → tests green → clippy gate clean → live-verify where possible → one focused commit.

---

## 15. Gotchas (dependency & platform)

- `jsonwebtoken` needs an explicit crypto-provider feature (`aws_lc_rs`) or verification panics at runtime ("CryptoProvider").
- `reqwest`: TLS feature is `rustls`; `RequestBuilder::query` needs the `query` feature.
- `sqlx`: `runtime-tokio` + `tls-rustls-ring`. `getrandom`: use `fill()` (not `getrandom()`).
- OpenTelemetry 0.32 API: `SdkTracerProvider`, `Resource::builder()`, `with_batch_exporter(exporter)` (no runtime arg); tonic needs a Tokio runtime (provided by `#[actix_web::main]`); no `rt-tokio` SDK feature needed.
- GitHub branch-prompts GraphQL: **one repo per request** or GitHub returns `RESOURCE_LIMITS_EXCEEDED` and the query silently comes back empty.
- Jira JQL timestamp predicates evaluate in the **account timezone**, not UTC.
- Supabase: session pooler only (§7.1).

---

## 16. Status & roadmap

**Done:** full TS parity (48 original operations, OpenAPI at `/api/openapi.json`) · A1 event backbone shipped and live-verified · **B-lite** `GET /api/me/events` read endpoint (§4.5) · `/activity` feed UI in the web client (built per `docs/superpowers/plans/2026-06-10-activity-feed-ui.md`) · Cloud Run deploy pipeline with telemetry.

**Planned next (specs/plans in `docs/superpowers/`):**

| Item | What | Status |
|---|---|---|
| **B (full)** | SSE/live updates on top of the events feed. Contract pinned in `docs/superpowers/specs/2026-06-10-activity-feed-ui-design.md`. | Designed, not started. |
| **A2** | GitHub/Jira webhooks to reduce event latency (polling stays as backfill). | Designed, not started. |
| **C** | PR ↔ Jira-issue linking + rules. | Designed, not started. |
| Hardening | OIDC auth for Scheduler→`/internal/tick`; GitHub PAT validation currently hardcodes `validation_status: "valid"` on connect (known gap). | Backlog. |
