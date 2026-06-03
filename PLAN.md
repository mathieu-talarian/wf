# Execution plan — TS→Rust backend migration

Working checklist for the remaining migration, derived from `2026-06-03-ts-to-rust-backend.md` (the spec) and current progress. Conventions/gotchas live in `CLAUDE.md`.

## Definition of done (every chunk)
1. Port faithfully from the TS source-of-truth: `../workflow/apps/server/src/...`.
2. `cargo test --workspace` green.
3. Lint gate: `cargo clippy --all --all-targets --locked -- -D warnings` clean.
4. Live-verify where possible (a `wf-db` example against real DB/GitHub/Jira, or the route with a Supabase token).
5. One focused commit.

## Status snapshot
- ✅ Phase 0 spikes (DB session pooler, AES-GCM on real row, JWKS ES256)
- ✅ Phase 1 skeleton · ✅ Phase 2 auth + `/me`
- ✅ Phase 3a GitHub client + PAT validation · ✅ 3b connection flow · ✅ 3c dashboard (data + routes + SWR) · ✅ 3c.3 PR enrichment · ✅ 3d.1 activity reads
- ✅ Dependency upgrade (reqwest 0.13, jsonwebtoken 10, sqlx 0.9, sea-orm 2.0-rc, getrandom 0.4)
- **Endpoints done: 21 / 48** — `/health`, `/hello/:name`, `/me`, `/me/github` (4 conn), `/me/github/{dashboard,queue,repos}` GET + `repos` PUT, `/me/github/pull` GET + `/me/github/pulls/enrich` POST, `/me/github/{branches,workflows,workflow/inputs,workflow/runs,repo/branches,repo/environments}` GET.

---

## Phase 3 — GitHub (remaining)

### 3c.3 — PR enrichment  ✅ DONE
- **Source:** `github/dashboard/enrich.ts`, `api.ts` (`fetchDetail`/`fetchReviews`/`fetchLatestRun`), `checks.ts`, `readiness.ts`.
- **Built:** `GithubPullRequestEnrichment` DTO + enums (`dashboard/types.rs`); `dashboard/enrich.rs` — best-effort REST fetches (pull detail, reviews, latest run, combined status, check-runs, branch protection) with `futures::join!`; check normalize/select, approval-state, readiness-badge + blocker logic; `enrich_pull_request` + `enrich_pull_requests` (bounded concurrency, pool 8, `buffered` preserves order). Path segments percent-encoded (branch may contain `/`).
- **Endpoints:** `GET /me/github/pull` (owner,repo,number) + `POST /me/github/pulls/enrich` ({refs}) — both `resolve_pat` → "No GitHub token connected" when unconnected.
- **Verified:** `cargo run -p wf-db --example gh_pr_enrich` enriched live PR `ScriptAddicts/gpt-for-excel-word#2001` (behind/blocked, 12 checks, latestRun). 5 unit tests for normalize/select/approval/badges.

### 3d.1 — Activity reads  ✅ DONE
- **Source:** `github/branches.ts`, `branches-graphql.ts`, `workflows/{workflows,inputs,environments}.ts`, `activity/*`.
- **Built:** `crates/github/src/activity/` — `branches`(+`branches_graphql` query/decode/select), `workflows` (active workflows + dispatch runs), `inputs` (Contents API + `serde_yaml` parse, `on`-key 1.1/1.2 guard), `environments`, `types`. Branch/workflow sweeps swallow per-repo errors into an `error` field (lenient partial-GraphQL read for branches); ref-scoped reads propagate failures as `GithubError::Api`. API `github/activity.rs` (`require_pat` → "No GitHub token connected"); 6 GET routes; `workflowId` parsed server-side. 7 unit tests (selectBranches + parseWorkflowInputs).
- **Endpoints:** `GET /me/github/{branches,workflows,workflow/inputs,workflow/runs,repo/branches,repo/environments}`.
- **Verified:** `cargo run -p wf-db --example gh_activity` — 7 repos' workflows, inputs parsed (`type: environment/boolean` + defaults), 100 branches, 6 environments, runs (200/parsed).

