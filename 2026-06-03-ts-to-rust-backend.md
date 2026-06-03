# Backend Migration: TypeScript (Elysia/Effect) → Rust (actix-web/SeaORM)

**Status:** Proposed · **Date:** 2026-06-03 · **Scope:** `apps/server` only (the web app stays React/Vite)

---

## 1. Summary

`apps/server` is a Bun + Elysia + Effect + Drizzle service that authenticates Supabase
users and proxies GitHub (via PAT) and Jira (via API token) on their behalf. This document
specifies a **full rewrite of that service in Rust**, preserving every HTTP endpoint, the
database schema, the auth model, and the encryption-at-rest of third-party tokens.

### Locked decisions

| Decision | Choice | Rationale |
| --- | --- | --- |
| **Web ↔ API type safety** | **OpenAPI codegen** | Rust emits an OpenAPI 3.1 spec (`utoipa`); the web app generates a typed client in CI. Replaces the Eden Treaty `typeof _app` coupling, which a Rust server breaks. |
| **Migration strategy** | **Big-bang rewrite** | Build full parity on a branch, cut over in one deploy. No dual-run proxy, no per-route shimming. The ~48-endpoint surface is small enough to rewrite wholesale. |
| **Web framework** | **actix-web** | Mature, high-throughput, stable extractor model. |
| **ORM / DB** | **SeaORM** | Async ORM over SQLx; entity model maps cleanly to the three existing tables. |
| **GitHub client** | **Raw `reqwest`** | Mirror the existing Jira `fetch` wrapper: a thin typed client over GitHub REST + the raw GraphQL dashboard query. One HTTP style for both integrations. |
| **Jira client** | **Raw `reqwest`** | Direct port of the current `fetch` Basic-auth client. |
| **Async runtime** | **Tokio** (multi-thread) | Required by actix-web, SeaORM, reqwest. |

### Goals

- 100% endpoint parity: same paths, methods, request/response JSON shapes, status codes.
- Same Supabase JWT auth (JWKS verification, `authenticated` audience).
- Same AES-256-GCM token encryption format — **decrypt existing DB rows without re-linking**.
- Same database schema (no destructive migration; reuse the existing tables/columns).
- Same RFC 9457 Problem Details error envelope.
- A published OpenAPI spec + a generated, fully-typed web client.

### Non-goals

- No web app rewrite (beyond swapping the API client).
- No schema redesign, no new features, no endpoint changes.
- No change to Supabase as the identity provider or to the PAT/API-token linking model.

---

## 2. What exists today (source of truth)

```
apps/server/src
  core/        auth (JWKS), config (env schema), crypto (AES-GCM), http (run/problem glue),
               logger (logtape), problem (RFC 9457)
  db/          client (bun:sql + Drizzle), schema (3 tables)
  domain/      todos (in-memory demo), users (upsert-from-auth)
  github/      octokit client, pat/ (validate+store+cache), dashboard/ (GraphQL+REST enrich),
               activity/ (workflow dispatch, PR create/merge/close), routes/
  jira/        client (fetch Basic-auth), pat/, issues/ (ADF, JQL, fields, mappers), routes/
  index.ts     Elysia app composition + ManagedRuntime wiring
  runtime.ts   AppServicesT / AppRuntimeT (Effect service union)
```

Runtime composition (`index.ts`): one Elysia app with prefix `/api`, CORS, request/response
logging hooks, a global error handler, then `.use()` of nine route plugins. All business logic
runs inside an Effect `ManagedRuntime` holding five services: `TodosService`, `UsersService`,
`TokenVerifier`, `GithubPatService`, `JiraPatService`.

**Database:** three tables on Supabase Postgres — `users`, `github_pat_connections`,
`jira_pat_connections`. RLS is **enabled with zero policies** on the two connection tables; the
backend connects as `postgres` and bypasses RLS. The connection string is the **transaction
pooler** (port 6543), which rejects named prepared statements.

**Web coupling:** `apps/web/src/lib/api.ts` does `treaty<AppT>(...)` where `AppT = typeof _app`
is imported from `server`, plus subpath type imports (`server/github-dashboard-types`,
`server/github-activity-types`, `server/jira-types`). **All of this is deleted** and replaced by
a generated client (§13).

---

## 3. Target architecture

### 3.1 Workspace layout

A Cargo workspace under `apps/server` (replacing the TS sources). The web app is untouched.

