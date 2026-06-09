# A1 Poll-Driven Event Backbone Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist a unified stream of GitHub + Jira activity into an `events` table via a bounded, idempotent `/internal/tick` poller (spec: `docs/superpowers/specs/2026-06-08-sub-project-a1-poll-event-backbone-design.md`).

**Architecture:** New `migration` crate (sea-orm-migration, raw-SQL DDL) creates `events` + `sync_state`. New **`wf-sync` lib crate** holds cursors, normalizers, and the tick routine (factored so the future `wf-worker` reuses it; this is why the smoke example is `-p wf-sync`, not `-p wf-db` as the spec sketched). `wf-github`/`wf-jira` gain poller-specific page reads ("page-1 DESC by `updated`, client-side compound-cursor filter" — uniform across all three pollers; Jira ASC ordering from spec §6.2 was replaced because absolute JQL timestamps are interpreted in the account's timezone, a correctness trap; the documented limitation is >50 updates per scope per tick lose the older ones, dedup-safe). `wf-api` exposes `POST /internal/tick` (shared-secret header) and a fire-and-forget "mark overdue" hook.

**Tech Stack:** Rust, actix-web 4, sea-orm 2.0.0-rc.40 (+ sea-orm-migration), reqwest, chrono, wiremock (new dev-dep), Supabase Postgres (session pooler :5432).

**Project gotchas (enforced by CI, apply to EVERY task):**
- Lint gate: `cargo clippy --all --all-targets --locked -- -D warnings` — includes `too_many_lines = 25`: **every function ≤ 25 lines**; extract helpers.
- `#[cfg(test)] mod tests` must be the **last item** in a file.
- Public types with `new()` need a `Default` impl.
- DB writes: `ActiveModel` is built **only** inside that table's `crud.rs`; callers pass typed input structs.
- Response/payload JSON: camelCase. Raw SQL: `db.query_one_raw` / `query_all_raw` / `execute_raw`.
- `cursor` is a **reserved word in Postgres** — always write it quoted (`"cursor"`) in DDL and raw SQL. SeaORM entities quote automatically.

---

## File structure

| Path | Responsibility |
|---|---|
| `crates/migration/` (new crate `migration`) | versioned DDL: `m0001_create_events`, `m0002_create_sync_state` |
| `crates/db/src/tables/events/{entity,crud,mod}.rs` | events schema + `insert_ignore_dups`, `latest_jira_status` |
| `crates/db/src/tables/sync_state/{entity,crud,mod}.rs` | sync_state schema + `replace_scopes`, `claim_due`, `complete_ok/err`, `mark_user_due` |
| `crates/db/src/tables/{github,jira}_pat_connections/crud.rs` | + `list_all` |
| `crates/core/src/config.rs` | + 5 tick env vars |
| `crates/github/src/poll.rs` (+ `client.rs` base injection) | `PolledWorkflowRun`/`PolledPullRequest` page reads |
| `crates/jira/src/poll.rs` | `PolledIssue` page read |
| `crates/sync/` (new crate `wf-sync`) | `cursor.rs`, `normalize.rs`, `tick.rs`, `lib.rs`; example `tick_smoke`; env-gated integration test |
| `crates/api/src/routes/internal.rs` + `main.rs` | `POST /internal/tick` |
| `crates/api/src/github/routes.rs` | opportunistic mark hook |
| `CLAUDE.md`, `DEPLOYMENT.md`, spec appendix | docs + Cloud Scheduler |

---

### Task 1: Tick config (wf-core)

**Files:** Modify: `crates/core/src/config.rs`; Modify: `.env` (user-local)

- [ ] **Step 1: Write failing tests** — append inside the existing `#[cfg(test)] mod tests` in `config.rs` (reuse its existing valid-env map helper; if tests build the map inline, copy that style). **The existing tests will now fail until `INTERNAL_TICK_TOKEN` is added to their base map — update them in the same commit.**

```rust
#[test]
fn tick_config_defaults() {
    let mut env = valid_env(); // existing helper / inline map incl. all required vars
    env.insert("INTERNAL_TICK_TOKEN".into(), "secret".into());
    let cfg = Config::from_map(&env).unwrap();
    assert_eq!(cfg.poll_interval_secs, 120);
    assert_eq!(cfg.tick_batch_size, 50);
    assert_eq!(cfg.tick_budget_ms, 30_000);
    assert_eq!(cfg.tick_lease_secs, 90);
    assert_eq!(cfg.internal_tick_token, "secret");
}

#[test]
fn tick_config_overrides_and_required_token() {
    let mut env = valid_env();
    env.insert("INTERNAL_TICK_TOKEN".into(), "s".into());
    env.insert("POLL_INTERVAL_SECS".into(), "30".into());
    assert_eq!(Config::from_map(&env).unwrap().poll_interval_secs, 30);
    env.insert("POLL_INTERVAL_SECS".into(), "abc".into());
    assert!(Config::from_map(&env).is_err());
    let mut no_token = valid_env();
    no_token.remove("INTERNAL_TICK_TOKEN");
    assert!(Config::from_map(&no_token).is_err());
}
```

- [ ] **Step 2: Run** `cargo test -p wf-core` — expect FAIL (unknown fields).
- [ ] **Step 3: Implement** — add to `Config`:

```rust
    pub poll_interval_secs: u64,
    pub tick_batch_size: u64,
    pub tick_budget_ms: u64,
    pub tick_lease_secs: u64,
    pub internal_tick_token: String,
```

in `from_map` (after the existing fields):

```rust
            poll_interval_secs: parse_u64(map, "POLL_INTERVAL_SECS", 120)?,
            tick_batch_size: parse_u64(map, "TICK_BATCH_SIZE", 50)?,
            tick_budget_ms: parse_u64(map, "TICK_BUDGET_MS", 30_000)?,
            tick_lease_secs: parse_u64(map, "TICK_LEASE_SECS", 90)?,
            internal_tick_token: required(map, "INTERNAL_TICK_TOKEN")?,
```

new helper next to `parse_port`:

```rust
/// Parses an optional positive-integer env var with a default.
fn parse_u64(map: &HashMap<String, String>, key: &str, default: u64) -> Result<u64, ConfigError> {
    match present(map, key) {
        None => Ok(default),
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| ConfigError::Invalid(format!("{key} must be a positive integer"))),
    }
}
```

- [ ] **Step 4: Run** `cargo test -p wf-core` — expect PASS (including pre-existing tests you updated).
- [ ] **Step 5: Add to your local `.env`:** `INTERNAL_TICK_TOKEN=$(openssl rand -hex 32)` (server won't boot without it).
- [ ] **Step 6: Commit** `git add crates/core && git commit -m "A1: tick configuration (wf-core)"`

---

### Task 2: `migration` crate + apply

**Files:** Create: `crates/migration/Cargo.toml`, `crates/migration/src/{lib.rs,main.rs,m0001_create_events.rs,m0002_create_sync_state.rs}`; Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Workspace member** — in root `Cargo.toml`: `members = [..., "crates/migration"]`.
- [ ] **Step 2: `crates/migration/Cargo.toml`:**

```toml
[package]
name = "migration"
version.workspace = true
edition.workspace = true
license.workspace = true
publish = false

[lints]
workspace = true

[dependencies]
sea-orm-migration = { version = "2.0.0-rc.40", default-features = false, features = [
    "sqlx-postgres",
    "runtime-tokio-rustls",
    "cli",
] }
tokio = { workspace = true }
dotenvy = { workspace = true }
async-trait = "0.1"
```

- [ ] **Step 3: `src/lib.rs`:**

```rust
//! Versioned DDL for the event backbone (A1 spec §7.1). Raw SQL migrations:
//! the SQL is the source of truth; hand-written entities in `wf-db` match it.

pub use sea_orm_migration::prelude::*;

mod m0001_create_events;
mod m0002_create_sync_state;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m0001_create_events::Migration),
            Box::new(m0002_create_sync_state::Migration),
        ]
    }
}
```

- [ ] **Step 4: `src/m0001_create_events.rs`** (RLS enabled / zero policies matches the existing tables' convention — backend connects as `postgres` and bypasses it):

```rust
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TABLE events (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  source text NOT NULL,
  type text NOT NULL,
  external_id text NOT NULL,
  scope_key text NOT NULL,
  actor text,
  title text,
  url text,
  occurred_at timestamptz NOT NULL,
  payload jsonb NOT NULL,
  ingested_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX events_user_source_external_idx ON events (user_id, source, external_id);
CREATE INDEX events_user_id_idx ON events (user_id, id);
CREATE INDEX events_user_scope_id_idx ON events (user_id, scope_key, id);
ALTER TABLE events ENABLE ROW LEVEL SECURITY;
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP).await.map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS events;")
            .await
            .map(|_| ())
    }
}
```

- [ ] **Step 5: `src/m0002_create_sync_state.rs`** (same shape; note quoted `"cursor"`):

```rust
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TABLE sync_state (
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  source text NOT NULL,
  scope_key text NOT NULL,
  entity_kind text NOT NULL,
  "cursor" text,
  last_polled_at timestamptz,
  next_poll_at timestamptz NOT NULL DEFAULT now(),
  consecutive_errors integer NOT NULL DEFAULT 0,
  last_error text,
  lease_owner text,
  lease_until timestamptz,
  PRIMARY KEY (user_id, source, scope_key, entity_kind)
);
CREATE INDEX sync_state_due_idx ON sync_state (next_poll_at);
ALTER TABLE sync_state ENABLE ROW LEVEL SECURITY;
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP).await.map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS sync_state;")
            .await
            .map(|_| ())
    }
}
```

- [ ] **Step 6: `src/main.rs`:**

```rust
//! Migration CLI. Run against the **session pooler (:5432)** DATABASE_URL
//! (spec §7.1): `cargo run -p migration -- up` / `-- status`.

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    sea_orm_migration::cli::run_cli(migration::Migrator).await;
}
```

- [ ] **Step 7: Build** `cargo build -p migration` — expect success. If the `cli` feature or API surface differs on rc.40, check the sea-orm-migration docs via context7 before improvising.
- [ ] **Step 8: Apply + verify** — `cargo run -p migration -- up` then `cargo run -p migration -- status` → both migrations `Applied`. (This touches the live Supabase DB — it only creates new tables.)
- [ ] **Step 9: Commit** `git add Cargo.toml Cargo.lock crates/migration && git commit -m "A1: migration crate — events + sync_state DDL"`

---

### Task 3: `events` table trio (wf-db)

**Files:** Create: `crates/db/src/tables/events/{entity.rs,crud.rs,mod.rs}`; Modify: `crates/db/src/tables/mod.rs`

- [ ] **Step 1: `entity.rs`:**

```rust
//! SeaORM entity for `events` (A1 spec §3.1) — matches `migration::m0001`.
//! `id` is a Postgres identity column (the feed cursor); `type` is mapped to
//! `event_type` because `type` is a Rust keyword.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "events")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: Uuid,
    pub source: String,
    #[sea_orm(column_name = "type")]
    pub event_type: String,
    pub external_id: String,
    pub scope_key: String,
    pub actor: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub occurred_at: DateTimeWithTimeZone,
    pub payload: Json,
    pub ingested_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
```

- [ ] **Step 2: `crud.rs`:**

```rust
//! `events` repository (A1 spec §3.1/§6): batch insert with per-user dedup via
//! `ON CONFLICT (user_id, source, external_id) DO NOTHING`, plus the latest-
//! status lookup the Jira normalizer needs (spec §5.1).

use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::OnConflict;
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait, Statement};

use super::entity as events;

/// Typed input for one event row (the only `ActiveModel` construction site).
#[derive(Debug, Clone)]
pub struct InsertEventInput {
    pub user_id: Uuid,
    pub source: String,
    pub event_type: String,
    pub external_id: String,
    pub scope_key: String,
    pub actor: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub occurred_at: DateTimeWithTimeZone,
    pub payload: serde_json::Value,
}

/// Inserts a batch, silently skipping rows whose `(user_id, source,
/// external_id)` already exist. Returns the number of rows actually written.
pub async fn insert_ignore_dups(
    db: &DatabaseConnection,
    inputs: Vec<InsertEventInput>,
) -> Result<u64, DbErr> {
    if inputs.is_empty() {
        return Ok(0);
    }
    let models = inputs.into_iter().map(active_model);
    events::Entity::insert_many(models)
        .on_conflict(
            OnConflict::columns([
                events::Column::UserId,
                events::Column::Source,
                events::Column::ExternalId,
            ])
            .do_nothing()
            .to_owned(),
        )
        .exec_without_returning(db)
        .await
}

/// Latest ingested Jira `statusId` for an issue (same user, same project
/// scope) — the Jira normalizer's "previous status" input (spec §5.1).
pub async fn latest_jira_status(
    db: &DatabaseConnection,
    user_id: Uuid,
    scope_key: &str,
    issue_key: &str,
) -> Result<Option<String>, DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT payload->>'statusId' AS status_id FROM events \
         WHERE user_id = $1 AND source = 'jira' AND scope_key = $2 \
           AND payload->>'issueKey' = $3 ORDER BY id DESC LIMIT 1",
        [user_id.into(), scope_key.into(), issue_key.into()],
    );
    let row = db.query_one_raw(stmt).await?;
    match row {
        None => Ok(None),
        Some(r) => r.try_get::<Option<String>>("", "status_id"),
    }
}

/// Builds the `ActiveModel` for [`insert_ignore_dups`]; `id`/`ingested_at`
/// stay `NotSet` (identity + DB default).
fn active_model(input: InsertEventInput) -> events::ActiveModel {
    events::ActiveModel {
        id: NotSet,
        user_id: Set(input.user_id),
        source: Set(input.source),
        event_type: Set(input.event_type),
        external_id: Set(input.external_id),
        scope_key: Set(input.scope_key),
        actor: Set(input.actor),
        title: Set(input.title),
        url: Set(input.url),
        occurred_at: Set(input.occurred_at),
        payload: Set(input.payload),
        ingested_at: NotSet,
    }
}
```

- [ ] **Step 3: `mod.rs`** (mirror `users/mod.rs` doc style):

```rust
//! `events` table. `entity` holds the SeaORM schema; `crud` holds every
//! operation (the only place an `events` `ActiveModel` is built).

mod crud;
mod entity;

pub use crud::*;
pub use entity::*;
```

- [ ] **Step 4:** Add `pub mod events;` to `crates/db/src/tables/mod.rs`.
- [ ] **Step 5: Verify** `cargo build -p wf-db && cargo clippy -p wf-db --all-targets --locked -- -D warnings` — expect clean. (No unit tests here: both fns are thin DB I/O; they're exercised by the Task 14 integration test.)
- [ ] **Step 6: Commit** `git add crates/db && git commit -m "A1: events table (entity + crud)"`

---

### Task 4: `sync_state` table trio (wf-db)

**Files:** Create: `crates/db/src/tables/sync_state/{entity.rs,crud.rs,mod.rs}`; Modify: `crates/db/src/tables/mod.rs`

- [ ] **Step 1: `entity.rs`:**

```rust
//! SeaORM entity for `sync_state` (A1 spec §3.2) — one row per pollable scope,
//! composite PK. Matches `migration::m0002` (note: the `cursor` column is a
//! reserved word in Postgres; SeaORM quotes it, raw SQL must too).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "sync_state")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub source: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub scope_key: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub entity_kind: String,
    pub cursor: Option<String>,
    pub last_polled_at: Option<DateTimeWithTimeZone>,
    pub next_poll_at: DateTimeWithTimeZone,
    pub consecutive_errors: i32,
    pub last_error: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_until: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
```

- [ ] **Step 2: `crud.rs`** — reconcile, lease-claim, owner-guarded completion, opportunistic mark:

```rust
//! `sync_state` repository (A1 spec §3.2/§4.2): scope reconciliation, atomic
//! lease-claim of due rows (`FOR UPDATE SKIP LOCKED`), owner-guarded
//! completion (a stale tick can't overwrite a re-claimed row), and the
//! opportunistic "mark overdue" hook (spec §4.3).

use sea_orm::prelude::Uuid;
use sea_orm::sea_query::OnConflict;
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, Statement,
};

use super::entity as sync_state;

/// One desired pollable scope: `(scope_key, entity_kind)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeKey {
    pub scope_key: String,
    pub entity_kind: String,
}