### 3d.2 — Activity writes  ⏳ NEXT
- **Source:** `github/pulls.ts`, `activity/runners.ts`, `write-runners`.
- **Endpoints:** `POST /me/github/workflow/dispatch`, `POST /me/github/pulls` (create), `POST /me/github/pull/merge`, `POST /me/github/pull/close`.
- **Note:** failures → `GithubError::Write { status, message }` (passthrough 403/404/422), slug `github-write-failed`.

### 3d.3 — Favorites
- **Source:** `github/pat/favorites.ts`, `routes/favorites.ts`.
- **Build:** `favorite_workflows` jsonb (`HashMap<String, Vec<i64>>`) repo ops.
- **Endpoints:** `GET /me/github/favorites`, `PUT /me/github/favorites` ({repoFullName, workflowIds}).

---

## Phase 4 — Jira (`wf-jira` + `wf-api/jira`)

### 4a — Jira client + validation
- **Source:** `jira/client.ts`, `errors.ts`, `site-url.ts`, `validate.ts`.
- **Build:** `reqwest` Basic-auth client bound to one `site_url`; **no-follow redirect policy → any 3xx = 502** (credential-replay defense); `JiraApiError { status, message, error_messages }`; site-URL normalization (port `site-url.test.ts`).

### 4b — Jira connection
- **Source:** `jira/pat/*`, `routes/pat.ts`. **DB:** `jira_pat_connections` entity (§6.3) + repo + summary.
- **Endpoints:** `GET /me/jira`, `POST /me/jira/token` ({siteUrl,email,token}), `POST /me/jira/token/validate`, `DELETE /me/jira`, `PUT /me/jira/projects`.
- **Note:** Jira reuses the GitHub `TokenCipher` key.

### 4c — Jira data + mappers
- **Source:** `jira/issues/{adf,jql,fields,mappers,status,dashboard,issues}.ts`, `routes/data.ts`.
- **Port the existing `bun:test` suites as Rust unit tests:** `adf.test.ts`, `jql.test.ts`, `fields.test.ts`, `status.test.ts`.
- **Endpoints:** `GET /me/jira/{dashboard,queue,issue,projects,issuetypes,boards,sprint/issues,issue/transitions,users,createmeta,editmeta}`, `POST /me/jira/search`.

### 4d — Jira actions
- **Source:** `jira/routes/actions.ts`, `action-runners.ts`.
- **Endpoints:** `POST /me/jira/issue/{transition,comment,assign,worklog}`, `POST /me/jira/issue` (create), `PUT /me/jira/issue` (edit).

---

## Phase 5 — OpenAPI + web client (must ship with cutover)
- **5a server:** `#[derive(utoipa::ToSchema)]` on DTOs, `#[utoipa::path]` on handlers, aggregate `OpenApi`, serve `GET /api/openapi.json`, bearer security scheme. Optional Swagger UI behind a dev flag.
- **5b web:** generate typed client from the spec into `apps/web/src/lib/api.gen.ts`; rewrite `apps/web/src/lib/api.ts` (keep `buildAuthHeaders`, `ApiError` shape); remove `@elysiajs/eden`, `treaty<AppT>`, and all `server`/`server/*` type imports; CI spec-drift check; generated client must typecheck.

## Phase 6 — Parity verification
- Scripted diff of TS vs Rust JSON responses for each §14 endpoint (same JWT, same DB). Load-check the dashboard path. Resolve the deferred parity TODOs (below).

## Phase 7 — Cutover
- Decide deploy target (§19.3). Point traffic at the Rust binary; keep TS warm for rollback (schema + token format unchanged → repoint to roll back). Soak, then decommission TS.

---

## Deferred parity TODOs (close before Phase 6)
- 404 framework `detail` is `"Not Found"` — confirm vs Elysia's `"NOT_FOUND"`.
- stdout/stderr log split (info→stdout, warn/error→stderr) not done — single fmt subscriber (spec §12).
- OTEL export wiring when `OTEL_EXPORTER_OTLP_ENDPOINT` is set (spec §12).
- Jira token cache: none today — keep decrypt-per-request for parity (spec §19.5).

## Open decisions (from spec §19)
- OpenAPI client generator: `openapi-typescript` vs `orval`/`openapi-fetch` (decide before 5b).
- Deploy target / log routing (§19.3); connection-pool sizing vs pooler limits (§19.4).