```
apps/server
  Cargo.toml                # workspace
  crates/
    api/                    # binary: actix-web app, routes, middleware, OpenAPI doc
      src/
        main.rs             # bootstrap: config → db pool → AppState → HttpServer
        state.rs            # AppState (db, http clients, caches, config, jwks)
        openapi.rs          # utoipa::OpenApi aggregator + /api/openapi.json
        middleware/
          auth.rs           # Bearer extraction + Supabase JWKS verify → AuthedUser
          request_log.rs    # tracing span per request
          problem.rs        # error → RFC 9457 response
        routes/
          health.rs         # /health, /hello/:name
          me.rs             # /me (upsert-from-auth)
          github/{pat.rs, data.rs, actions.rs, runs.rs, favorites.rs}
          jira/{pat.rs, data.rs, actions.rs}
    core/                   # lib: config, crypto, problem, auth types, error enums
    db/                     # lib: SeaORM entities + repositories
    github/                 # lib: reqwest client, REST DTOs, GraphQL dashboard query, validate
    jira/                   # lib: reqwest Basic-auth client, ADF, JQL, field mappers
  entity/                   # SeaORM generated entities (or under db/)
```

Crate boundaries mirror the current folder domains so the rewrite is a module-for-module port.

### 3.2 Dependency-injection model (replacing Effect `ManagedRuntime`)

Effect's `Context.Tag` + `Layer` + `ManagedRuntime` is replaced by a single **`AppState`**
struct stored in actix `Data<AppState>` and pulled into handlers by extractor:

```rust
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: DatabaseConnection,          // SeaORM (cloneable, pooled)
    pub jwks: Arc<JwksVerifier>,         // cached Supabase JWKS
    pub cipher: Arc<TokenCipher>,        // AES-256-GCM
    pub github: GithubClientFactory,     // builds a reqwest client per PAT
    pub jira: JiraClientFactory,         // builds a reqwest client per creds
    pub token_cache: Arc<TokenCache>,    // decrypted PAT, 60s TTL
    pub dashboard_cache: Arc<DashboardCache>, // SWR
}
```

Each "service" becomes a module of free functions / a small struct that takes `&AppState`
(or the specific deps it needs). No global mutable state; `Arc` for shared, `DatabaseConnection`
is internally pooled and cheaply cloneable.

### 3.3 Error handling (replacing Effect typed errors)

Effect's `Data.TaggedError` channel becomes per-domain `enum` errors with `thiserror`, each
implementing actix's `ResponseError` to emit an RFC 9457 body (§9):

```rust
#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("unauthorized: {0}")] Auth(String),
    #[error(transparent)] Db(#[from] DbErr),
    #[error(transparent)] Github(#[from] GithubError),
    #[error(transparent)] Jira(#[from] JiraError),
    #[error("validation: {0}")] Validation(String),
    #[error("internal")] Internal(#[source] anyhow::Error),
}
```

Handlers return `Result<Json<T>, AppError>`; the `ResponseError` impl maps each variant to the
correct status + Problem body, preserving today's mapping (auth → 401, validation → 400, GitHub
connection failures → the `reason` extension member, etc.).

---

## 4. Crate dependencies (Cargo)

| Concern | Crate | Notes |
| --- | --- | --- |
| HTTP server | `actix-web` 4 | + `actix-cors` for CORS |
| Async runtime | `tokio` (rt-multi-thread, macros) | |
| ORM | `sea-orm` (runtime-tokio-rustls, postgres) | + `sea-orm-migration` if we generate migrations; **we reuse existing SQL** |
| HTTP client | `reqwest` (json, rustls-tls) | GitHub + Jira |
| JWT | `jsonwebtoken` 9 | RS256/ES256/EdDSA/HS256 verification |
| JWKS fetch+cache | `reqwest` + `moka` (or hand-rolled `RwLock<HashMap>` with TTL) | replaces `jose` `createRemoteJWKSet` |
| Crypto | `aes-gcm` + `base64` | AES-256-GCM, 12-byte IV — **format-compatible with `node:crypto`** |
| Serialization | `serde`, `serde_json` | |
| OpenAPI | `utoipa` + `utoipa-swagger-ui` (optional) | `#[derive(ToSchema)]` on DTOs, `#[utoipa::path]` on handlers |
| Config | `serde` + `envy` (or `figment`) | typed env loading, replaces Effect `Schema` |
| Logging/tracing | `tracing` + `tracing-subscriber` | + `tracing-opentelemetry` + `opentelemetry-otlp` for the existing OTEL export |
| Errors | `thiserror`, `anyhow` | |
| Time | `chrono` or `time` | timestamps with tz |
| YAML (workflow inputs parsing) | `serde_yaml` | replaces `yaml` package in `github/workflows/inputs.ts` |

**`aes-gcm` compatibility note:** Node's `aes-256-gcm` with a 12-byte IV and a 16-byte auth tag
is interoperable with the `aes-gcm` crate's `Aes256Gcm`. The DB stores `ciphertext`, `iv`,
`authTag` as separate base64 columns; the Rust `open()` must base64-decode each, set the IV
(nonce) and append/verify the tag exactly as `node:crypto` produced it. Round-trip this against
a real row in a test before cutover (§16).

---

## 5. Configuration & environment

Port `core/config.ts` (Effect `Schema`) to a typed `Config` loaded with `envy`. Same variable
names, same defaults, same validation intent (fail fast at boot).

