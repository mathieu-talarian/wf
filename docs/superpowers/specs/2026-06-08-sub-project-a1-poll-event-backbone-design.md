# Sub-project A1 — Poll-driven Event Backbone (design)

- **Date:** 2026-06-08
- **Status:** Approved design. Next step: writing-plans → implementation.
- **Parent roadmap:** `2026-06-08-events-backbone-and-cross-integration-roadmap-design.md`
- **Scope:** The poll-driven core of sub-project A. Webhook ingestion (receivers,
  signature/secret schemes, best-effort registration) is explicitly **A2**, a
  separate follow-up spec.

## 1. Purpose

Persist a unified, queryable stream of GitHub + Jira activity into an `events`
table, populated by a **wake-up / tick** poller that runs whatever is due and
exits (Cloud Run scale-to-zero safe). This is the foundation that unblocks
sub-project B (realtime/cursor delivery) and sub-project C (unified feed + rules
engine). Everything here works with **every** connection regardless of PAT
scopes, because it relies only on the same provider read permissions the
dashboard already uses. A1 may add new read helpers, but not new privileged
provider scopes.

## 2. Decisions locked during brainstorming

| # | Decision | Choice |
|---|---|---|
| 1 | Schema mechanism | Adopt `sea-orm-migration` (versioned migrations) |
| 2 | First-cut scope | **Poll-first core (A1)**; webhooks deferred to A2 |
| 3 | `sync_state` table (vs columns on credential tables) | **Yes** — separate table, per-scope cursors, no churn on credential tables |
| 4 | `work_queue` table | **Deferred to C** — its only consumer is the rules engine (YAGNI here) |
| 5 | `events.id` | `bigint` identity — PK and monotonic feed cursor |
| 6 | `/internal/tick` auth | Shared-secret header for v1; OIDC is the noted hardening path |
| 7 | v1 event vocabulary | The focused set in §5 |

### Deviations from the roadmap (deliberate, approved)
- **`sync_state` table** replaces the roadmap's "add cursor columns to the
  connection tables." A GitHub connection polls many repos × several entity
  kinds; a single column can't hold per-scope cursors, and we avoid touching the
  sensitive credential tables.
- **`work_queue` moves to sub-project C.** Nothing in A1 consumes it.

## 3. Data model

Both tables follow the project convention: one directory per table under
`crates/db/src/tables/<table>/` with `entity.rs` + `crud.rs` + `mod.rs`.

### 3.1 `events`

| column | type | notes |
|---|---|---|
| `id` | `bigint` GENERATED ALWAYS AS IDENTITY | PK **and** monotonic cursor for `?since=` |
| `user_id` | uuid | FK → `users.id` ON DELETE CASCADE |
| `source` | text | `github` \| `jira` |
| `type` | text | e.g. `github.workflow_run.completed`, `github.pull_request.merged`, `jira.issue.transitioned` |
| `external_id` | text | stable per-user dedup key encoding provider instance + entity + state (§6) |
| `scope_key` | text | repo full-name (`owner/repo`) or Jira project key |
| `actor` | text | nullable — login / display name |
| `title` | text | nullable — human summary |
| `url` | text | nullable — html link to the entity |
| `occurred_at` | timestamptz | provider event time |
| `payload` | jsonb | normalized fields + raw subset |
| `ingested_at` | timestamptz | default `now()` |

**Indexes**
- `UNIQUE (user_id, source, external_id)` — dedup safety net (poll re-runs, and
  later webhook-vs-poll convergence in A2) without dropping another user's copy
  of the same repo/project event.
- `(user_id, id)` — cursor paging for the feed / `?since=`.
- `(user_id, scope_key, id)` — filtered feed by repo/project.

### 3.2 `sync_state`

One row per pollable scope (a scope = one entity kind within one repo/project).

| column | type | notes |
|---|---|---|
| `user_id` | uuid | part of PK; FK → `users.id` ON DELETE CASCADE |
| `source` | text | part of PK — `github` \| `jira` |
| `scope_key` | text | part of PK — repo full-name or project key |
| `entity_kind` | text | part of PK — e.g. `workflow_run`, `pull_request`, `issue` |
| `cursor` | text | nullable — high-watermark (`updated_at` / `updated`) |
| `last_polled_at` | timestamptz | nullable |
| `next_poll_at` | timestamptz | not null, default `now()` — drives tick selection |
| `consecutive_errors` | int | default 0 — backoff input |
| `last_error` | text | nullable |
| `lease_owner` | text | nullable — tick instance that claimed the row |
| `lease_until` | timestamptz | nullable — expires abandoned claims |

PK = `(user_id, source, scope_key, entity_kind)`.

**Lifecycle:** `sync_state` rows are reconciled from the connection's
`selected_repos` / `selected_projects` at the start of a tick — insert a row per
newly-selected `(scope × entity_kind)` with `next_poll_at = now()`, and **delete
rows for de-selected scopes** (so they stop being polled). New connections
therefore begin polling on the next tick with no extra wiring.

