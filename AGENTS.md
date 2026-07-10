# Workflow backend

## Load canary and user address

- If this file is loaded at session start, begin your first response with exactly:
  `AGENTS.md loaded — hello Mr. Twinkle.`
- Address the user as `Mr. Twinkle` in the first sentence of every substantive response.

## Required tool contract

The required tools are:

- Graphify
- Serena
- Sequential Thinking
- Context7

Use all four tools on every non-trivial repository task: planning, debugging, architecture analysis, dependency/API work, refactors, multi-file edits, migrations, or unfamiliar code.

For trivial single-file or text-only tasks, still check whether the required tools are available and use the relevant tool or tools. Do not fake a tool call.

If any required tool is unavailable, disconnected, denied, or failing, stop before editing and report:

1. which tool is missing or failing,
2. what it would have been used for,
3. the exact setup, permission, or approval needed,
4. whether Mr. Twinkle wants to continue without that tool.

Never claim a tool was used unless it was actually called through MCP, a native client integration, slash command, or CLI command.

Prefer MCP/native tool calls over shell fallbacks. Use shell commands only when the tool is installed as a CLI/slash command and no MCP tool is available.

## Default task flow

### 1. Plan with Sequential Thinking

- Use Sequential Thinking at the beginning of every non-trivial task.
- Use revision or branching when assumptions change.
- Use a final verification step before the final answer.
- Do not reveal private chain-of-thought. Share concise conclusions, decisions, and next actions instead.

### 2. Load project context with Serena

- Activate the current repository/project first.
- Read Serena initial instructions.
- If onboarding has not been done, run onboarding before deeper work.
- Read relevant Serena memories before making changes.
- Write a Serena memory after discovering stable project conventions, important architecture, or recurring commands.
- Prefer Serena’s symbol-aware tools for code navigation and edits.

Preferred Serena sequence:

1. `activate_project` for the current repo.
2. `initial_instructions`.
3. `onboarding` if not already performed.
4. `get_symbols_overview` before reading large source files.
5. `find_symbol`, `find_referencing_symbols`, or `find_implementations` before modifying code.
6. `get_diagnostics_for_file` or other diagnostics after edits.
7. `write_memory` for stable, reusable project findings.

Avoid repeated plain grep/read-file loops when Serena symbol tools can answer the question.

### 3. Build or query architecture context with Graphify

Use Graphify for:

- unfamiliar repositories,
- architecture questions,
- cross-file or cross-domain relationships,
- design rationale in docs, PDFs, diagrams, and other non-code artifacts,
- identifying central modules, communities, and surprising connections.

Before broad file search or architecture explanations, check whether a Graphify graph/report exists.

If the graph is missing or stale, build or update Graphify before relying on it.

Preferred Graphify commands when available:

- `/graphify` or `graphify .` to build the graph for the current repo.
- `/graphify <repo-path> --update` to refresh changed files.
- `/graphify query "<question>"` to answer architecture or dependency-flow questions.
- `/graphify path "<entity A>" "<entity B>"` to inspect relationships.
- `/graphify explain "<entity>"` to explain a module, class, subsystem, or concept.
- `graphify hook install` when Mr. Twinkle wants the graph kept fresh after commits and branch switches.
- The built graph lives in `graphify-out/` at repo root — check there before rebuilding.

Validate graph-derived conclusions against source files before editing.

### 4. Ground external/library knowledge with Context7

Use Context7 before writing or changing code that depends on:

- external libraries,
- APIs,
- frameworks,
- cloud SDKs,
- build tools,
- config formats,
- test frameworks,
- generated boilerplate,
- migrations or deprecations.

Preferred Context7 sequence:

1. Identify the library/framework and version from the repo.
2. Use `resolve-library-id` when the exact Context7 ID is unknown.
3. Use `query-docs` with a focused query and the exact library ID.
4. Prefer Context7 docs over model memory for APIs and configuration.
5. Mention the library ID used when the answer depends on it.

If Context7 has no relevant docs, say so and fall back to official docs or repository source.

### 5. Implement with minimal, verified changes

- Inspect existing conventions before editing.
- Prefer small, targeted edits over broad rewrites.
- Use Serena for symbol-aware edits and refactors where possible.
- Use Graphify for cross-module impact checks.
- Use Context7 before touching external API/library usage.
- Run the narrowest relevant tests/checks first, then broader checks when risk warrants.
- Report what changed, which required tools were used, and what verification passed or could not be run.