| Env var | Type | Default | Notes |
| --- | --- | --- | --- |
| `PORT` | u16 | `3000` | |
| `CORS_ORIGINS` | CSV → `Vec<String>` | `["http://localhost:5173"]` | split/trim/filter empty |
| `NODE_ENV` | enum | `development` | `development\|production\|test` |
| `LOG_LEVEL` | enum | `info` | maps to `tracing` filter |
| `OTEL_SERVICE_NAME` | String | `workflow-server` | |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Option<String> | — | enables OTEL when set |
| `DATABASE_URL` | String | (required) | **transaction pooler** — see §6 |
| `SUPABASE_URL` | String | (required) | JWKS + issuer base |
| `SUPABASE_JWT_AUDIENCE` | String | `authenticated` | |
| `WEB_APP_URL` | String | `http://localhost:5173` | |
| `GITHUB_TOKEN_ENCRYPTION_KEY` | String (base64, 32 bytes) | (required) | decode → must be exactly 32 bytes or panic at boot |

CSV parsing and the base64-key length check are explicit; replicate the current "decode to 32
bytes or throw" behavior as a boot-time `expect`.

---

## 6. Database layer (SeaORM)

### 6.1 Connection — the Supabase pooler constraint (critical)

`DATABASE_URL` points at Supabase's **transaction pooler on port 6543**, which does **not**
support named prepared statements. The TS code handles this with `new SQL(url, { prepare: false })`.

SeaORM sits on SQLx, whose Postgres driver uses the extended/prepared protocol and a
statement cache by default. **You must disable statement caching** or named prepares will fail
with `42P05 prepared statement already exists`:

```rust
let mut opt = ConnectOptions::new(config.database_url.clone());
opt.sqlx_logging(false)
   .max_connections(10)
   .min_connections(1);
// Disable the prepared-statement cache for the transaction pooler.
// On sqlx this is `.statement_cache_capacity(0)` via PgConnectOptions;
// with SeaORM, construct PgConnectOptions explicitly and pass it to
// SqlxPostgresConnector, OR append `?statement_cache_capacity=0` / use the
// simple-query path. VERIFY against the pooler before cutover.
let db = Database::connect(opt).await?;
```

> ⚠️ This is the single highest-risk infra detail. Confirm the exact SeaORM/SQLx incantation
> that yields zero named prepares against the 6543 pooler **first**, with a smoke query, before
> porting any business logic. Alternative fallback: connect to the **session pooler / direct
> 5432** if prepared statements there are acceptable for the workload.

### 6.2 RLS

`github_pat_connections` and `jira_pat_connections` have **RLS enabled, zero policies**, by
design — the backend connects as `postgres` and bypasses RLS; clients never touch Postgres
directly. **Do not add policies.** The `rls_enabled_no_policy` advisor on these tables is
intentional. No migration is needed for the Rust port; reuse the live schema.

### 6.3 Entities

Generate SeaORM entities from the live DB (`sea-orm-cli generate entity`) or hand-write them to
match `db/schema.ts`. The three tables:

**`users`**
| column | type | notes |
| --- | --- | --- |
| `id` | uuid | PK (Supabase user id) |
| `email` | text | not null |
| `name` | text | nullable |
| `avatar_url` | text | nullable |
| `created_at` | timestamptz | default now |
| `updated_at` | timestamptz | default now |

**`github_pat_connections`** (PK `user_id` → `users.id` ON DELETE CASCADE)
| column | type | notes |
| --- | --- | --- |
| `user_id` | uuid | PK / FK |
| `github_user_id` | bigint | not null |
| `github_login` | text | not null |
| `access_token_ciphertext` / `access_token_iv` / `access_token_auth_tag` | text | AES-GCM sealed PAT |
| `token_kind` | text | `classic_pat\|fine_grained_pat\|unknown`, default `unknown` |
| `scope` | text | comma-joined scopes, nullable |
| `permissions` | jsonb | nullable |
| `selected_repos` | jsonb | `Vec<String>`, nullable |
| `favorite_workflows` | jsonb | `HashMap<String, Vec<i64>>`, nullable |
| `dashboard_snapshot` | jsonb | `{ tab, data }` SWR snapshot, nullable |
| `last_four` | text | nullable |
| `expires_at` / `last_validated_at` / `last_used_at` | timestamptz | nullable |
| `validation_status` | text | default `unknown` |
| `validation_error` | text | nullable |
| `created_at` / `updated_at` | timestamptz | default now |

**`jira_pat_connections`** (PK `user_id` → `users.id` ON DELETE CASCADE)
| column | type | notes |
| --- | --- | --- |
| `user_id` | uuid | PK / FK |
| `site_url` | text | not null |
| `cloud_id` | text | nullable |
| `account_id` / `email` / `display_name` | text | not null |
| `api_token_ciphertext` / `api_token_iv` / `api_token_auth_tag` | text | AES-GCM sealed token |
| `selected_projects` | jsonb | `Vec<String>`, nullable |
| `last_four` | text | nullable |
| `last_validated_at` / `last_used_at` | timestamptz | nullable |
| `validation_status` | text | default `unknown` |
| `validation_error` | text | nullable |
| `created_at` / `updated_at` | timestamptz | default now |