## 4. The tick

### 4.1 Endpoint
`POST /internal/tick` — authenticated by a shared-secret header
(`X-Internal-Token`) compared against `INTERNAL_TICK_TOKEN` (env). Cloud Scheduler
is configured to send the header on a ~1–2 min cron. *(OIDC verification is the
documented hardening path; not in A1.)*

### 4.2 Algorithm (bounded, idempotent, resumable)
1. **Reconcile scopes** — ensure `sync_state` holds exactly one row per
   `(selected scope × entity_kind)` per connection: insert missing rows with
   `next_poll_at = now()`, delete rows for de-selected scopes.
2. **Claim due** — atomically lease rows with expired/no lease:
   `WITH due AS (SELECT user_id, source, scope_key, entity_kind FROM sync_state
   WHERE next_poll_at <= now() AND (lease_until IS NULL OR lease_until < now())
   ORDER BY next_poll_at LIMIT <batch> FOR UPDATE SKIP LOCKED) UPDATE
   sync_state ... SET lease_owner, lease_until = now() + <lease_ttl> ...
   RETURNING *`.
3. **Poll each** — load connection, decrypt PAT via the existing
   `wf_core` `TokenCipher`, call the matching `wf-github` / `wf-jira` read,
   **diff against `cursor`**, normalize new items (§5), insert
   `ON CONFLICT (user_id, source, external_id) DO NOTHING`.
4. **Advance** — set `cursor`, `last_polled_at = now()`,
   `next_poll_at = now() + interval` (exponential backoff using
   `consecutive_errors` on failure), clear the lease, update `last_error` /
   reset on success. Updates include the lease owner in the `WHERE` clause so a
   stale tick cannot overwrite a row re-claimed after lease expiry.
5. **Stay bounded** — stop at the batch count **or** a wall-clock budget,
   whichever first; remaining due scopes wait for the next tick.

Every step is idempotent: a tick that times out or crashes mid-way is safe to
re-run — committed cursor advances and `ON CONFLICT DO NOTHING` make repeats
harmless.

### 4.3 Opportunistic trigger (minimal)
Authenticated reads (e.g. dashboard/feed) may **mark** an overdue scope so the
next tick prioritizes it. No inline polling on the request path in A1 — keeps
request latency clean.

### 4.4 Reconciliation
In poll-only mode **every poll is a reconciliation** (always diff latest-remote
vs cursor). A dedicated reconciliation sweep is only needed once webhooks can
miss deliveries — that is an A2 concern.

### 4.5 Upgrade path (Approach 2)
A future `wf-worker` calls the *same* "process due scopes" routine inside
`loop { …; sleep }` instead of once per request. No logic rewrite; the tick body
is factored so both entry points share it.

## 5. Event vocabulary (v1)

Scoped to each connection's existing `selected_repos` / `selected_projects`.

| source | entity_kind | emitted `type`s | read API |
|---|---|---|---|
| github | `workflow_run` | `github.workflow_run.completed` (success/failure) | **new** repo-level delta helper over `/repos/{owner}/{repo}/actions/runs` |
| github | `pull_request` | `github.pull_request.opened` / `.merged` / `.closed` | **new** PR delta helper; do not use dashboard queues |
| jira | `issue` | `jira.issue.created`, `jira.issue.transitioned` (status change) | **new** issue delta helper using selected-project JQL |

The existing GitHub dashboard APIs are intentionally not the poller contract:
they are user-queue shaped and open-PR-only. A1 adds poller-specific reads that
return stable provider ids, state, created/closed/merged/status timestamps, and
updated watermarks.

**Deferred (post-A1, trivial to add):** PR review/check-state churn, Jira
comments, pushes/branches.

### 5.1 Normalization
A normalizer per `(source, entity_kind)` maps a provider item → an `events` row:
`external_id`, `type`, `scope_key`, `actor`, `title`, `url`, `occurred_at`, and a
`payload` carrying the normalized fields plus a raw subset for fidelity.
Normalizers are pure functions (provider DTO → `Event`), making them unit-testable
from fixtures.

Jira issue deltas must request enough fields for the vocabulary:
`summary`, `status.id`, `status.name`, `status.statusCategory`, `created`,
`updated`, `project`, and a stable issue id/key. For A1, `transitioned` means
"status changed relative to the last ingested status for this issue"; if multiple
Jira status changes happen between polls and the delta response does not include
changelog history, A1 records the latest observed transition and does not promise
every intermediate status.

## 6. Dedup & cursors

### 6.1 `external_id` (dedup key)
Encodes **provider instance + entity + state**, so re-polling an unchanged entity
collides for the same user but never suppresses another user's event. Proposed
forms:
- workflow run: `repo:{owner}/{repo}:wfrun:{run_id}:{run_attempt}:{conclusion}`
- pull request: `repo:{owner}/{repo}:pr:{number}:{state}` (`state` ∈ open/merged/closed)
- jira issue created: `site:{site_or_cloud_id}:issue:{issueKey}:created`
- jira status change: `site:{site_or_cloud_id}:issue:{issueKey}:status:{statusId}:updated:{updated}`

