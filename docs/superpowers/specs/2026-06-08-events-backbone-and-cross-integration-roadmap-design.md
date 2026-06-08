# Roadmap: Event Backbone + Cross-Integration Workflow Features

- **Date:** 2026-06-08
- **Status:** Approved roadmap (high-level). Each sub-project gets its own detailed spec → plan → build cycle.
- **Scope:** Directions #2 (cross-integration workflow features) and #3 (poll → push), decomposed into three sub-projects A, B, C.

## 1. Context

`wf` is the Rust rewrite of the workflow backend (crates: `wf-core`, `wf-db`,
`wf-github`, `wf-jira`, `wf-api`). It currently reaches parity with the old TS
server: a Supabase-auth'd service that proxies GitHub + Jira through stored PATs,
with read-mostly dashboard / activity / actions endpoints and in-memory SWR
caches. Three DB tables exist: `users`, `github_pat_connections`,
`jira_pat_connections`.

Today GitHub and Jira are **two separate proxies**. This roadmap turns `wf` into
an actual *workflow* product by (a) persisting a unified stream of activity and
(b) connecting the two integrations with linking, a merged feed, and a
user-configurable automations engine.

## 2. Decisions locked during brainstorming

| Decision | Choice | Rationale |
|---|---|---|
| Sequencing | Full A+B+C roadmap first, then drill into A | User wants the whole picture before committing |
| Ingestion model | **Hybrid** — webhooks where registerable + scheduled polling fallback + reconciliation | Robust across PAT-only and app-backed connections |
| Rules engine ambition | **User-configurable, DB-stored** rules (trigger · condition · action) | Automation is a headline feature, not a few built-ins |
| Background-work home | **Wake-up / tick model now**, dedicated `wf-worker` (Approach 2) **deferred** | Avoids always-on cost under Cloud Run scale-to-zero; DB-as-state makes the later swap a no-op |

## 3. Keystone architecture — the wake-up / tick model

**The DB is the source of truth for "what is due." A wake-up processes whatever
is due, then exits.** No long-lived background loops (they don't survive Cloud
Run scale-to-zero).

### Triggers
- **Cloud Scheduler** → OIDC-authenticated `POST /internal/tick` on a cron
  (target ~1–2 min cadence).
- **Opportunistic** — a cheap "is anything overdue?" check piggy-backed on
  normal user requests, to cut latency while the instance is already warm.

### Tick algorithm (bounded, idempotent, resumable)
1. **Select due connections** — `WHERE next_poll_at <= now()`.
2. **Poll** each due connection; **diff** new activity into `events` using a
   per-connection `sync_cursor` / ETag so only deltas are fetched.
3. **Enqueue** rule-evaluation work for new events into `work_queue`.
4. **Drain** a bounded batch of `work_queue` (execute due rules), tracking
   `attempts` + `next_attempt_at` for retry/backoff.
5. **Advance** `next_poll_at` and `sync_cursor`.

Every step is idempotent, so a tick that times out or crashes is safe to re-run;
leftover work simply waits for the next tick (all state is in DB).

### Why this shape
- **Scale-to-zero safe** — nothing depends on a process staying alive.
- **Bounded** — each tick respects the Cloud Run request timeout; overflow is
  deferred, not lost.
- **Trivial upgrade to Approach 2** — a future `wf-worker` calls the *same*
  "process what's due" routine inside `loop { …; sleep }` instead of
  once-per-request. No logic rewrite.

### Honest caveat (recorded)
True realtime **push (SSE)** is weak under scale-to-zero: connections drop when
the instance idles out. In this interim, realtime = **SSE while warm + reliable
cursor-based catch-up from `events` on reconnect**. Instant, always-on push
arrives with Approach 2 (a min-instance worker). The DB-cursor design guarantees
the client can always catch up correctly regardless of instance warmth.

## 4. Sub-project A — Event backbone

**Purpose:** land GitHub + Jira activity into one unified `events` store via
hybrid ingestion, wake-up driven. This is the foundation for both B and C.

### Components
- **Webhook receivers**
  - `POST /webhooks/github` — verify HMAC-SHA256 `X-Hub-Signature-256` against
    the connection's stored secret.
  - `POST /webhooks/jira` — Jira PAT-registered webhooks have no native signing,
    so authenticate via a per-connection secret token embedded in the webhook
    URL, plus a source-IP allowlist.
  - Both normalize the payload → write `events` → enqueue `work_queue`.
    Idempotent on the provider delivery/event id.
- **Poller** — per-connection delta fetch reusing the existing `wf-github` /
  `wf-jira` clients; cursor/ETag persisted per connection.
- **Reconciliation** — a periodic wider sweep that compares remote-latest vs
  known-latest to catch events missed by webhooks.
- **Best-effort webhook registration** — attempt to register GitHub repo
  webhooks when the PAT carries `admin:repo_hook`; record per-connection
  registration state; fall back to polling when registration isn't possible
  (notably Jira, where dynamic REST webhooks are admin-gated and expire ~30 days
  unless app-backed).