### 6.4 Repository operations to port

From `github/pat/account.ts`, `jira/pat/account.ts`, `domain/users.ts`:

- `users`: `upsert_from_auth(authed)` — insert-or-update `(email, name, avatar_url, updated_at)` keyed on `id`, return the row (the `/me` response).
- `github_pat_connections`: select-by-user, **upsert** (insert … on conflict `user_id` do update, omitting `created_at`, preserving `last_used_at`), mark-validation, set-selected-repos (also nulls `dashboard_snapshot`), get/set favorites, set/clear `dashboard_snapshot`, touch `last_used_at`, delete.
- `jira_pat_connections`: the analogous set (connect/upsert, validate, set selected projects, delete).
- Cache-busting: `set`/`delete`/`set_selected_repos` must clear the in-memory token cache + dashboard cache for that user (§11).

---

## 7. Authentication — Supabase JWKS token verification

Port `core/auth.ts`. This is the security-critical path; replicate it exactly.

### 7.1 Flow

1. Extract `Authorization: Bearer <jwt>` (parse case-insensitive scheme; reject missing/empty → 401).
2. Verify the JWT against Supabase's **remote JWKS**:
   - JWKS URL: `<SUPABASE_URL>/auth/v1/.well-known/jwks.json` (strip a trailing slash from `SUPABASE_URL` first).
   - **Issuer:** `<SUPABASE_URL>/auth/v1`
   - **Audience:** `SUPABASE_JWT_AUDIENCE` (default `authenticated`)
   - **Algorithms:** `ES256`, `RS256`, `EdDSA`, `HS256` (select per-token by the JWK `kid`/`alg`).
3. Decode claims → `AuthedUser`:
   - `id` ← `sub` (required, non-empty)
   - `email` ← `email` (optional → `""`)
   - `name` ← `user_metadata.full_name` (string or null)
   - `avatar_url` ← `user_metadata.avatar_url` (string or null)
4. Any failure → `AppError::Auth` → 401 Problem (`unauthorized`).

### 7.2 JWKS caching

`jose`'s `createRemoteJWKSet` fetches and caches keys with background refresh. Replicate with a
`JwksVerifier`:

- Fetch the JWKS once on first use; cache parsed `DecodingKey`s by `kid`.
- On an unknown `kid`, refetch (handles Supabase key rotation), then verify.
- TTL/refresh: cache for e.g. 10 minutes, with a forced refetch on `kid` miss. Use `moka` or an
  `RwLock<HashMap<String, DecodingKey>>` + an `Instant` stamp.

```rust
pub struct AuthedUser {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}
```

### 7.3 actix extractor

Implement `FromRequest` for `AuthedUser` so any handler can take it as an argument and get
auth-or-401 for free (replaces the repeated `authedUserEffect(authorization)` prelude in every
Effect handler). `/me` additionally upserts the user row from the verified claims.

---

## 8. Crypto — AES-256-GCM token sealing

Direct port of `core/crypto.ts`. **Format must be byte-compatible** so existing sealed rows
decrypt unchanged.

- Algorithm: `aes-256-gcm`; key = base64-decode `GITHUB_TOKEN_ENCRYPTION_KEY` (exactly 32 bytes).
- `seal(plaintext) -> { ciphertext, iv, auth_tag }`: random **12-byte** IV; GCM encrypt; return all three base64-encoded.
- `open({ ciphertext, iv, auth_tag }) -> plaintext`: base64-decode each; GCM decrypt with the supplied IV; verify tag.
- The same `TokenCipher` seals **both** GitHub PATs and Jira API tokens (Jira reuses the GitHub key — keep that).

```rust
// aes-gcm crate: Aes256Gcm, 96-bit nonce. The ciphertext column holds ONLY the
// ciphertext (no appended tag); the tag lives in its own column. So use
// `aes_gcm` low-level: encrypt_in_place_detached / decrypt_in_place_detached,
// keeping ciphertext and tag separate to match the Node layout.
```

> ⚠️ The `aes-gcm` crate's high-level `encrypt` **appends** the tag to the ciphertext, but the DB
> keeps `ciphertext` and `auth_tag` in separate columns. Use the **detached** API so the column
> layout stays identical to what `node:crypto` wrote.

---

## 9. Error handling — RFC 9457 Problem Details

Port `core/problem.ts` + `core/http.ts` mapping. Every error response is
`application/problem+json` with:

```json
{ "type": "https://docs.workflow.app/problems/<slug>", "title": "...",
  "status": 4xx|5xx, "detail": "...", "instance": "/api/...", "reason": "<optional>" }
```

- `type` = `https://docs.workflow.app/problems/{slug}`.
- `reason` is an **extension member** carrying the PAT/Jira validation status when relevant
  (e.g. `needs_sso`, `missing_permissions`). Preserve it.