/// Makes `sync_state` for `(user, source)` hold exactly the `desired` scopes:
/// inserts missing rows (due immediately) and deletes de-selected ones.
pub async fn replace_scopes(
    db: &DatabaseConnection,
    user_id: Uuid,
    source: &str,
    desired: &[ScopeKey],
) -> Result<(), DbErr> {
    let existing = existing_scopes(db, user_id, source).await?;
    insert_missing(db, user_id, source, desired, &existing).await?;
    delete_deselected(db, user_id, source, desired, &existing).await
}

async fn existing_scopes(
    db: &DatabaseConnection,
    user_id: Uuid,
    source: &str,
) -> Result<Vec<ScopeKey>, DbErr> {
    let rows = sync_state::Entity::find()
        .filter(sync_state::Column::UserId.eq(user_id))
        .filter(sync_state::Column::Source.eq(source))
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| ScopeKey { scope_key: r.scope_key, entity_kind: r.entity_kind })
        .collect())
}

async fn insert_missing(
    db: &DatabaseConnection,
    user_id: Uuid,
    source: &str,
    desired: &[ScopeKey],
    existing: &[ScopeKey],
) -> Result<(), DbErr> {
    let missing: Vec<_> = desired
        .iter()
        .filter(|s| !existing.contains(s))
        .map(|s| new_scope_model(user_id, source, s))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    sync_state::Entity::insert_many(missing)
        .on_conflict(conflict_pk().do_nothing().to_owned())
        .exec_without_returning(db)
        .await
        .map(|_| ())
}

async fn delete_deselected(
    db: &DatabaseConnection,
    user_id: Uuid,
    source: &str,
    desired: &[ScopeKey],
    existing: &[ScopeKey],
) -> Result<(), DbErr> {
    for gone in existing.iter().filter(|s| !desired.contains(s)) {
        sync_state::Entity::delete_by_id((
            user_id,
            source.to_string(),
            gone.scope_key.clone(),
            gone.entity_kind.clone(),
        ))
        .exec(db)
        .await?;
    }
    Ok(())
}

fn conflict_pk() -> OnConflict {
    OnConflict::columns([
        sync_state::Column::UserId,
        sync_state::Column::Source,
        sync_state::Column::ScopeKey,
        sync_state::Column::EntityKind,
    ])
}

/// The only `ActiveModel` construction site: a fresh scope, due immediately.
fn new_scope_model(user_id: Uuid, source: &str, s: &ScopeKey) -> sync_state::ActiveModel {
    sync_state::ActiveModel {
        user_id: Set(user_id),
        source: Set(source.to_string()),
        scope_key: Set(s.scope_key.clone()),
        entity_kind: Set(s.entity_kind.clone()),
        cursor: Set(None),
        last_polled_at: Set(None),
        next_poll_at: NotSet, // DB default now()
        consecutive_errors: Set(0),
        last_error: Set(None),
        lease_owner: Set(None),
        lease_until: Set(None),
    }
}

/// Atomically claims up to `batch` due, unleased (or lease-expired) scopes for
/// `owner`, leasing them for `lease_secs` (spec §4.2 step 2).
pub async fn claim_due(
    db: &DatabaseConnection,
    batch: u64,
    owner: &str,
    lease_secs: u64,
) -> Result<Vec<sync_state::Model>, DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"WITH due AS (
             SELECT user_id, source, scope_key, entity_kind FROM sync_state
             WHERE next_poll_at <= now()
               AND (lease_until IS NULL OR lease_until < now())
             ORDER BY next_poll_at LIMIT $1
             FOR UPDATE SKIP LOCKED)
           UPDATE sync_state s
           SET lease_owner = $2, lease_until = now() + make_interval(secs => $3)
           FROM due d
           WHERE (s.user_id, s.source, s.scope_key, s.entity_kind)
               = (d.user_id, d.source, d.scope_key, d.entity_kind)
           RETURNING s.*"#,
        [(batch as i64).into(), owner.into(), (lease_secs as f64).into()],
    );
    let rows = db.query_all_raw(stmt).await?;
    rows.iter().map(|r| sync_state::Model::from_query_result(r, "")).collect()
}

/// Success completion: store the (possibly advanced) cursor, reset errors,
/// clear the lease, schedule the next poll. Owner-guarded (spec §4.2 step 4).
pub async fn complete_ok(
    db: &DatabaseConnection,
    row: &sync_state::Model,
    owner: &str,
    new_cursor: Option<String>,
    next_in_secs: u64,
) -> Result<(), DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"UPDATE sync_state
           SET "cursor" = $5, last_polled_at = now(),
               next_poll_at = now() + make_interval(secs => $6),
               consecutive_errors = 0, last_error = NULL,
               lease_owner = NULL, lease_until = NULL
           WHERE user_id = $1 AND source = $2 AND scope_key = $3
             AND entity_kind = $4 AND lease_owner = $7"#,
        [
            row.user_id.into(),
            row.source.clone().into(),
            row.scope_key.clone().into(),
            row.entity_kind.clone().into(),
            new_cursor.into(),
            (next_in_secs as f64).into(),
            owner.into(),
        ],
    );
    db.execute_raw(stmt).await.map(|_| ())
}