- **`POST /internal/tick`** — OIDC-secured entry point for the tick algorithm.
- **`GET /me/events?since=<cursor>`** — cursor-based catch-up read over `events`
  (lands here, not in B, because it backs both client catch-up and C's feed).

### New tables
- `events` — `id`, `user_id`, `connection_id`, `source` (github|jira), `type`,
  `external_id` (**unique**, dedup key across webhook + poll paths),
  `payload` (jsonb), `occurred_at`, `ingested_at`.
- `work_queue` — `id`, `kind`, `event_id`, `status`, `attempts`,
  `next_attempt_at`, `locked_at`.

### Connection-table additions
`next_poll_at`, `sync_cursor`, `webhook_secret`, `webhook_state` added to
`github_pat_connections` and `jira_pat_connections`.

### Key risks
- Webhook registration feasibility (PAT scopes; Jira expiry) → mitigated by the
  polling fallback always running.
- Dedup of the same event arriving via both webhook and poll → handled by the
  `events.external_id` unique constraint.
- Per-source signature/auth differences → encapsulated in each receiver.

### Success criteria
Activity from a connected GitHub repo and a connected Jira project reliably
appears in `events` within one poll interval (or near-instantly via webhook),
with no duplicates, and survives a missed webhook via reconciliation.

## 5. Sub-project B — Realtime + sync delivery

**Purpose:** get events to the web client promptly and reliably.

### Components
- **`GET /me/events?since=<cursor>`** — (defined in A) the reliable backbone;
  paginated catch-up that works regardless of instance warmth.
- **`GET /me/events/stream`** (SSE) — pushes new events while the instance is
  warm; the client falls back to cursor polling on disconnect and resumes from
  its last cursor.
- **Sync freshness** — last-synced timestamp per connection surfaced so the UI
  can show "last updated."
- **Opportunistic tick** — these requests carry the cheap freshness check from
  the keystone.

### Key risks / notes
- SSE under scale-to-zero is warm-only by design (see keystone caveat).
- Cross-instance fan-out (event ingested on instance X must reach an SSE client
  on instance Y) is an Approach-2 / pub-sub concern; under scale-to-zero the
  service is effectively single-instance, so warm-SSE + catch-up is correct for
  now.

### Success criteria
A warm client receives new events over SSE within seconds; a client that
disconnects and reconnects loses nothing — it resumes exactly from its last
cursor.

## 6. Sub-project C — Cross-integration features

### C1. PR ↔ Issue linking *(independent of A — read-time; can start first)*
Parse Jira issue keys (e.g. `PROJ-123`) from PR title / branch / body, resolve
them against Jira, and surface the linked issue inline on the dashboard (and the
reverse: issue → its PRs). Optionally persist to `issue_pr_links` so the feed
and rules engine can consume the relationships.

### C2. Unified activity feed *(needs A)*
A normalized DTO query over `events` that merges GitHub + Jira into one timeline,
with filtering and pagination.

### C3. Rules engine *(needs A)*
- `automations` table: `trigger`, `conditions`, `action`, `enabled`, `owner`.
- The engine fires on each new event (via `work_queue`) and executes actions
  through the existing `wf-github` / `wf-jira` action code.
- `automation_runs` log for idempotency + observability.
- CRUD endpoints to manage rules.
- **Guards:** action idempotency (never double-transition); loop prevention
  (rule-generated events are tagged so they don't re-trigger rules); each rule
  runs with the owning connection's PAT (permission scoping).
- Start with a small but genuinely useful trigger / condition / action
  vocabulary (e.g. trigger: `pr.merged`; condition: linked issue exists;
  action: transition issue to Done).

### New tables
- `automations` — rule definitions.
- `automation_runs` — execution log (idempotency + audit).
- `issue_pr_links` *(optional in C1, required if persisted)* — PR↔issue edges.

### Key risks
- Rule action idempotency and infinite-loop prevention (designed-in via run log +
  event tagging).
- Safe, comprehensible rule vocabulary — keep it small first.

### Success criteria
A user can create a rule in the UI, and merging a PR that references a Jira issue
transitions that issue exactly once, with the run recorded in `automation_runs`.

## 7. Build order

**A → C → B**, with two pragmatic tweaks:
- **C1 (linking)** can start *before / alongside* A — it needs no new infra.
- The **`GET /me/events?since=` cursor endpoint lands in A**, not B; only SSE is
  deferred to B.

Rationale: A is the riskiest/most architectural piece and unblocks the rest; C
delivers the headline product value; B is realtime polish that is partially
gated on Approach 2 anyway.

## 8. Net-new database objects (all follow `tables/<t>/{entity,crud,mod}.rs`)

| Table | Sub-project | Purpose |
|---|---|---|
| `events` | A | Unified persisted GitHub + Jira activity |
| `work_queue` | A | Durable in-DB buffer decoupling ingest from execution |
| `automations` | C3 | User-configurable rule definitions |
| `automation_runs` | C3 | Rule execution log (idempotency + audit) |
| `issue_pr_links` | C1 (opt) | Persisted PR↔issue relationships |

Connection tables (`github_pat_connections`, `jira_pat_connections`) gain
`next_poll_at`, `sync_cursor`, `webhook_secret`, `webhook_state`.

## 9. Deferred / out of scope (for now)

- **Approach 2** — a dedicated `wf-worker` service (always-on / Cloud Run Job)
  that runs the tick routine in a loop and enables true cross-instance realtime
  push. Deliberately deferred; the DB-as-state design makes the later swap a
  no-op.
- **Queue infrastructure** (Cloud Tasks / Pub-Sub) — the `work_queue` table is
  the interim durable buffer; migrating to managed queues is an Approach-3
  upgrade path.
- **OAuth Apps** (GitHub App / Jira 3LO) — would make webhook registration
  first-class and remove the Jira 30-day expiry constraint; not required for the
  hybrid model, which already degrades to polling.

## 10. Next step

Drill into **Sub-project A** with its own detailed spec, then writing-plans for
A's implementation.