- Status/slug mapping to replicate:
  - 400 → `bad-request` ("Bad Request"); validation/parse failures.
  - 401 → `unauthorized` ("Unauthorized"); from auth failures (the `/me` path emits this).
  - 404 → `not-found` ("Not Found").
  - 500 → `internal-error` ("Internal Server Error"); detail is always the generic string (never leak internals); the real cause is logged, not returned.
- Connection vs data failures: GitHub/Jira connect routes map known validation states to a
  specific status + `reason` (port `github/routes/helpers.ts` `handleConnectionFailure` /
  `handleDataFailure` and the Jira equivalents).
- Logging: `status >= 500` logs at error; `4xx` logs at warn — keep this split.

Implement via `ResponseError` on `AppError`, plus a fallback actix error handler for
framework-level 400/404 (validation/route-not-found) that emits the same envelope.

---

## 10. External integrations

### 10.1 GitHub (raw `reqwest`)

Replaces Octokit. Two transports, both `reqwest`:

**REST** — base `https://api.github.com`, headers: `Authorization: Bearer <pat>`,
`User-Agent: workflow-server`, `Accept: application/vnd.github+json`,
`X-GitHub-Api-Version: 2022-11-28`. Endpoints used today (port each as a typed method returning
a `serde` DTO):

- `GET /user` (validate identity; read `x-oauth-scopes` and `github-authentication-token-expiration` response headers).
- `GET /search/issues` (validation probe only — see throttle note).
- `GET /repos/{owner}/{repo}/pulls/{n}`, `…/pulls/{n}/reviews`.
- `GET /repos/{owner}/{repo}/actions/runs?head_sha=…`, `…/commits/{sha}/status`, `…/commits/{sha}/check-runs`, `…/branches/{branch}/protection/required_status_checks`.
- `POST …/pulls` (create), `PUT …/pulls/{n}/merge` (merge), `PATCH …/pulls/{n}` (close → `state:"closed"`).
- `POST …/actions/workflows/{id}/dispatches`, `GET …/actions/workflows`, `…/workflows/{id}/runs`, `GET …/contents/{path}` (workflow inputs YAML → `serde_yaml`), `GET …/branches`, `GET …/environments`, `GET …/repos` (repo listing).

**GraphQL** — POST `https://api.github.com/graphql` with the dashboard query. **This is
mandatory, not optional:** the REST `/search/issues` endpoint is throttled (Octokit serialized
it to 1 concurrent / 2s each) and `advanced_search` reads repeated `repo:` as AND, undercounting.
The current code aliases all dashboard searches into **one** GraphQL request
(`search(query, type:ISSUE){ issueCount nodes{ ...PullRequest } }`) which bypasses the throttle
and uses OR semantics. Port `github/dashboard/graphql.ts` + `queries.ts` verbatim as a single
GraphQL POST with aliased `search` fields; keep REST only for per-PR enrichment and repo listing.

**Rate limiting / retries:** port the Octokit throttle behavior (retry on primary rate-limit up
to 2×, honor secondary limits) as reqwest middleware or a small retry wrapper.

**Validation classifier** (`github/pat/validate.ts`): detect token kind by prefix
(`github_pat_` → fine-grained, `ghp_` → classic); map HTTP 401 → `invalid`,
403 + `x-github-sso` header → `needs_sso`, 403 → `missing_permissions`, 404 →
`missing_permissions`; "best effort" enrichment probes where only 401/403 invalidate. Reproduce
this state machine exactly — the web app keys connection UI off these statuses.

### 10.2 Jira (raw `reqwest`)

Direct port of `jira/client.ts`:

- Basic auth: `Authorization: Basic base64(email:token)`.
- Bound to one validated `site_url` origin; build URLs with the path + query.
- `redirect: "manual"` equivalent: configure reqwest with a **no-follow redirect policy**;
  treat any 3xx/opaque-redirect as `502 Unexpected Jira redirect` (prevents credential replay
  to a redirect target — keep this defense).
- Non-2xx → typed `JiraApiError { status, message, error_messages }` (parse Jira's
  `errorMessages[]`).
- Methods: `get/post/put/del`. Port the ADF builder (`issues/adf.ts`), JQL builder
  (`issues/jql.ts`), field mappers (`issues/mappers.ts`, `fields.ts`), status (`status.ts`),
  and site-URL normalization (`site-url.ts`) — these have existing `bun:test` suites; mirror
  them as Rust unit tests.

---

## 11. Caching

Two in-memory caches, both per-process (no Redis today — keep it that way):

- **Token cache** (`github/pat/token-cache.ts`): decrypted PAT cached for **60s** keyed by user,
  to avoid a DB read + AES decrypt on every request. Port as `Arc<Moka<Uuid, (String, Instant)>>`
  or `RwLock<HashMap>`. **Invalidate on connect/disconnect/repo-change.** Note: the audit found
  some activity routes bypass this cache (DB+decrypt per call); the Rust port should route **all**
  PAT reads through the cache for consistency.