The `UNIQUE (user_id, source, external_id)` index is the safety net; the cursor
diff is the primary mechanism. This same key makes A2's webhook events converge
with polled events for free after the webhook receiver resolves the owning user /
connection.

### 6.2 Cursors
- Cursor values are JSON encoded in the `sync_state.cursor` text column.
- GitHub: compound high-watermark per repo+kind, `{ "updated_at": "...",
  "id": "..." }`, using provider id / node id as the tie-breaker.
- Jira: compound high-watermark per project, `{ "updated": "...", "issue_key":
  "..." }`, ordered by `updated ASC, key ASC`.
- Provider APIs that cannot express the full compound predicate use an overlap
  query on the provider update timestamp (for example, `updated >=
  cursor.timestamp`) and client-side filtering of items at or below the stored
  compound cursor. Dedup still protects replays.

## 7. Internal auth, migrations, config

### 7.1 Migrations
- New `migration` crate using `sea-orm-migration`: `m0001_create_events`,
  `m0002_create_sync_state`.
- **Must run against the session pooler (`:5432`) with the statement cache
  disabled** — the documented `42P05` constraint. A validation step confirms the
  migrator connects and applies cleanly on the pooler.
- Migrations are the source of truth for DDL; hand-written entities match them.

### 7.2 Config (env, parsed in `wf-core`)
| var | default | purpose |
|---|---|---|
| `POLL_INTERVAL_SECS` | 120 | base interval written into `next_poll_at` |
| `TICK_BATCH_SIZE` | 50 | max scopes per tick |
| `TICK_BUDGET_MS` | 30000 | wall-clock budget per tick |
| `TICK_LEASE_SECS` | 90 | claim TTL for due `sync_state` rows |
| `INTERNAL_TICK_TOKEN` | — (required) | shared secret for `/internal/tick` |

## 8. Error handling

- **Scope isolation** — a failing scope never blocks others; its `last_error` /
  `consecutive_errors` are recorded and `next_poll_at` is backed off.
- **Revoked / invalid PAT** — detected via the existing validation path; reuses
  the connection's `validation_status` field and pauses that connection's scopes
  (don't hammer a dead token).
- **Partial tick** — safe by construction (idempotent steps, committed cursors);
  abandoned leases expire and are claimable by a later tick.
- Errors surface as structured logs/metrics via the existing telemetry
  middleware; `/internal/tick` returns a small JSON summary (scopes processed,
  events written, errors) for Scheduler logs.

## 9. Testing

- **Unit:** normalizers (fixture payload → `Event`); `external_id` stability
  across re-polls; cursor advance; backoff math; scope reconciliation from
  `selected_repos` / `selected_projects`; compound cursor tie handling; lease
  claim/update guards.
- **Integration:** drive the tick against **wiremock** GitHub/Jira endpoints
  serving fixtures → assert `events` rows; re-run the same tick → assert **no
  duplicates** (dedup) and cursor advanced; run two concurrent ticks over the
  same due rows → assert each scope is processed once; seed two users watching
  the same repo/project → assert both receive events.
- **Live smoke:** `cargo run -p wf-db --example tick_smoke` (existing
  example-harness style; needs `.env` + a real connected user) — one tick,
  prints events written.
- **CI:** must pass the lint gate
  `cargo clippy --all --all-targets --locked -- -D warnings` and
  `cargo test --workspace`.

## 10. Out of scope (this spec)

- **A2:** webhook receivers (`/webhooks/github` HMAC, `/webhooks/jira`
  secret-token), best-effort webhook registration, `webhook_*` state, dedicated
  reconciliation sweep.
- **B:** the public `GET /me/events?since=` read endpoint and SSE. *(A1 produces
  the data and the `id` cursor; exposing it to clients is B. The internal cursor
  semantics are defined here so B is a thin read layer.)*
- **C:** `work_queue`, unified feed DTO, rules engine, PR↔issue linking.
- **Approach 2:** the always-on `wf-worker`.

## 11. New / changed artifacts (orientation for the plan)

- New crate: `migration` (sea-orm-migration).
- New tables (per-table dirs): `crates/db/src/tables/events/`,
  `crates/db/src/tables/sync_state/`.
- `wf-core`: config additions (§7.2).
- `wf-github`: new poller-specific delta helpers for repo workflow runs and PRs
  (separate from dashboard queue reads).
- `wf-jira`: new poller-specific issue delta helper with created/status-id fields
  and compound cursor ordering.
- `wf-api`: `POST /internal/tick` route + handler; the shared tick routine
  (factored for the future worker); per-source pollers + normalizers (likely
  `crates/api/src/sync/…` reusing `wf-github` / `wf-jira` reads); opportunistic
  "mark overdue" hook on existing reads.
- Tests + `tick_smoke` example.

## 12. Next step

Invoke writing-plans to turn this into a bite-sized implementation plan.
