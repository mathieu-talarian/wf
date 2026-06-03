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
- ✅ Phase 3 GitHub COMPLETE — 3a/3b/3c/3c.3/3d.1/3d.2/3d.3
- ✅ Phase 4 Jira: 4a client+validate · 4b connection · 4c data+mappers (4d actions next)
- ✅ Dependency upgrade (reqwest 0.13, jsonwebtoken 10, sqlx 0.9, sea-orm 2.0-rc, getrandom 0.4)
- **Endpoints done: 44 / 48** (GitHub 27 + Jira conn 5 + Jira data 12) — `/health`, `/hello/:name`, `/me`, `/me/github` (4 conn), `/me/github/{dashboard,queue,repos}` GET + `repos` PUT, `/me/github/pull` GET + `/me/github/pulls/enrich` POST, `/me/github/{branches,workflows,workflow/inputs,workflow/runs,repo/branches,repo/environments}` GET, `/me/github/{workflow/dispatch,pulls,pull/merge,pull/close}` POST, `/me/github/favorites` GET+PUT. (All GitHub endpoints done; Jira next.)

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

### 3d.2 — Activity writes  ✅ DONE
- **Source:** `github/pulls.ts`, `activity/{activity,runners}.ts` (write half).
- **Built:** `activity/write.rs` (`write_send`/`write_detail`: 403 → permission hint, else upstream `.message`, transport → 502); `activity/pulls.rs` (create/merge/close); `dispatch_workflow` in `workflows.rs`. Write DTOs in `types.rs`. API `activity.rs` write fns + 4 POST routes; dispatch/close → `{ ok: true }`.
- **Endpoints:** `POST /me/github/{workflow/dispatch,pulls,pull/merge,pull/close}`.
- **Verified:** `cargo run -p wf-db --example gh_write_probe` (non-destructive) — bogus dispatch + merge both return `Write { status: 404, message: "Not Found" }` (status passthrough). Happy-path writes intentionally not live-exercised (side effects); covered by faithful port + `error.rs` mapping.

### 3d.3 — Favorites  ✅ DONE
- **Source:** `github/pat/favorites.ts`, `pat/account.ts` (runGet/runSet), `routes/favorites.ts`.
- **Built:** `wf-db github_pat.rs` — `FavoritesMap` (`HashMap<String, Vec<i64>>`), `favorites_of`/`set_repo_in_favorites` (dedupe first-wins, drop-empty), `get_favorites`/`set_repo_favorites` (no row → `DbErr::Custom` → 500, matching TS GithubDbError). 5 unit tests (ported `favorites.test.ts`). Routes GET/PUT `/me/github/favorites` in `routes.rs`.
- **Endpoints:** `GET /me/github/favorites`, `PUT /me/github/favorites` ({repoFullName, workflowIds}).
- **Verified:** `cargo run -p wf-db --example gh_favorites` — set [1,2,2,3]→[1,2,3], merged without touching other repos, persisted, restored original (DB unchanged).

---

## Phase 4 — Jira (`wf-jira` + `wf-api/jira`)

### 4a — Jira client + validation  ✅ DONE
- **Source:** `jira/client.ts`, `errors.ts`, `site-url.ts`, `validate.ts`, `issues/status.ts`.
- **Built:** `wf-jira` crate — `errors.rs` (JiraApiError/JiraWriteError/JiraNotConnected), `site_url.rs` (`normalize_site_url`/`is_same_jira_origin` via `url` crate; 14 ported tests), `status.rs` (JiraValidationStatus + `validation_status_for_http`), `client.rs` (reqwest Basic-auth, `redirect::Policy::none()` → 3xx = 502 redirect error; non-2xx → JiraApiError w/ errorMessages[0]), `validate.rs` (`validate_credentials` → `/rest/api/3/myself`). 15 unit tests.
- **Verified:** `cargo run -p wf-jira --example jira_validate` (offline normalization demo; env-gated live validate). Real-credential network check deferred to 4b (stored connection).

### 4b — Jira connection  ✅ DONE
- **Source:** `jira/pat/{account,runners}.ts`, `routes/{pat,helpers}.ts`.
- **Built:** `wf-db` `jira_pat_connections` entity (§6.3) + `repositories/jira_pat.rs` (select/upsert/mark_validation/set_selected_projects/touch_last_used/disconnect; re-connect preserves selectedProjects/createdAt/lastUsedAt). `wf-api/jira/{summary,pat,routes,mod}.rs` — connect/validate/disconnect/set_projects (decrypt-per-request, no cache per §19.5). `AppError` extended: `jira-token-rejected` (httpStatus ?? invalid→401/missing_permissions→403/else 502, reason), `jira-not-connected` (404), `jira-write-failed` (passthrough), `jira-request-failed` (502). Routes registered in `routes/mod.rs`.
- **Endpoints:** `GET /me/jira`, `POST /me/jira/token`, `POST /me/jira/token/validate`, `DELETE /me/jira`, `PUT /me/jira/projects`.
- **Note:** Jira reuses the GitHub `TokenCipher` key.
- **Verified:** `cargo run -p wf-db --example jira_row` — entity select against live Supabase succeeds (schema match). Full connect flow needs real creds (deferred; same as 4a).

### 4c — Jira data + mappers  ✅ DONE
- **Source:** `jira/issues/{adf,jql,fields,mappers,status,dashboard,issues}.ts`, `action-runners.ts` (reads), `routes/data.ts`.
- **Built:** `wf-jira` — `types.rs` (DTOs + field constants), `issues/{adf,jql,fields,mappers,search,dashboard,reads}.rs`. ADF flatten, JQL builders (injection-safe quoting), metadata-driven field coercion (write-path security boundary), payload mappers, search (`/search/jql` + approximate-count), 5-queue concurrent dashboard with per-queue degradation, lookups (projects/issuetypes/boards/sprint/transitions/users/createmeta/editmeta). `status.rs` gained `classify_queue_failure`. `wf-api/jira/data.rs` (loadConnected → client, decrypt-per-request) + 12 routes. **All 4 bun:test suites ported** (adf/jql/fields/status) → 52 wf-jira unit tests.
- **Endpoints:** `GET /me/jira/{dashboard,queue,issue,projects,issuetypes,boards,sprint/issues,issue/transitions,users,createmeta,editmeta}`, `POST /me/jira/search`.
- **Verified:** 52 unit tests (ported suites). Live data path needs real Jira creds (deferred — same as 4a/4b).
- **Minor parity note:** editmeta field order is by fieldId (BTreeMap) vs Jira's insertion order; createmeta uses an array (order preserved).

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