- **Dashboard cache** (`github/pat/dashboard-cache.ts`): stale-while-revalidate. Plus a durable
  `dashboard_snapshot` jsonb column for cold-start SWR (server restart / first load). Port both
  the in-memory SWR and the snapshot read/write; invalidate the snapshot on repo-selection change.

A Jira token cache is not present today; match current behavior (decrypt per request) unless we
choose to add one — out of scope for parity.

---

## 12. Observability

Port `core/logger.ts` (logtape) → `tracing`:

- Structured logs; `info` → stdout, `warn`/`error` → stderr (match current routing if the deploy
  relies on it).
- Per-request span (method, url, status) via middleware — replaces the `onRequest`/`onAfterResponse`
  Elysia hooks. **Put request logging in middleware/handlers, not in a fragile global hook.**
- Log keys: keep the existing dotted keys (`me.github.dashboard`, `me.jira.queue`, …) as span
  fields / event targets so existing log queries keep working.
- OTEL: when `OTEL_EXPORTER_OTLP_ENDPOINT` is set, wire `tracing-opentelemetry` +
  `opentelemetry-otlp` with `OTEL_SERVICE_NAME`. Mirrors `@logtape/otel`.

---

## 13. Web ↔ API contract: OpenAPI + generated client (replacing Eden Treaty)

This is the web-side half of the migration and **must ship in the same cutover**.

### 13.1 Server: emit OpenAPI

- Annotate every response/request DTO with `#[derive(utoipa::ToSchema)]`.
- Annotate every handler with `#[utoipa::path(...)]` (method, path, params, request body,
  responses incl. the Problem schema, security = bearer).
- Aggregate into one `#[derive(OpenApi)]` doc; serve it at **`GET /api/openapi.json`** (and
  optionally Swagger UI at `/api/docs` behind a dev flag).
- Define the security scheme once (HTTP bearer JWT) and apply to all `/me/**` routes.

### 13.2 Web: generate a typed client

- Remove `@elysiajs/eden`, the `treaty<AppT>` call, and all `server`/`server/*` type imports
  from `apps/web`.
- Add a codegen step (e.g. `openapi-typescript` for types, or `orval`/`openapi-fetch` for a
  typed fetch client) that reads `/api/openapi.json` and emits `apps/web/src/lib/api.gen.ts`.
- Re-implement the thin wrapper in `apps/web/src/lib/api.ts`: keep `buildAuthHeaders` (pull the
  Supabase access token, emit `Authorization: Bearer …`), but back it with the generated client
  instead of Eden. Keep the `ApiError` Effect wrapper shape so call sites barely change.
- Wire codegen into CI and `yarn dev` (generate against a running server or a committed spec
  snapshot). Treat the spec as the contract; a drift check in CI fails the build if the
  committed `openapi.json` and the server's emitted spec disagree.

### 13.3 Subpath type exports

The `server` package's `exports` (`./github-dashboard-types`, `./github-activity-types`,
`./jira-types`, `./todos`) disappear. Every web import of those becomes an import from the
generated client's component schemas. Inventory these imports in `apps/web` and migrate them
as part of the cutover PR.

---

## 14. Complete endpoint inventory (parity checklist)

All routes are served under the **`/api`** prefix. **Auth** = requires a valid Supabase Bearer
JWT. Query/body shapes below are the exact current contracts.

### 14.1 Base / info

| Method | Path | Auth | Input | Response |
| --- | --- | --- | --- | --- |
| GET | `/health` | no | — | `{ status:"ok", time:ISO8601 }` |
| GET | `/hello/:name` | no | path `name` | `{ greeting }` |
| GET | `/me` | **yes** | — | `{ id, email, name, avatarUrl, createdAt, updatedAt }` (upserts user row) |

> **`/todos` is dropped.** The two demo routes (`GET`/`POST /todos`) and the in-memory
> `TodosService` are **not** ported — they exist only as a scaffold example. Remove any web-side
> usage of them in the cutover PR. (`/hello/:name` is kept as a trivial liveness echo alongside
> `/health`; drop it too if unused.)

### 14.2 GitHub — connection (`github/routes/pat.ts`)

| Method | Path | Input | Response / notes |
| --- | --- | --- | --- |
| GET | `/me/github` | — | connection summary |
| POST | `/me/github/token` | `{ token }` | validates against GitHub then stores (AES-GCM); returns summary |
| POST | `/me/github/token/validate` | — | re-validates stored token; returns summary |
| DELETE | `/me/github` | — | `{ disconnected: true }`; deletes row + clears caches |

### 14.3 GitHub — data (`github/routes/data.ts`)