/// Failure completion: record the error, bump `consecutive_errors`, back off
/// by `next_in_secs` (caller computes the backoff), clear the lease.
pub async fn complete_err(
    db: &DatabaseConnection,
    row: &sync_state::Model,
    owner: &str,
    error: &str,
    next_in_secs: u64,
) -> Result<(), DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"UPDATE sync_state
           SET last_polled_at = now(),
               next_poll_at = now() + make_interval(secs => $6),
               consecutive_errors = consecutive_errors + 1, last_error = $5,
               lease_owner = NULL, lease_until = NULL
           WHERE user_id = $1 AND source = $2 AND scope_key = $3
             AND entity_kind = $4 AND lease_owner = $7"#,
        [
            row.user_id.into(),
            row.source.clone().into(),
            row.scope_key.clone().into(),
            row.entity_kind.clone().into(),
            error.into(),
            (next_in_secs as f64).into(),
            owner.into(),
        ],
    );
    db.execute_raw(stmt).await.map(|_| ())
}

/// Opportunistic hook (spec §4.3): pull every scope of this user forward to
/// "due now" so the next tick prioritizes them. No-op for already-due rows.
pub async fn mark_user_due(db: &DatabaseConnection, user_id: Uuid) -> Result<(), DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE sync_state SET next_poll_at = now() \
         WHERE user_id = $1 AND next_poll_at > now()",
        [user_id.into()],
    );
    db.execute_raw(stmt).await.map(|_| ())
}
```

- [ ] **Step 3: `mod.rs`** (same two-line shape as events) and add `pub mod sync_state;` to `crates/db/src/tables/mod.rs`.
- [ ] **Step 4: Verify** `cargo build -p wf-db && cargo clippy -p wf-db --all-targets --locked -- -D warnings`. If `execute_raw`/`query_all_raw` names differ on this sea-orm rc, check with context7 (`/SeaQL/sea-orm`) — CLAUDE.md documents the `*_raw` family as correct for 2.0.
- [ ] **Step 5: Commit** `git add crates/db && git commit -m "A1: sync_state table — reconcile, lease claim, owner-guarded completion"`

---

### Task 5: `list_all` on both connection tables

**Files:** Modify: `crates/db/src/tables/github_pat_connections/crud.rs`, `crates/db/src/tables/jira_pat_connections/crud.rs`

- [ ] **Step 1:** Append to each `crud.rs` (adjust the alias to the file's existing `use super::entity as …` name):

```rust
/// All connections — the tick's reconcile pass enumerates these (A1 §4.2.1).
pub async fn list_all(db: &DatabaseConnection) -> Result<Vec<gh::Model>, DbErr> {
    gh::Entity::find().all(db).await
}
```

(In `jira_pat_connections/crud.rs` the alias is `jira`/similar — return `Vec<jira::Model>`.)

- [ ] **Step 2: Verify** `cargo clippy -p wf-db --all-targets --locked -- -D warnings`.
- [ ] **Step 3: Commit** `git add crates/db && git commit -m "A1: list_all on connection tables"`

---

### Task 6: GitHub poller reads (`wf-github`)

**Files:** Modify: `crates/github/src/client.rs`, `crates/github/src/lib.rs`, `crates/github/Cargo.toml`; Create: `crates/github/src/poll.rs`

- [ ] **Step 1: Base injection in `client.rs`** — needed so wiremock can stand in for `api.github.com`. Change the struct + constructors and use `self.base` in `request` (GraphQL stays hardcoded — the poller is REST-only):

```rust
pub struct GithubClient {
    http: reqwest::Client,
    token: String,
    base: String,
}

impl GithubClient {
    pub fn new(token: impl Into<String>) -> Self {
        Self::with_base(token, REST_BASE)
    }