## Tool-specific rules

### Serena: semantic code operations

Use Serena for project activation, onboarding, memories, symbol lookup, references, implementation discovery, diagnostics, safe refactors, targeted symbol-body edits, insertion near symbols, and renames.

Do not modify a symbol-heavy source file until you have used Serena to inspect its symbol overview or locate the relevant symbol.

### Graphify: repository knowledge graph

Use Graphify before relying on raw search for architecture or cross-module questions.

When Graphify and source files disagree, trust the source files and update the graph.

Use Graphify outputs as navigation and hypothesis support, not as the sole source of truth for edits.

### Context7: current external documentation

Use Context7 for dependency-aware coding. Do not rely on model memory for external APIs when Context7 can provide version-specific documentation.

When the repo pins a version, query docs for that version.

### Sequential Thinking: structured reasoning

Use Sequential Thinking to break work into steps, revise the plan when evidence changes, branch alternatives when needed, and verify the solution hypothesis before finalizing.

Summarize outcomes, not hidden reasoning.

## Client-specific notes

### Codex

- Codex should load this file as project guidance when it is named `AGENTS.md` in the repository root or another loaded instruction location.
- MCP servers must still be configured in Codex. This file tells Codex how to use them, but does not install them.
- In Codex, check active MCP servers with `/mcp` when available.
- If Codex MCP config supports required servers, mark required MCP servers as required so startup fails instead of silently running without them.

### Claude Code

- `CLAUDE.md` is a **symlink to this file** (`AGENTS.md`) — editing either edits both. Claude Code loads it via that symlink.
- MCP tools must be connected and permitted. If Claude can see a tool but lacks approval to call it, ask Mr. Twinkle to approve/allow the tool instead of silently continuing without it.
- Use hooks for hard enforcement when behavior must happen at a lifecycle point, such as Graphify before broad search or Serena activation at session start.

## Final response checklist

Before responding:

- Confirm whether each required tool was used or explain why it could not be used.
- Summarize code changes and files touched.
- Summarize verification/tests.
- Call out risks, assumptions, and follow-up work.
- Address Mr. Twinkle by name in the first sentence.
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
- Poll config envs (all have defaults): `POLL_INTERVAL_SECS` (120), `TICK_BATCH_SIZE` (50), `TICK_BUDGET_MS` (30000), `TICK_LEASE_SECS` (90), `TICK_CONCURRENCY` (4), `TICK_SCHEDULER_SECS` (120).
- Within a tick, claimed scopes poll concurrently via `buffer_unordered(TICK_CONCURRENCY)` (`tick::run_tick`); each scope briefly borrows a DB connection, so keep `TICK_CONCURRENCY` well under the pool size (10).
- Outbound HTTP goes through **`wf-http`** (`crates/http`): `wf_http::shared()` / `shared_no_redirect()` return one process-wide `HttpClient` (a `reqwest-middleware` `ClientWithMiddleware` over a pooled `reqwest::Client`), so keep-alive sockets + TLS sessions are reused across every call site. `GithubClient`/`SlackClient` use `shared()`; `JiraClient` uses `shared_no_redirect()` (Basic creds must never replay to a redirect). Build requests from the stored `wf_http::HttpClient` — never add `reqwest::Client::builder()` to a provider path. `RequestBuilder` is `reqwest_middleware::RequestBuilder` (its `.query()`/`.json()` need the `query`/`json` features, enabled in the workspace dep).
- OTLP: `wf-http` installs `reqwest_tracing::TracingMiddleware`, emitting an OTEL **client** span per outbound request that flows through the existing `tracing`→OTLP pipeline (`telemetry.rs`). Version-locked via `reqwest-tracing`'s `opentelemetry_0_32` feature (matches `opentelemetry` 0.32 / `tracing-opentelemetry` 0.33). `auth.rs`'s JWKS fetcher still builds its own `reqwest::Client` (singleton; not yet on `wf-http`).
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
- Web client: `../workflow` (React 19 + TanStack Router/Query + Mantine, Orval-generated client). Sync API types there: `yarn api:spec && yarn api:gen`. Its gates are `yarn type`/`lint`/`build`/`test`.

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