| Method | Path | Input | Notes |
| --- | --- | --- | --- |
| GET | `/me/github/dashboard` | query `tab?` (default `assigned`) | one GraphQL request; SWR + snapshot |
| GET | `/me/github/queue` | query `key` | single queue pull |
| GET | `/me/github/pull` | query `owner, repo, number` | per-PR enrichment |
| POST | `/me/github/pulls/enrich` | `{ refs: [{owner, repo, number}] }` | batch enrichment |
| GET | `/me/github/repos` | — | repo listing + current selection |
| PUT | `/me/github/repos` | `{ repos: string[] }` | set selection; nulls snapshot; clears caches |

### 14.4 GitHub — actions (`github/routes/actions.ts`, `actions-runs.ts`)

| Method | Path | Input |
| --- | --- | --- |
| GET | `/me/github/branches` | — (branch prompts across selected repos) |
| GET | `/me/github/workflows` | — (workflows across selected repos) |
| GET | `/me/github/workflow/inputs` | query `owner, repo, path` |
| GET | `/me/github/workflow/runs` | query `owner, repo, workflowId, branch` |
| GET | `/me/github/repo/branches` | query `owner, repo` |
| GET | `/me/github/repo/environments` | query `owner, repo` |
| POST | `/me/github/workflow/dispatch` | `{ owner, repo, workflowId, ref, inputs: {k:v} }` → `{ ok:true }` |
| POST | `/me/github/pulls` | `{ owner, repo, base, head, title, body? }` (create PR) |
| POST | `/me/github/pull/merge` | `{ owner, repo, number, method: merge\|squash\|rebase }` |
| POST | `/me/github/pull/close` | `{ owner, repo, number }` → `{ ok:true }` |

### 14.5 GitHub — favorites (`github/routes/favorites.ts`)

| Method | Path | Input |
| --- | --- | --- |
| GET | `/me/github/favorites` | — |
| PUT | `/me/github/favorites` | `{ repoFullName, workflowIds: number[] }` |

### 14.6 Jira — connection (`jira/routes/pat.ts`)

| Method | Path | Input | Notes |
| --- | --- | --- | --- |
| GET | `/me/jira` | — | connection summary |
| POST | `/me/jira/token` | `{ siteUrl, email, token }` | validate → store |
| POST | `/me/jira/token/validate` | — | re-validate |
| DELETE | `/me/jira` | — | `{ disconnected: true }` |
| PUT | `/me/jira/projects` | `{ projects: string[] }` | set selection |

### 14.7 Jira — data (`jira/routes/data.ts`)

| Method | Path | Input |
| --- | --- | --- |
| GET | `/me/jira/dashboard` | — |
| GET | `/me/jira/queue` | query `key, cursor?` |
| POST | `/me/jira/search` | `{ jql, cursor? }` |
| GET | `/me/jira/issue` | query `key` |
| GET | `/me/jira/projects` | — |
| GET | `/me/jira/issuetypes` | query `projectKey` |
| GET | `/me/jira/boards` | — |
| GET | `/me/jira/sprint/issues` | query `boardId` |
| GET | `/me/jira/issue/transitions` | query `key` |
| GET | `/me/jira/users` | query `query, issueKey?, projectKeyOrId?, actionDescriptorId?` |
| GET | `/me/jira/createmeta` | query `projectKey, issueTypeId` |
| GET | `/me/jira/editmeta` | query `key` |

### 14.8 Jira — actions (`jira/routes/actions.ts`)

| Method | Path | Input |
| --- | --- | --- |
| POST | `/me/jira/issue/transition` | `{ key, transitionId }` → `{ ok:true }` |
| POST | `/me/jira/issue/comment` | `{ key, body }` |
| POST | `/me/jira/issue/assign` | `{ key, accountId: string\|null }` → `{ ok:true }` |
| POST | `/me/jira/issue/worklog` | `{ key, timeSpent, started?, comment? }` → `{ ok:true }` |
| POST | `/me/jira/issue` | `{ projectKey, issueTypeId, fields: {} }` (create) |
| PUT | `/me/jira/issue` | `{ key, fields: {} }` (edit) → `{ ok:true }` |

**Total: 48 endpoints** to port (`/health`, `/hello/:name`, `/me`, and the 45 `/me/**`
GitHub + Jira routes). The two `/todos` demo routes are intentionally dropped. Numeric query params (`number`, `workflowId`, `boardId`) arrive as
strings and are parsed server-side — define them as `String` in OpenAPI query schemas and parse,
matching the current `Number()` handling. Numeric **body** params are real numbers.

---

## 15. Request lifecycle (target)

```
HTTP request
  → actix-cors (CORS_ORIGINS, credentials)
  → tracing span middleware (method, url)                    [replaces onRequest hook]
  → route match
  → AuthedUser extractor (for /me/**): Bearer → JWKS verify  [replaces authedUserEffect]
  → body/query extractor + validation                        [replaces TypeBox schemas]
  → handler(&AppState, AuthedUser, input) -> Result<Json<T>, AppError>
  → on Err: ResponseError → RFC 9457 problem+json            [replaces runRoute onFailure]
  → response span close (status)                             [replaces onAfterResponse hook]
```