    /// Test seam: point REST calls at a mock server (A1 §9).
    pub fn with_base(token: impl Into<String>, base: impl Into<String>) -> Self {
        Self { http: reqwest::Client::new(), token: token.into(), base: base.into() }
    }
```

and in `request()`: `.request(method, format!("{}{path}", self.base))`.

- [ ] **Step 2: Run** `cargo test -p wf-github` — expect PASS (no behavior change).
- [ ] **Step 3: dev-deps** in `crates/github/Cargo.toml`:

```toml
[dev-dependencies]
wiremock = "0.6"
tokio = { workspace = true }
serde_json = { workspace = true }
```

- [ ] **Step 4: Write `poll.rs` with failing tests first** — create the file with DTOs + test module (fns stubbed via `todo!()` bodies is NOT allowed by clippy gate; instead write tests + full impl together, run, and iterate). Full file:

```rust
//! Poller-specific repo reads (A1 spec §5): page-1, newest-first by update
//! recency, raw provider ids/timestamps. Deliberately separate from the
//! dashboard queue reads (user-queue shaped, open-PR-only). Cursor filtering
//! happens in `wf-sync`; these fetch one page.
//!
//! Documented limitation (spec §6.2 deviation, plan header): >50 updates per
//! scope per tick lose the older ones; dedup keeps replays safe.

use chrono::{DateTime, Utc};
use reqwest::Method;
use serde::Deserialize;

use crate::client::GithubClient;
use crate::errors::GithubError;

#[derive(Debug, Clone, Deserialize)]
pub struct GithubActor {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolledWorkflowRun {
    pub id: i64,
    #[serde(default = "default_attempt")]
    pub run_attempt: i64,
    pub name: Option<String>,
    pub display_title: Option<String>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub html_url: Option<String>,
    pub head_branch: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub actor: Option<GithubActor>,
}

fn default_attempt() -> i64 {
    1
}

#[derive(Deserialize)]
struct RunsResponse {
    workflow_runs: Vec<PolledWorkflowRun>,
}

/// Newest page of completed workflow runs for `owner/repo`.
pub async fn list_workflow_runs_page(
    client: &GithubClient,
    owner: &str,
    repo: &str,
) -> Result<Vec<PolledWorkflowRun>, GithubError> {
    let path = format!("/repos/{owner}/{repo}/actions/runs");
    let req = client
        .request(Method::GET, &path)
        .query(&[("status", "completed"), ("per_page", "50")]);
    let body: RunsResponse = send_json(req).await?;
    Ok(body.workflow_runs)
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolledPullRequest {
    pub number: i64,
    pub state: String,
    pub title: Option<String>,
    pub html_url: Option<String>,
    pub draft: Option<bool>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub merged_at: Option<DateTime<Utc>>,
    pub user: Option<GithubActor>,
}

/// Newest page of PRs (all states) for `owner/repo`, sorted by update time.
pub async fn list_pulls_page(
    client: &GithubClient,
    owner: &str,
    repo: &str,
) -> Result<Vec<PolledPullRequest>, GithubError> {
    let path = format!("/repos/{owner}/{repo}/pulls");
    let req = client.request(Method::GET, &path).query(&[
        ("state", "all"),
        ("sort", "updated"),
        ("direction", "desc"),
        ("per_page", "50"),
    ]);
    send_json(req).await
}

/// Sends a poller request, mapping transport / non-2xx / parse failures to
/// `GithubError::Api`.
async fn send_json<T: serde::de::DeserializeOwned>(
    req: reqwest::RequestBuilder,
) -> Result<T, GithubError> {
    let resp = req.send().await.map_err(|e| GithubError::Api(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(GithubError::Api(format!("poll HTTP {}", status.as_u16())));
    }
    resp.json().await.map_err(|e| GithubError::Api(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn run_fixture() -> serde_json::Value {
        serde_json::json!({ "workflow_runs": [{
            "id": 42, "run_attempt": 2, "name": "CI",
            "display_title": "Fix the build", "status": "completed",
            "conclusion": "success", "html_url": "https://github.com/o/r/actions/runs/42",
            "head_branch": "main",
            "created_at": "2026-06-01T10:00:00Z", "updated_at": "2026-06-01T10:05:00Z",
            "actor": { "login": "octocat" }
        }]})
    }

    #[tokio::test]
    async fn parses_workflow_runs_page() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/actions/runs"))
            .and(query_param("status", "completed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(run_fixture()))
            .mount(&server)
            .await;
        let client = GithubClient::with_base("t", server.uri());
        let runs = list_workflow_runs_page(&client, "o", "r").await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, 42);
        assert_eq!(runs[0].run_attempt, 2);
        assert_eq!(runs[0].conclusion.as_deref(), Some("success"));
    }

    #[tokio::test]
    async fn parses_pulls_page_and_maps_errors() {
        let server = MockServer::start().await;
        let pulls = serde_json::json!([{
            "number": 7, "state": "closed", "title": "Add feature",
            "html_url": "https://github.com/o/r/pull/7", "draft": false,
            "created_at": "2026-06-01T09:00:00Z", "updated_at": "2026-06-02T09:00:00Z",
            "closed_at": "2026-06-02T09:00:00Z", "merged_at": "2026-06-02T09:00:00Z",
            "user": { "login": "octocat" }
        }]);
        Mock::given(method("GET"))
            .and(path("/repos/o/r/pulls"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pulls))
            .mount(&server)
            .await;
        let client = GithubClient::with_base("t", server.uri());
        let prs = list_pulls_page(&client, "o", "r").await.unwrap();
        assert_eq!(prs[0].number, 7);
        assert!(prs[0].merged_at.is_some());

        let bad = GithubClient::with_base("t", server.uri());
        let err = list_workflow_runs_page(&bad, "missing", "repo").await;
        assert!(err.is_err()); // 404 from unmatched mock
    }
}
```

- [ ] **Step 5:** Export in `lib.rs`: `pub mod poll;` and `pub use poll::{list_pulls_page, list_workflow_runs_page, GithubActor, PolledPullRequest, PolledWorkflowRun};`
- [ ] **Step 6: Run** `cargo test -p wf-github` — expect PASS (note: an unmatched wiremock request returns 404, which `send_json` maps to `Err` — that's what the last assert checks).
- [ ] **Step 7:** `cargo clippy -p wf-github --all-targets --locked -- -D warnings`.
- [ ] **Step 8: Commit** `git add crates/github Cargo.lock && git commit -m "A1: GitHub poller page reads + client base injection"`

---

### Task 7: Jira poller read (`wf-jira`)

**Files:** Create: `crates/jira/src/poll.rs`; Modify: `crates/jira/src/lib.rs`, `crates/jira/Cargo.toml`

- [ ] **Step 1: dev-deps** in `crates/jira/Cargo.toml` (same block as Task 6 Step 3).
- [ ] **Step 2: `poll.rs`** (JiraClient already takes `site_url` from creds — mockable without changes). Uses the exported `quote_jql_string`:

```rust
//! Poller-specific issue read (A1 spec §5): newest page by `updated` for one
//! project, with the raw status id / timestamps the normalizer needs.
//! Cursor filtering happens in `wf-sync`. Relative-ordering DESC (not the
//! spec's ASC sketch): absolute JQL timestamps are interpreted in the
//! account's timezone — a correctness trap; see plan header.

use serde::Deserialize;
use serde_json::json;

use crate::client::JiraClient;
use crate::errors::JiraApiError;
use crate::issues::jql::quote_jql_string;

#[derive(Debug, Clone)]
pub struct PolledIssue {
    pub key: String,
    pub summary: Option<String>,
    pub status_id: Option<String>,
    pub status_name: Option<String>,
    pub status_category: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub url: String,
}

#[derive(Deserialize)]
struct PollSearchResponse {
    issues: Option<Vec<RawPollIssue>>,
}

#[derive(Deserialize)]
struct RawPollIssue {
    key: String,
    fields: RawPollFields,
}

#[derive(Deserialize)]
struct RawPollFields {
    summary: Option<String>,
    status: Option<RawStatus>,
    created: Option<String>,
    updated: Option<String>,
}

#[derive(Deserialize)]
struct RawStatus {
    id: Option<String>,
    name: Option<String>,
    #[serde(rename = "statusCategory")]
    status_category: Option<RawCategory>,
}

#[derive(Deserialize)]
struct RawCategory {
    key: Option<String>,
}

/// JQL for the newest page of a project's issues by update recency.
pub fn recent_issues_jql(project_key: &str) -> String {
    format!("project = {} ORDER BY updated DESC", quote_jql_string(project_key))
}

/// Newest page (≤50) of issues for `project_key` with poller fields.
pub async fn fetch_recent_issues(
    client: &JiraClient,
    project_key: &str,
) -> Result<Vec<PolledIssue>, JiraApiError> {
    let body = json!({
        "jql": recent_issues_jql(project_key),
        "maxResults": 50,
        "fields": ["summary", "status", "created", "updated"],
    });
    let res: PollSearchResponse = client.post("/rest/api/3/search/jql", &body).await?;
    let site = client.site_url().to_string();
    Ok(res.issues.unwrap_or_default().into_iter().map(|r| to_polled(&site, r)).collect())
}

fn to_polled(site_url: &str, raw: RawPollIssue) -> PolledIssue {
    let status = raw.fields.status;
    PolledIssue {
        url: format!("{site_url}/browse/{}", raw.key),
        key: raw.key,
        summary: raw.fields.summary,
        status_id: status.as_ref().and_then(|s| s.id.clone()),
        status_name: status.as_ref().and_then(|s| s.name.clone()),
        status_category: status
            .as_ref()
            .and_then(|s| s.status_category.as_ref())
            .and_then(|c| c.key.clone()),
        created: raw.fields.created,
        updated: raw.fields.updated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::JiraCreds;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn jql_quotes_project_key() {
        assert_eq!(recent_issues_jql("PROJ"), "project = \"PROJ\" ORDER BY updated DESC");
    }

    #[tokio::test]
    async fn parses_poll_page() {
        let server = MockServer::start().await;
        let fixture = serde_json::json!({ "issues": [{
            "key": "PROJ-1",
            "fields": {
                "summary": "Fix login",
                "status": { "id": "3", "name": "In Progress",
                            "statusCategory": { "key": "indeterminate" } },
                "created": "2026-06-01T10:00:00.000+0200",
                "updated": "2026-06-02T11:00:00.000+0200"
            }
        }]});
        Mock::given(method("POST"))
            .and(path("/rest/api/3/search/jql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture))
            .mount(&server)
            .await;
        let creds = JiraCreds {
            site_url: server.uri(),
            email: "e@x.com".into(),
            token: "t".into(),
        };
        let issues = fetch_recent_issues(&JiraClient::new(&creds), "PROJ").await.unwrap();
        assert_eq!(issues[0].key, "PROJ-1");
        assert_eq!(issues[0].status_id.as_deref(), Some("3"));
        assert!(issues[0].url.ends_with("/browse/PROJ-1"));
    }
}
```

If `quote_jql_string` output style differs (single vs double quotes), match the existing function's actual output in the `jql_quotes_project_key` assertion.

- [ ] **Step 3:** Export in `lib.rs`: `pub mod poll;` + `pub use poll::{fetch_recent_issues, recent_issues_jql, PolledIssue};`
- [ ] **Step 4: Run** `cargo test -p wf-jira && cargo clippy -p wf-jira --all-targets --locked -- -D warnings` — expect PASS/clean.
- [ ] **Step 5: Commit** `git add crates/jira Cargo.lock && git commit -m "A1: Jira poller page read"`

---

### Task 8: `wf-sync` crate scaffold + cursors

**Files:** Create: `crates/sync/Cargo.toml`, `crates/sync/src/{lib.rs,cursor.rs}`; Modify: root `Cargo.toml`

- [ ] **Step 1:** Workspace: `members = [..., "crates/sync"]` and under `[workspace.dependencies]`: `wf-sync = { path = "crates/sync" }`.
- [ ] **Step 2: `crates/sync/Cargo.toml`:**

```toml
[package]
name = "wf-sync"
version.workspace = true
edition.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
wf-core = { workspace = true }
wf-db = { workspace = true }
wf-github = { workspace = true }
wf-jira = { workspace = true }
sea-orm = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
chrono = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
url = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
wiremock = "0.6"
dotenvy = { workspace = true }
anyhow = { workspace = true }
```

- [ ] **Step 3: `src/cursor.rs`** — compound cursors (spec §6.2), TDD: write the test module first, watch it fail to compile, then the impl. Full file:

```rust
//! Compound poll cursors (A1 spec §6.2), JSON-encoded into
//! `sync_state."cursor"`. GitHub compares `(updated_at, id)`; Jira compares
//! `(updated-as-instant, issue_key)` — Jira timestamps carry a UTC offset
//! (`2026-06-02T11:00:00.000+0200`), so ordering parses them rather than
//! comparing strings.

use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubCursor {
    pub updated_at: DateTime<Utc>,
    pub id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraCursor {
    /// Raw Jira `updated` string (round-trips exactly; parsed for ordering).
    pub updated: String,
    pub issue_key: String,
}

/// Parses a stored cursor; `None`/garbage → `None` (treated as baseline).
pub fn parse<T: serde::de::DeserializeOwned>(raw: Option<&str>) -> Option<T> {
    raw.and_then(|s| serde_json::from_str(s).ok())
}

/// Encodes a cursor for storage.
pub fn encode<T: Serialize>(cursor: &T) -> String {
    serde_json::to_string(cursor).expect("cursor serializes")
}

impl GithubCursor {
    /// Is `(updated_at, id)` strictly after this cursor?
    pub fn is_before(&self, updated_at: DateTime<Utc>, id: i64) -> bool {
        updated_at > self.updated_at || (updated_at == self.updated_at && id > self.id)
    }
}

/// Parses a Jira timestamp (`%Y-%m-%dT%H:%M:%S%.3f%z`).
pub fn parse_jira_ts(raw: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.3f%z").ok()
}

impl JiraCursor {
    /// Is `(updated, key)` strictly after this cursor? Unparseable timestamps
    /// order as "not after" (skipped; dedup keeps this safe).
    pub fn is_before(&self, updated: &str, key: &str) -> bool {
        let (Some(a), Some(b)) = (parse_jira_ts(&self.updated), parse_jira_ts(updated)) else {
            return false;
        };
        b > a || (b == a && key > self.issue_key.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn gh(ts: i64, id: i64) -> GithubCursor {
        GithubCursor { updated_at: Utc.timestamp_opt(ts, 0).unwrap(), id }
    }

    #[test]
    fn github_ordering_with_tiebreak() {
        let cur = gh(100, 5);
        assert!(cur.is_before(Utc.timestamp_opt(101, 0).unwrap(), 1));
        assert!(cur.is_before(Utc.timestamp_opt(100, 0).unwrap(), 6));
        assert!(!cur.is_before(Utc.timestamp_opt(100, 0).unwrap(), 5));
        assert!(!cur.is_before(Utc.timestamp_opt(99, 0).unwrap(), 9));
    }

    #[test]
    fn jira_ordering_across_offsets() {
        let cur = JiraCursor {
            updated: "2026-06-02T11:00:00.000+0200".into(), // 09:00Z
            issue_key: "PROJ-1".into(),
        };
        assert!(cur.is_before("2026-06-02T10:00:00.000+0000", "PROJ-1")); // 10:00Z
        assert!(!cur.is_before("2026-06-02T08:00:00.000+0000", "PROJ-9")); // 08:00Z
        assert!(cur.is_before("2026-06-02T11:00:00.000+0200", "PROJ-2")); // tie → key
        assert!(!cur.is_before("garbage", "PROJ-2"));
    }

    #[test]
    fn round_trips_through_json() {
        let cur = gh(100, 5);
        let parsed: GithubCursor = parse(Some(&encode(&cur))).unwrap();
        assert_eq!(parsed, cur);
        assert_eq!(parse::<GithubCursor>(Some("not json")), None);
        assert_eq!(parse::<GithubCursor>(None), None);
    }
}
```

- [ ] **Step 4: `src/lib.rs`** (grows in later tasks):

```rust
//! Event-backbone sync engine (A1 spec): cursors, normalizers, and the tick
//! routine. A library so both `wf-api` (`POST /internal/tick`) and the future
//! `wf-worker` (roadmap Approach 2) share the exact same logic.

pub mod cursor;
```

- [ ] **Step 5: Run** `cargo test -p wf-sync && cargo clippy -p wf-sync --all-targets --locked -- -D warnings` — expect PASS/clean.
- [ ] **Step 6: Commit** `git add Cargo.toml Cargo.lock crates/sync && git commit -m "A1: wf-sync crate — compound cursors"`

---

### Task 9: Normalizers (`wf-sync`)

**Files:** Create: `crates/sync/src/normalize.rs`; Modify: `crates/sync/src/lib.rs`

- [ ] **Step 1: `src/normalize.rs`** — pure provider-DTO → `InsertEventInput` functions (spec §5.1, §6.1). Payload keys are camelCase; `issueKey`/`statusId` are load-bearing (queried by `latest_jira_status`). Full file:

```rust
//! Pure normalizers (A1 spec §5.1): provider DTO → `events` row input.
//! `external_id` forms are spec §6.1 — they encode provider instance + entity
//! + state, so re-observations collide (deduped) and state changes don't.

use sea_orm::prelude::Uuid;
use serde_json::json;
use wf_db::tables::events::InsertEventInput;
use wf_github::{PolledPullRequest, PolledWorkflowRun};
use wf_jira::PolledIssue;

use crate::cursor::parse_jira_ts;

/// `github.workflow_run.completed` for a completed run; `None` otherwise.
pub fn workflow_run_event(
    user_id: Uuid,
    repo: &str,
    run: &PolledWorkflowRun,
) -> Option<InsertEventInput> {
    if run.status.as_deref() != Some("completed") {
        return None;
    }
    let conclusion = run.conclusion.clone().unwrap_or_else(|| "unknown".into());
    Some(InsertEventInput {
        user_id,
        source: "github".into(),
        event_type: "github.workflow_run.completed".into(),
        external_id: format!("repo:{repo}:wfrun:{}:{}:{conclusion}", run.id, run.run_attempt),
        scope_key: repo.to_string(),
        actor: run.actor.as_ref().map(|a| a.login.clone()),
        title: run.display_title.clone().or_else(|| run.name.clone()),
        url: run.html_url.clone(),
        occurred_at: run.updated_at.fixed_offset(),
        payload: json!({
            "runId": run.id,
            "runAttempt": run.run_attempt,
            "conclusion": conclusion,
            "headBranch": run.head_branch,
            "name": run.name,
        }),
    })
}

/// `github.pull_request.{opened|merged|closed}` from the PR's current state.
pub fn pull_request_event(
    user_id: Uuid,
    repo: &str,
    pr: &PolledPullRequest,
) -> Option<InsertEventInput> {
    let (state, event_type, occurred_at) = match (pr.state.as_str(), pr.merged_at) {
        ("open", _) => ("open", "github.pull_request.opened", pr.created_at),
        ("closed", Some(merged)) => ("merged", "github.pull_request.merged", merged),
        ("closed", None) => {
            ("closed", "github.pull_request.closed", pr.closed_at.unwrap_or(pr.updated_at))
        }
        _ => return None,
    };
    Some(InsertEventInput {
        user_id,
        source: "github".into(),
        event_type: event_type.into(),
        external_id: format!("repo:{repo}:pr:{}:{state}", pr.number),
        scope_key: repo.to_string(),
        actor: pr.user.as_ref().map(|a| a.login.clone()),
        title: pr.title.clone(),
        url: pr.html_url.clone(),
        occurred_at: occurred_at.fixed_offset(),
        payload: json!({
            "number": pr.number,
            "state": state,
            "draft": pr.draft,
            "title": pr.title,
        }),
    })
}

/// `jira.issue.created` (no prior ingested status) or
/// `jira.issue.transitioned` (status differs); `None` when unchanged or the
/// issue lacks a status id (spec §5.1).
pub fn jira_issue_event(
    user_id: Uuid,
    project: &str,
    site_key: &str,
    prev_status_id: Option<&str>,
    issue: &PolledIssue,
) -> Option<InsertEventInput> {
    let status_id = issue.status_id.as_deref()?;
    let (event_type, external_id) = match prev_status_id {
        None => ("jira.issue.created", format!("site:{site_key}:issue:{}:created", issue.key)),
        Some(prev) if prev == status_id => return None,
        Some(_) => (
            "jira.issue.transitioned",
            format!(
                "site:{site_key}:issue:{}:status:{status_id}:updated:{}",
                issue.key,
                issue.updated.as_deref().unwrap_or("unknown")
            ),
        ),
    };
    Some(InsertEventInput {
        user_id,
        source: "jira".into(),
        event_type: event_type.into(),
        external_id,
        scope_key: project.to_string(),
        actor: None,
        title: issue.summary.clone(),
        url: Some(issue.url.clone()),
        occurred_at: occurred_ts(event_type, issue),
        payload: json!({
            "issueKey": issue.key,
            "statusId": status_id,
            "statusName": issue.status_name,
            "statusCategory": issue.status_category,
            "summary": issue.summary,
        }),
    })
}

/// `created` events use the issue's creation time; transitions use `updated`.
/// Unparseable/missing provider times fall back to "now" (ingestion time).
fn occurred_ts(event_type: &str, issue: &PolledIssue) -> sea_orm::prelude::DateTimeWithTimeZone {
    let raw = if event_type == "jira.issue.created" { &issue.created } else { &issue.updated };
    raw.as_deref()
        .and_then(parse_jira_ts)
        .unwrap_or_else(|| chrono::Utc::now().fixed_offset())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use wf_github::GithubActor;

    fn run(status: &str, conclusion: Option<&str>) -> PolledWorkflowRun {
        PolledWorkflowRun {
            id: 42,
            run_attempt: 2,
            name: Some("CI".into()),
            display_title: Some("Fix build".into()),
            status: Some(status.into()),
            conclusion: conclusion.map(Into::into),
            html_url: Some("https://x/runs/42".into()),
            head_branch: Some("main".into()),
            created_at: Utc.timestamp_opt(100, 0).unwrap(),
            updated_at: Utc.timestamp_opt(200, 0).unwrap(),
            actor: Some(GithubActor { login: "octocat".into() }),
        }
    }

    #[test]
    fn workflow_run_only_when_completed() {
        let uid = Uuid::new_v4();
        let ev = workflow_run_event(uid, "o/r", &run("completed", Some("failure"))).unwrap();
        assert_eq!(ev.external_id, "repo:o/r:wfrun:42:2:failure");
        assert_eq!(ev.event_type, "github.workflow_run.completed");
        assert_eq!(ev.payload["conclusion"], "failure");
        assert!(workflow_run_event(uid, "o/r", &run("in_progress", None)).is_none());
    }

    fn pr(state: &str, merged: bool) -> PolledPullRequest {
        PolledPullRequest {
            number: 7,
            state: state.into(),
            title: Some("Add feature".into()),
            html_url: Some("https://x/pull/7".into()),
            draft: Some(false),
            created_at: Utc.timestamp_opt(100, 0).unwrap(),
            updated_at: Utc.timestamp_opt(300, 0).unwrap(),
            closed_at: Some(Utc.timestamp_opt(300, 0).unwrap()),
            merged_at: merged.then(|| Utc.timestamp_opt(300, 0).unwrap()),
            user: Some(GithubActor { login: "octocat".into() }),
        }
    }

    #[test]
    fn pull_request_state_mapping() {
        let uid = Uuid::new_v4();
        assert_eq!(pull_request_event(uid, "o/r", &pr("open", false)).unwrap().external_id, "repo:o/r:pr:7:open");
        assert_eq!(
            pull_request_event(uid, "o/r", &pr("closed", true)).unwrap().event_type,
            "github.pull_request.merged"
        );
        assert_eq!(
            pull_request_event(uid, "o/r", &pr("closed", false)).unwrap().external_id,
            "repo:o/r:pr:7:closed"
        );
    }

    fn issue(status_id: &str) -> PolledIssue {
        PolledIssue {
            key: "PROJ-1".into(),
            summary: Some("Fix login".into()),
            status_id: Some(status_id.into()),
            status_name: Some("In Progress".into()),
            status_category: Some("indeterminate".into()),
            created: Some("2026-06-01T10:00:00.000+0200".into()),
            updated: Some("2026-06-02T11:00:00.000+0200".into()),
            url: "https://x/browse/PROJ-1".into(),
        }
    }

    #[test]
    fn jira_created_transitioned_unchanged() {
        let uid = Uuid::new_v4();
        let created = jira_issue_event(uid, "PROJ", "site1", None, &issue("3")).unwrap();
        assert_eq!(created.event_type, "jira.issue.created");
        assert_eq!(created.external_id, "site:site1:issue:PROJ-1:created");
        assert_eq!(created.payload["issueKey"], "PROJ-1");
        assert_eq!(created.payload["statusId"], "3");

        let moved = jira_issue_event(uid, "PROJ", "site1", Some("2"), &issue("3")).unwrap();
        assert_eq!(moved.event_type, "jira.issue.transitioned");
        assert!(moved.external_id.contains(":status:3:updated:"));

        assert!(jira_issue_event(uid, "PROJ", "site1", Some("3"), &issue("3")).is_none());
    }
}
```

- [ ] **Step 2:** `lib.rs`: add `pub mod normalize;`
- [ ] **Step 3: Run** `cargo test -p wf-sync` — expect PASS. Note: `Uuid::new_v4` in tests requires the `uuid` crate's `v4` feature; if it fails to resolve, add `uuid = { version = "1", features = ["v4"] }` to `[dev-dependencies]` and use `uuid::Uuid` in tests (it's the same type sea-orm re-exports).
- [ ] **Step 4:** `cargo clippy -p wf-sync --all-targets --locked -- -D warnings`.
- [ ] **Step 5: Commit** `git add crates/sync Cargo.lock && git commit -m "A1: normalizers (workflow runs, PRs, Jira issues)"`

---

### Task 10: The tick routine (`wf-sync`)

**Files:** Create: `crates/sync/src/tick.rs`; Modify: `crates/sync/src/lib.rs`

This is the largest file; every function must stay ≤25 lines — the decomposition below is designed for that.

- [ ] **Step 1: `src/tick.rs`** — full file:

```rust
//! The wake-up tick (A1 spec §4): reconcile scopes → claim due → poll each →
//! advance/back off. Bounded by batch + wall-clock budget; idempotent; safe
//! to run concurrently (leases). Shared by `wf-api` and the future worker.

use std::time::{Duration, Instant};

use sea_orm::prelude::Uuid;
use sea_orm::DbErr;
use serde::Serialize;
use wf_core::{Sealed, TokenCipher};
use wf_db::tables::{events, github_pat_connections as gh, jira_pat_connections as jira, sync_state};
use wf_db::Db;
use wf_github::{GithubClient, PolledPullRequest, PolledWorkflowRun};
use wf_jira::{JiraClient, JiraCreds};

use crate::cursor::{self, GithubCursor, JiraCursor};
use crate::normalize;

#[derive(Debug, Clone)]
pub struct TickOptions {
    pub batch: u64,
    pub budget: Duration,
    pub lease_secs: u64,
    pub poll_interval_secs: u64,
    /// Identifies this tick run in `lease_owner`.
    pub owner: String,
    /// Test seam: override the GitHub REST base (wiremock).
    pub github_base: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TickSummary {
    pub scopes_claimed: usize,
    pub scopes_ok: usize,
    pub scopes_failed: usize,
    pub events_written: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum TickError {
    #[error("tick db error: {0}")]
    Db(#[from] DbErr),
}

/// One bounded tick (spec §4.2). Errors inside a scope are isolated (recorded
/// + backed off); only DB-level failures abort the tick.
pub async fn run_tick(db: &Db, cipher: &TokenCipher, opts: &TickOptions) -> Result<TickSummary, TickError> {
    reconcile_all(db).await?;
    let claimed = sync_state::claim_due(db, opts.batch, &opts.owner, opts.lease_secs).await?;
    let started = Instant::now();
    let mut summary = TickSummary { scopes_claimed: claimed.len(), ..TickSummary::default() };
    for scope in &claimed {
        if started.elapsed() >= opts.budget {
            break; // unprocessed leases expire and are re-claimable (spec §8)
        }
        step_scope(db, cipher, scope, opts, &mut summary).await?;
    }
    Ok(summary)
}

/// Polls one claimed scope and records the outcome on `sync_state`.
async fn step_scope(
    db: &Db,
    cipher: &TokenCipher,
    scope: &sync_state::Model,
    opts: &TickOptions,
    summary: &mut TickSummary,
) -> Result<(), TickError> {
    match process_scope(db, cipher, scope, opts).await {
        Ok(ScopeOutcome { written, new_cursor }) => {
            let cursor = new_cursor.or_else(|| scope.cursor.clone());
            sync_state::complete_ok(db, scope, &opts.owner, cursor, opts.poll_interval_secs).await?;
            summary.scopes_ok += 1;
            summary.events_written += written;
        }
        Err(ScopeError::Db(e)) => return Err(e.into()),
        Err(ScopeError::Poll(msg)) => {
            let backoff = backoff_secs(opts.poll_interval_secs, scope.consecutive_errors);
            tracing::warn!(scope = %scope.scope_key, kind = %scope.entity_kind, error = %msg, "scope poll failed");
            sync_state::complete_err(db, scope, &opts.owner, &msg, backoff).await?;
            summary.scopes_failed += 1;
        }
    }
    Ok(())
}

struct ScopeOutcome {
    written: u64,
    new_cursor: Option<String>,
}

/// Scope-level failure split: `Db` aborts the tick, `Poll` is isolated.
enum ScopeError {
    Db(DbErr),
    Poll(String),
}

impl From<DbErr> for ScopeError {
    fn from(e: DbErr) -> Self {
        ScopeError::Db(e)
    }
}

/// Exponential backoff capped at 1h: `interval * 2^min(errors, 5)`.
pub fn backoff_secs(interval: u64, consecutive_errors: i32) -> u64 {
    let exp = consecutive_errors.clamp(0, 5) as u32;
    (interval.saturating_mul(2u64.saturating_pow(exp))).min(3600)
}

/// Rebuilds `sync_state` rows from every connection's selections (spec §4.2.1).
/// Connections whose PAT is known-invalid get their scopes removed (spec §8).
async fn reconcile_all(db: &Db) -> Result<(), DbErr> {
    for conn in gh::list_all(db).await? {
        let desired = github_scopes(&conn);
        sync_state::replace_scopes(db, conn.user_id, "github", &desired).await?;
    }
    for conn in jira::list_all(db).await? {
        let desired = jira_scopes(&conn);
        sync_state::replace_scopes(db, conn.user_id, "jira", &desired).await?;
    }
    Ok(())
}

fn github_scopes(conn: &gh::Model) -> Vec<sync_state::ScopeKey> {
    if conn.validation_status == "invalid" {
        return vec![];
    }
    json_strings(conn.selected_repos.as_ref())
        .into_iter()
        .flat_map(|repo| {
            ["workflow_run", "pull_request"].map(|kind| sync_state::ScopeKey {
                scope_key: repo.clone(),
                entity_kind: kind.to_string(),
            })
        })
        .collect()
}

fn jira_scopes(conn: &jira::Model) -> Vec<sync_state::ScopeKey> {
    if conn.validation_status == "invalid" {
        return vec![];
    }
    json_strings(conn.selected_projects.as_ref())
        .into_iter()
        .map(|p| sync_state::ScopeKey { scope_key: p, entity_kind: "issue".to_string() })
        .collect()
}

/// `Option<Json>` → `Vec<String>` (selected repos/projects are JSON arrays).
fn json_strings(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(|j| serde_json::from_value(j.clone()).ok()).unwrap_or_default()
}

/// Routes a claimed scope to its source-specific poll.
async fn process_scope(
    db: &Db,
    cipher: &TokenCipher,
    scope: &sync_state::Model,
    opts: &TickOptions,
) -> Result<ScopeOutcome, ScopeError> {
    match scope.source.as_str() {
        "github" => process_github(db, cipher, scope, opts).await,
        "jira" => process_jira(db, cipher, scope).await,
        other => Err(ScopeError::Poll(format!("unknown source {other:?}"))),
    }
}

async fn process_github(
    db: &Db,
    cipher: &TokenCipher,
    scope: &sync_state::Model,
    opts: &TickOptions,
) -> Result<ScopeOutcome, ScopeError> {
    let Some(conn) = gh::select_row(db, scope.user_id).await? else {
        // Connection deleted: scope rows are orphans — self-heal.
        sync_state::replace_scopes(db, scope.user_id, "github", &[]).await?;
        return Ok(ScopeOutcome { written: 0, new_cursor: None });
    };
    let token = open_sealed(cipher, &conn.access_token_ciphertext, &conn.access_token_iv, &conn.access_token_auth_tag)
        .map_err(ScopeError::Poll)?;
    let client = match &opts.github_base {
        Some(base) => GithubClient::with_base(token, base.clone()),
        None => GithubClient::new(token),
    };
    let (owner, repo) = split_repo(&scope.scope_key).map_err(ScopeError::Poll)?;
    let cur: Option<GithubCursor> = cursor::parse(scope.cursor.as_deref());
    poll_github_kind(db, &client, scope, &owner, &repo, cur).await
}

/// Fetches the page for the scope's entity kind, filters by cursor, inserts.
async fn poll_github_kind(
    db: &Db,
    client: &GithubClient,
    scope: &sync_state::Model,
    owner: &str,
    repo: &str,
    cur: Option<GithubCursor>,
) -> Result<ScopeOutcome, ScopeError> {
    match scope.entity_kind.as_str() {
        "workflow_run" => {
            let items = wf_github::list_workflow_runs_page(client, owner, repo)
                .await
                .map_err(|e| ScopeError::Poll(e.to_string()))?;
            let (new, next) = filter_new(cur.as_ref(), &items, |r| (r.updated_at, r.id));
            let events = collect_runs(scope.user_id, &scope.scope_key, &new);
            insert(db, events, next).await
        }
        "pull_request" => {
            let items = wf_github::list_pulls_page(client, owner, repo)
                .await
                .map_err(|e| ScopeError::Poll(e.to_string()))?;
            let (new, next) = filter_new(cur.as_ref(), &items, |p| (p.updated_at, p.number));
            let events = collect_pulls(scope.user_id, &scope.scope_key, &new);
            insert(db, events, next).await
        }
        other => Err(ScopeError::Poll(format!("unknown github kind {other:?}"))),
    }
}

fn collect_runs(
    user_id: Uuid,
    repo: &str,
    items: &[&PolledWorkflowRun],
) -> Vec<events::InsertEventInput> {
    items.iter().filter_map(|r| normalize::workflow_run_event(user_id, repo, r)).collect()
}

fn collect_pulls(
    user_id: Uuid,
    repo: &str,
    items: &[&PolledPullRequest],
) -> Vec<events::InsertEventInput> {
    items.iter().filter_map(|p| normalize::pull_request_event(user_id, repo, p)).collect()
}

async fn insert(
    db: &Db,
    events: Vec<events::InsertEventInput>,
    next: Option<GithubCursor>,
) -> Result<ScopeOutcome, ScopeError> {
    let written = events::insert_ignore_dups(db, events).await?;
    Ok(ScopeOutcome { written, new_cursor: next.as_ref().map(cursor::encode) })
}

/// Cursor filter (spec §6.2): with a cursor, keep items strictly after it; on
/// the **baseline poll (no cursor) emit nothing** — just establish the
/// watermark. Returns the kept items + the max `(updated, id)` over ALL items.
pub(crate) fn filter_new<'a, T>(
    cur: Option<&GithubCursor>,
    items: &'a [T],
    key: impl Fn(&T) -> (chrono::DateTime<chrono::Utc>, i64),
) -> (Vec<&'a T>, Option<GithubCursor>) {
    let max = items
        .iter()
        .map(&key)
        .max()
        .map(|(updated_at, id)| GithubCursor { updated_at, id });
    let next = match (&max, cur) {
        (Some(m), Some(c)) if !c.is_before(m.updated_at, m.id) => Some(c.clone()),
        (None, Some(c)) => Some(c.clone()),
        _ => max,
    };
    let kept = match cur {
        None => vec![],
        Some(c) => items.iter().filter(|t| { let (u, i) = key(t); c.is_before(u, i) }).collect(),
    };
    (kept, next)
}

async fn process_jira(
    db: &Db,
    cipher: &TokenCipher,
    scope: &sync_state::Model,
) -> Result<ScopeOutcome, ScopeError> {
    let Some(conn) = jira::select_row(db, scope.user_id).await? else {
        sync_state::replace_scopes(db, scope.user_id, "jira", &[]).await?;
        return Ok(ScopeOutcome { written: 0, new_cursor: None });
    };
    let token = open_sealed(cipher, &conn.api_token_ciphertext, &conn.api_token_iv, &conn.api_token_auth_tag)
        .map_err(ScopeError::Poll)?;
    let creds = JiraCreds { site_url: conn.site_url.clone(), email: conn.email.clone(), token };
    let site_key = conn.cloud_id.clone().unwrap_or_else(|| host_of(&conn.site_url));
    let client = JiraClient::new(&creds);
    let issues = wf_jira::fetch_recent_issues(&client, &scope.scope_key)
        .await
        .map_err(|e| ScopeError::Poll(e.to_string()))?;
    let cur: Option<JiraCursor> = cursor::parse(scope.cursor.as_deref());
    let (new, next) = filter_new_jira(cur.as_ref(), &issues);
    let events = collect_issues(db, scope, &site_key, &new).await?;
    let written = events::insert_ignore_dups(db, events).await?;
    Ok(ScopeOutcome { written, new_cursor: next.as_ref().map(cursor::encode) })
}

/// Jira flavor of [`filter_new`]: same baseline + strictly-after semantics
/// over `(updated, key)`.
pub(crate) fn filter_new_jira<'a>(
    cur: Option<&JiraCursor>,
    issues: &'a [wf_jira::PolledIssue],
) -> (Vec<&'a wf_jira::PolledIssue>, Option<JiraCursor>) {
    let max = issues
        .iter()
        .filter_map(|i| i.updated.as_deref().map(|u| (u.to_string(), i.key.clone())))
        .max_by(|a, b| cmp_jira(&a.0, &a.1, &b.0, &b.1))
        .map(|(updated, issue_key)| JiraCursor { updated, issue_key });
    let next = match (&max, cur) {
        (Some(m), Some(c)) if !c.is_before(&m.updated, &m.issue_key) => Some(c.clone()),
        (None, Some(c)) => Some(c.clone()),
        _ => max,
    };
    let kept = match cur {
        None => vec![],
        Some(c) => issues
            .iter()
            .filter(|i| i.updated.as_deref().is_some_and(|u| c.is_before(u, &i.key)))
            .collect(),
    };
    (kept, next)
}

fn cmp_jira(a_ts: &str, a_key: &str, b_ts: &str, b_key: &str) -> std::cmp::Ordering {
    use crate::cursor::parse_jira_ts;
    match (parse_jira_ts(a_ts), parse_jira_ts(b_ts)) {
        (Some(a), Some(b)) => a.cmp(&b).then_with(|| a_key.cmp(b_key)),
        (a, b) => a.is_some().cmp(&b.is_some()).then_with(|| a_key.cmp(b_key)),
    }
}

/// Normalizes new issues, looking up each issue's last ingested status
/// (spec §5.1) — sequential; page is ≤50.
async fn collect_issues(
    db: &Db,
    scope: &sync_state::Model,
    site_key: &str,
    issues: &[&wf_jira::PolledIssue],
) -> Result<Vec<events::InsertEventInput>, ScopeError> {
    let mut out = Vec::new();
    for issue in issues {
        let prev =
            events::latest_jira_status(db, scope.user_id, &scope.scope_key, &issue.key).await?;
        if let Some(ev) = normalize::jira_issue_event(
            scope.user_id,
            &scope.scope_key,
            site_key,
            prev.as_deref(),
            issue,
        ) {
            out.push(ev);
        }
    }
    Ok(out)
}

/// Decrypts a sealed PAT, mapping crypto failures to a scope error string.
fn open_sealed(
    cipher: &TokenCipher,
    ciphertext: &str,
    iv: &str,
    auth_tag: &str,
) -> Result<String, String> {
    let sealed = Sealed {
        ciphertext: ciphertext.to_string(),
        iv: iv.to_string(),
        auth_tag: auth_tag.to_string(),
    };
    cipher.open(&sealed).map_err(|e| format!("token decrypt failed: {e}"))
}

/// `"owner/repo"` → `(owner, repo)`.
fn split_repo(scope_key: &str) -> Result<(String, String), String> {
    scope_key
        .split_once('/')
        .map(|(o, r)| (o.to_string(), r.to_string()))
        .ok_or_else(|| format!("malformed repo scope {scope_key:?}"))
}

fn host_of(site_url: &str) -> String {
    url::Url::parse(site_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| site_url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff_secs(120, 0), 120);
        assert_eq!(backoff_secs(120, 1), 240);
        assert_eq!(backoff_secs(120, 3), 960);
        assert_eq!(backoff_secs(120, 50), 3600);
    }

    #[test]
    fn filter_new_baseline_emits_nothing_but_advances() {
        let items = vec![(Utc.timestamp_opt(100, 0).unwrap(), 1i64)];
        let (kept, next) = filter_new(None, &items, |t| *t);
        assert!(kept.is_empty());
        assert_eq!(next.unwrap().id, 1);
    }

    #[test]
    fn filter_new_keeps_strictly_after_and_never_regresses() {
        let cur = GithubCursor { updated_at: Utc.timestamp_opt(100, 0).unwrap(), id: 5 };
        let items = vec![
            (Utc.timestamp_opt(99, 0).unwrap(), 9i64),
            (Utc.timestamp_opt(100, 0).unwrap(), 5),
            (Utc.timestamp_opt(100, 0).unwrap(), 6),
        ];
        let (kept, next) = filter_new(Some(&cur), &items, |t| *t);
        assert_eq!(kept.len(), 1);
        let next = next.unwrap();
        assert_eq!((next.updated_at, next.id), (Utc.timestamp_opt(100, 0).unwrap(), 6));

        let older = vec![(Utc.timestamp_opt(50, 0).unwrap(), 1i64)];
        let (kept, next) = filter_new(Some(&cur), &older, |t| *t);
        assert!(kept.is_empty());
        assert_eq!(next.unwrap(), cur); // never regress below the cursor
    }

    #[test]
    fn split_repo_and_host() {
        assert_eq!(split_repo("o/r").unwrap(), ("o".into(), "r".into()));
        assert!(split_repo("nope").is_err());
        assert_eq!(host_of("https://x.atlassian.net"), "x.atlassian.net");
    }
}
```

- [ ] **Step 2:** `lib.rs` — add `pub mod tick;` and `pub use tick::{run_tick, TickError, TickOptions, TickSummary};`. Check `wf_core` exports `Sealed` (it's in `crypto.rs`; re-export if needed).
- [ ] **Step 3: Run** `cargo test -p wf-sync` — expect PASS. Field-name mismatches against the connection `Model`s (e.g. `api_token_*` vs `access_token_*` on jira) are likely — fix against `crates/db/src/tables/*_pat_connections/entity.rs`, not by guessing.
- [ ] **Step 4:** `cargo clippy -p wf-sync --all-targets --locked -- -D warnings` — split any function that grew past 25 lines.
- [ ] **Step 5: Commit** `git add crates/sync && git commit -m "A1: tick routine — reconcile, claim, poll, advance"`

---

### Task 11: `POST /internal/tick` (wf-api)

**Files:** Create: `crates/api/src/routes/internal.rs`; Modify: `crates/api/src/routes/mod.rs`, `crates/api/src/main.rs`, `crates/api/Cargo.toml`

- [ ] **Step 1:** `crates/api/Cargo.toml`: add `wf-sync = { workspace = true }` under `[dependencies]`.
- [ ] **Step 2: `routes/internal.rs`** — full file. The token check is a pure fn (unit-testable without a DB); the endpoint is deliberately **not** annotated with utoipa (internal, not part of the client API — the committed openapi.json must not change):

```rust
//! `POST /internal/tick` (A1 spec §4.1): shared-secret-authenticated entry
//! point for the tick. Cloud Scheduler calls it on a ~1–2 min cron with the
//! `X-Internal-Token` header. Not part of the public OpenAPI surface.

use actix_web::{web, HttpRequest, HttpResponse};
use wf_sync::TickOptions;

use crate::error::AppError;
use crate::state::AppState;

/// Constant-shape comparison of the provided header against the configured
/// secret. (Both sides are operator-controlled secrets; a timing-safe compare
/// is not load-bearing here, but reject on any mismatch.)
fn token_ok(provided: Option<&str>, expected: &str) -> bool {
    provided.is_some_and(|p| !expected.is_empty() && p == expected)
}

fn tick_options(state: &AppState) -> TickOptions {
    TickOptions {
        batch: state.config.tick_batch_size,
        budget: std::time::Duration::from_millis(state.config.tick_budget_ms),
        lease_secs: state.config.tick_lease_secs,
        poll_interval_secs: state.config.poll_interval_secs,
        owner: format!("api-{}-{}", std::process::id(), chrono::Utc::now().timestamp_millis()),
        github_base: None,
    }
}

/// `POST /internal/tick` → `TickSummary` JSON (camelCase) for Scheduler logs.
pub(crate) async fn tick(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let provided = req.headers().get("X-Internal-Token").and_then(|v| v.to_str().ok());
    if !token_ok(provided, &state.config.internal_tick_token) {
        return Err(AppError::auth("invalid internal token"));
    }
    let summary = wf_sync::run_tick(&state.db, &state.cipher, &tick_options(&state))
        .await
        .map_err(AppError::internal)?;
    Ok(HttpResponse::Ok().json(summary))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/internal/tick", web::post().to(tick));
}

#[cfg(test)]
mod tests {
    use super::token_ok;

    #[test]
    fn token_check() {
        assert!(token_ok(Some("s3cret"), "s3cret"));
        assert!(!token_ok(Some("wrong"), "s3cret"));
        assert!(!token_ok(None, "s3cret"));
        assert!(!token_ok(Some(""), ""));
    }
}
```

- [ ] **Step 3: Mount at the root** (NOT under `/api` — the spec path is `/internal/tick`). In `main.rs`, next to the existing `.service(web::scope("/api").configure(routes::configure))`:

```rust
            .configure(crate::routes::internal::configure)
```

and in `routes/mod.rs`: `pub mod internal;` (do **not** call it from the `/api`-scoped `configure`).

- [ ] **Step 4: Verify** `cargo test -p wf-api && cargo clippy -p wf-api --all-targets --locked -- -D warnings`. If `AppError::internal`'s signature doesn't accept `TickError` directly, wrap: `.map_err(|e| AppError::internal(anyhow::anyhow!(e)))`.
- [ ] **Step 5: Spec-drift guard** — regenerate/compare the committed `openapi.json` the same way CI does; it must be unchanged.
- [ ] **Step 6: Smoke locally:** `cargo run -p wf-api` then
  `curl -s -X POST localhost:3000/internal/tick -H "X-Internal-Token: $(grep INTERNAL_TICK_TOKEN .env | cut -d= -f2)"` → `{"scopesClaimed":…}` JSON; and without the header → 401 problem+json.
- [ ] **Step 7: Commit** `git add crates/api Cargo.lock && git commit -m "A1: POST /internal/tick"`

---

### Task 12: Opportunistic mark hook (wf-api)

**Files:** Modify: the GitHub dashboard GET handler in `crates/api/src/github/routes.rs` (the route that serves the dashboard read)

- [ ] **Step 1:** At the top of the dashboard handler (after the user is authenticated, before the read), add a fire-and-forget mark — `mark_user_due` covers ALL the user's scopes (both sources), so one call site suffices (spec §4.3):

```rust
    // Opportunistic tick hint (A1 spec §4.3): pull this user's scopes forward
    // so the next tick prioritizes them. Fire-and-forget; never blocks reads.
    let mark_db = state.db.clone();
    let mark_user = user_id;
    actix_web::rt::spawn(async move {
        if let Err(e) = wf_db::tables::sync_state::mark_user_due(&mark_db, mark_user).await {
            tracing::debug!(error = %e, "mark_user_due failed");
        }
    });
```

- [ ] **Step 2: Verify** `cargo clippy -p wf-api --all-targets --locked -- -D warnings && cargo test -p wf-api`.
- [ ] **Step 3: Commit** `git add crates/api && git commit -m "A1: opportunistic mark-overdue on dashboard read"`

---

### Task 13: `tick_smoke` example (wf-sync)

**Files:** Create: `crates/sync/examples/tick_smoke.rs`

- [ ] **Step 1:** Full file (existing live-harness style: needs `.env` + a connected user):

```rust
//! Live smoke (A1 spec §9): one tick against the real DB + providers.
//! Run: `cargo run -p wf-sync --example tick_smoke`

use std::time::Duration;

use wf_core::{Config, TokenCipher};
use wf_sync::TickOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cfg = Config::load()?;
    let db = wf_db::connect(&cfg.database_url, wf_db::ConnectOptions::default()).await?;
    let cipher = TokenCipher::new(&cfg.encryption_key_bytes()?);
    let opts = TickOptions {
        batch: cfg.tick_batch_size,
        budget: Duration::from_millis(cfg.tick_budget_ms),
        lease_secs: cfg.tick_lease_secs,
        poll_interval_secs: cfg.poll_interval_secs,
        owner: "tick_smoke".to_string(),
        github_base: None,
    };
    let summary = wf_sync::run_tick(&db, &cipher, &opts).await?;
    println!("tick summary: {summary:?}");
    println!("(first run per scope is the baseline — re-run after new activity to see events)");
    Ok(())
}
```

- [ ] **Step 2:** `cargo build -p wf-sync --examples` — success. (Error types must compose with anyhow: `ConfigError`/`DbError`/`TickError` all implement `std::error::Error` via thiserror, so `?` works.)
- [ ] **Step 3: Run it twice** (`cargo run -p wf-sync --example tick_smoke`): first run = baselines (`eventsWritten: 0`), second shortly after = 0 or more depending on activity. Verify rows: `sync_state` has rows for your selected repos/projects (check via a quick example or Supabase console).
- [ ] **Step 4: Commit** `git add crates/sync && git commit -m "A1: tick_smoke live harness"`

---

### Task 14: Env-gated DB integration test (wf-sync)

**Files:** Create: `crates/sync/tests/tick_db.rs`

- [ ] **Step 1:** Full file. Skips without `DATABASE_URL` (CI). Uses a throwaway user (FK target) + wiremock GitHub; cleans up via `users` CASCADE. Check `UpsertPatInput`'s real fields in `github_pat_connections/crud.rs` and adjust the seeding helper accordingly — the shape below names the fields from the schema (§6.3) but the input struct is authoritative:

```rust
//! A1 spec §9 integration: tick against wiremock GitHub + the real DB.
//! Env-gated: skips unless DATABASE_URL is set (live-harness pattern).

use std::time::Duration;

use sea_orm::prelude::Uuid;
use wf_core::TokenCipher;
use wf_db::tables::{events, github_pat_connections as gh, users};
use wf_sync::TickOptions;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn opts(owner: &str, base: &MockServer) -> TickOptions {
    TickOptions {
        batch: 50,
        budget: Duration::from_secs(20),
        lease_secs: 60,
        poll_interval_secs: 0, // immediately due again — lets one test tick repeatedly
        owner: owner.to_string(),
        github_base: Some(base.uri()),
    }
}

fn runs_fixture(updated_at: &str, id: i64) -> serde_json::Value {
    serde_json::json!({ "workflow_runs": [{
        "id": id, "run_attempt": 1, "name": "CI", "display_title": "Run",
        "status": "completed", "conclusion": "success",
        "html_url": "https://x/runs", "head_branch": "main",
        "created_at": updated_at, "updated_at": updated_at,
        "actor": { "login": "octocat" }
    }]})
}

async fn mount(server: &MockServer, runs: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/repos/o/r/actions/runs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(runs))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/o/r/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(server)
        .await;
}

/// Seeds a throwaway user + GitHub connection (selected repo `o/r`) whose PAT
/// is sealed with `cipher`. Returns the user id. ADJUST field names to the
/// real `UpsertPatInput` / `users` API — do not invent fields.
async fn seed_user(db: &wf_db::Db, cipher: &TokenCipher, tag: &str) -> Uuid {
    let user_id = Uuid::new_v4();
    seed_user_row(db, user_id, tag).await; // helper: insert users row (id, email)
    let sealed = cipher.seal("gh-token").unwrap();
    seed_gh_connection(db, user_id, sealed, vec!["o/r".to_string()]).await;
    user_id
}

#[tokio::test]
async fn tick_baseline_then_events_then_dedup() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let db = wf_db::connect(&url, wf_db::ConnectOptions::default()).await.unwrap();
    let cipher = TokenCipher::new(&[7u8; 32]);
    let server = MockServer::start().await;
    mount(&server, runs_fixture("2026-06-01T10:00:00Z", 1)).await;
    let user = seed_user(&db, &cipher, "t1").await;

    // Tick 1: baseline — scopes created, no events.
    let s1 = wf_sync::run_tick(&db, &cipher, &opts("t1a", &server)).await.unwrap();
    assert_eq!(s1.events_written, 0, "baseline emits nothing");

    // Tick 2: a newer run appears → exactly one event.
    server.reset().await;
    mount(&server, runs_fixture("2026-06-01T11:00:00Z", 2)).await;
    let s2 = wf_sync::run_tick(&db, &cipher, &opts("t1b", &server)).await.unwrap();
    assert_eq!(s2.events_written, 1);

    // Tick 3: same payload re-polled → cursor + dedup yield nothing.
    let s3 = wf_sync::run_tick(&db, &cipher, &opts("t1c", &server)).await.unwrap();
    assert_eq!(s3.events_written, 0);

    cleanup_user(&db, user).await; // DELETE FROM users → CASCADE
}

#[tokio::test]
async fn two_users_same_repo_both_get_events() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let db = wf_db::connect(&url, wf_db::ConnectOptions::default()).await.unwrap();
    let cipher = TokenCipher::new(&[7u8; 32]);
    let server = MockServer::start().await;
    mount(&server, runs_fixture("2026-06-01T10:00:00Z", 1)).await;
    let (u1, u2) =
        (seed_user(&db, &cipher, "t2a").await, seed_user(&db, &cipher, "t2b").await);

    wf_sync::run_tick(&db, &cipher, &opts("t2-base", &server)).await.unwrap(); // baselines
    server.reset().await;
    mount(&server, runs_fixture("2026-06-01T11:00:00Z", 2)).await;
    let s = wf_sync::run_tick(&db, &cipher, &opts("t2-ev", &server)).await.unwrap();
    assert_eq!(s.events_written, 2, "per-user dedup: both users get their copy");

    cleanup_user(&db, u1).await;
    cleanup_user(&db, u2).await;
}

#[tokio::test]
async fn concurrent_ticks_process_each_scope_once() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let db = wf_db::connect(&url, wf_db::ConnectOptions::default()).await.unwrap();
    let cipher = TokenCipher::new(&[7u8; 32]);
    let server = MockServer::start().await;
    mount(&server, runs_fixture("2026-06-01T10:00:00Z", 1)).await;
    let user = seed_user(&db, &cipher, "t3").await;
    wf_sync::run_tick(&db, &cipher, &opts("t3-base", &server)).await.unwrap();

    server.reset().await;
    mount(&server, runs_fixture("2026-06-01T11:00:00Z", 2)).await;
    let (a, b) = tokio::join!(
        wf_sync::run_tick(&db, &cipher, &opts("t3-a", &server)),
        wf_sync::run_tick(&db, &cipher, &opts("t3-b", &server)),
    );
    let total = a.unwrap().events_written + b.unwrap().events_written;
    assert_eq!(total, 1, "leases: the due scope is processed by exactly one tick");

    cleanup_user(&db, user).await;
}
```

Plus three small helpers at the bottom of the file (before the tests if free fns — note `#[cfg(test)] mod` rule doesn't apply to `tests/` files): `seed_user_row` (raw INSERT into users via `Statement`), `seed_gh_connection` (call `gh::upsert_pat` with the real `UpsertPatInput`), `cleanup_user` (`DELETE FROM users WHERE id = $1` raw). Write them against the actual signatures.

- [ ] **Step 2: Run gated** — `DATABASE_URL=<session-pooler-url> cargo test -p wf-sync --test tick_db -- --test-threads=1` → 3 passed (serial: they share `sync_state` due-rows; `--test-threads=1` keeps the concurrency test's accounting clean). And plain `cargo test -p wf-sync` (no env) → tests print "skipping".
- [ ] **Step 3:** `cargo clippy -p wf-sync --all-targets --locked -- -D warnings`.
- [ ] **Step 4: Commit** `git add crates/sync && git commit -m "A1: env-gated tick integration tests (baseline/dedup, two users, leases)"`

---

### Task 15: Docs, spec sync, final gates

**Files:** Modify: `CLAUDE.md`, `DEPLOYMENT.md`, `docs/superpowers/specs/2026-06-08-sub-project-a1-poll-event-backbone-design.md`

- [ ] **Step 1: Spec appendix** — append an `## Implementation deltas (A1 as built)` section to the A1 spec recording: (a) new `wf-sync` lib crate (tick shared with future worker; smoke is `cargo run -p wf-sync --example tick_smoke`, not `-p wf-db`); (b) all pollers use page-1-DESC + client-side compound-cursor filtering (Jira ASC dropped — JQL absolute timestamps are account-timezone-interpreted); (c) baseline poll emits nothing, only sets the watermark; (d) documented >50-updates-per-tick limitation.
- [ ] **Step 2: CLAUDE.md** — update the workspace-crates line to add `migration` + `wf-sync`; add 3-4 lines under Architecture: events backbone (events/sync_state tables, `POST /internal/tick` + `X-Internal-Token`, `cargo run -p migration -- up` for schema changes, tick logic lives in `wf-sync`).
- [ ] **Step 3: DEPLOYMENT.md** — add a "Tick scheduling" section: `INTERNAL_TICK_TOKEN` comes from Secret Manager; Cloud Scheduler job:

```bash
gcloud scheduler jobs create http wf-tick \
  --schedule="*/2 * * * *" \
  --uri="https://<service-url>/internal/tick" \
  --http-method=POST \
  --headers="X-Internal-Token=<token>" \
  --attempt-deadline=60s
```

- [ ] **Step 4: Full gates** — `cargo clippy --all --all-targets --locked -- -D warnings` and `cargo test --workspace` → both clean (integration tests self-skip without DATABASE_URL).
- [ ] **Step 5: Commit** `git add CLAUDE.md DEPLOYMENT.md docs && git commit -m "A1: docs — deployment scheduling, spec as-built appendix"`

---

## Plan self-review (done at authoring time)

- **Spec coverage:** §3.1→T2/3 · §3.2→T2/4 · §4.1→T11 · §4.2→T4/T10 · §4.3→T4/T12 · §5→T6/7/9 · §5.1 prev-status→T3 (`latest_jira_status`) + T10 (`collect_issues`) · §6.1→T9 · §6.2→T8/T10 (with the documented DESC deviation) · §7.1→T2 · §7.2→T1 · §8→T10 (isolation/backoff; invalid-PAT scopes removed in `github_scopes`/`jira_scopes`) · §9→T6/7/8/9/10 (unit), T14 (integration), T13 (smoke), T15 (gates).
- **Known intentional gaps vs spec:** revoked-PAT *detection* during a poll doesn't update `validation_status` (poll errors back off; the existing validate flow owns that field) — acceptable §8 reading, noted in the spec appendix if contested.
- **Type consistency spot-checks:** `InsertEventInput` fields (T3) = `normalize.rs` usage (T9) = `collect_*` (T10); `ScopeKey` (T4) = `github_scopes`/`jira_scopes` (T10); `TickOptions`/`TickSummary` (T10) = route (T11) = smoke (T13) = tests (T14); `PolledWorkflowRun.updated_at: DateTime<Utc>` matches `filter_new` key fn.
- **Verify-don't-guess markers:** jira connection token field names (T10 S3), `UpsertPatInput` shape (T14), `quote_jql_string` output (T7), `execute_raw` naming (T4), `Sealed` re-export (T10 S2) — each flagged at point of use.