---

## 16. Migration plan (big-bang)

A single cutover. Work proceeds on a branch; the TS server keeps running in prod until the
switch.

**Phase 0 — Infra spikes (de-risk first).**
1. SeaORM/SQLx connects to the **6543 transaction pooler** with zero named prepares (smoke query). §6.1
2. `aes-gcm` **decrypts a real `github_pat_connections` row** sealed by the TS server (detached tag, separate columns). §8
3. `jsonwebtoken` verifies a **real Supabase access token** against the live JWKS (issuer/audience/alg). §7

> Do not proceed past Phase 0 until all three pass. They are the only things a Rust rewrite can
> get subtly, silently wrong.

**Phase 1 — Skeleton.** Workspace, `AppState`, config loader, DB connect, tracing, CORS, the
`ResponseError`→Problem plumbing, `/health` + `/hello/:name`. Deployable, no auth yet.

**Phase 2 — Auth + users.** `AuthedUser` extractor, JWKS verifier, `/me` upsert. The token
cipher module + token cache.

**Phase 3 — GitHub.** reqwest REST client + DTOs, the GraphQL dashboard query, the validate
state machine, the PAT service + repository, all GitHub routes (connection, data, actions,
favorites). Dashboard SWR + snapshot.

**Phase 4 — Jira.** reqwest Basic client, ADF/JQL/fields/mappers (+ port the unit tests), the
Jira PAT service + routes (connection, data, actions).

**Phase 5 — OpenAPI + web client.** Finalize `utoipa` annotations; emit `/api/openapi.json`;
generate the web client; swap `apps/web/src/lib/api.ts`; remove Eden + `server` type imports;
CI drift check.

**Phase 6 — Parity verification.** Run the §14 checklist endpoint-by-endpoint against both
servers (same JWT, same DB) and diff JSON responses. Load-check the dashboard path.

**Phase 7 — Cutover.** Point the deploy/proxy at the Rust binary; keep the TS server warm for
fast rollback; monitor; decommission TS after a soak period.

### Rollback

Because the **schema and token-encryption format are unchanged**, the TS server can resume
serving the same DB at any point. Rollback = repoint traffic back to the TS process. No data
migration, no dual-write.

---

## 17. Testing strategy

- **Unit:** crypto round-trip + **decrypt-TS-sealed-row** vector; JWKS verify with a known token;
  config parsing; Jira ADF/JQL/field mappers (port existing `bun:test` cases); GitHub validation
  classifier (status→state mapping); Problem mapping.
- **Integration:** spin actix in-process; hit each route with a mock/real JWT; mock GitHub/Jira
  with `wiremock`. Assert status + JSON shape against the §14 contracts.
- **Contract:** snapshot `/api/openapi.json`; CI fails if it drifts; the generated web client must
  typecheck.
- **Parity (Phase 6):** scripted diff of TS vs Rust responses for representative authed requests.

---

## 18. Risks & mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| SQLx prepared statements vs 6543 pooler | Every query 500s | Phase 0 spike; disable statement cache or use session pooler/direct 5432 |
| AES-GCM tag layout mismatch (appended vs detached) | Can't decrypt existing tokens → all users must re-link | Phase 0 decrypt-real-row test; use detached API |
| JWKS alg/issuer/audience mismatch | All auth 401s | Phase 0 verify-real-token test; replicate all four algs |
| GitHub search throttle / AND-bug regression | Dashboard slow or undercounts | Keep the single-GraphQL approach; do **not** fall back to REST search |
| Jira credential replay on redirect | Security | reqwest no-follow policy; treat 3xx as 502 |
| OpenAPI drift from handlers | Web client lies about the API | CI spec-drift check; generated client must compile |
| Effect's exhaustive typed errors lost | Silent error swallowing | `thiserror` enums + `ResponseError`; deny `unwrap` in handlers via lint |
| Numeric query params as strings | 400s / parse bugs | Model query nums as `String` + parse, exactly as today |
| Behavioral drift in 48 endpoints | Subtle breakage | Phase 6 response-diff parity gate before cutover |

---

## 19. Open questions

1. ~~**`todos`** — keep or drop?~~ **Resolved: dropped.** Demo routes + `TodosService` are not ported.
2. **OpenAPI client generator** — `openapi-typescript` (types only) vs `orval`/`openapi-fetch`
   (types + runtime client)? **Deferred** — decide before Phase 5; does not block Phases 0–4.
3. **Deploy target** — where does the Rust binary run (same host/container as Bun today)? Affects
   the cutover/proxy step and the stdout/stderr log routing assumption.
4. **Connection pool sizing** — confirm acceptable `max_connections` against the Supabase pooler's
   own limits.
5. **Jira token cache** — add one (GitHub has a 60s cache; Jira doesn't), or keep decrypt-per-request
   for strict parity? (Default: parity — no new cache.)
```
