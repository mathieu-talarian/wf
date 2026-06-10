//! A1 spec §9 integration: tick against wiremock GitHub + the real DB.
//! Env-gated: skips unless DATABASE_URL is set (live-harness pattern).
//! Run: DATABASE_URL=... cargo test -p wf-sync --test tick_db -- --test-threads=1

use std::time::Duration;

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;
use wf_core::{Sealed, TokenCipher};
use wf_db::tables::github_pat_connections as gh;
use wf_sync::TickOptions;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Fixture / option helpers
// ---------------------------------------------------------------------------

fn cipher() -> TokenCipher {
    TokenCipher::new(&[42u8; 32])
}

fn make_opts(owner: &str, uri: &str) -> TickOptions {
    TickOptions {
        batch: 50,
        budget: Duration::from_secs(20),
        lease_secs: 60,
        poll_interval_secs: 0, // immediately due again
        owner: owner.to_string(),
        github_base: Some(uri.to_string()),
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

/// Mounts GET /repos/o/r/actions/runs → `runs` and GET /repos/o/r/pulls → [].
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

// ---------------------------------------------------------------------------
// Seeding / cleanup helpers
// ---------------------------------------------------------------------------

async fn seed_user_row(db: &DatabaseConnection, user_id: Uuid, tag: &str) {
    let now = chrono::Utc::now();
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO users (id, email, created_at, updated_at) VALUES ($1, $2, $3, $4)",
        [
            user_id.into(),
            format!("tick-test-{tag}@example.invalid").into(),
            now.into(),
            now.into(),
        ],
    );
    db.execute_raw(stmt).await.expect("seed_user_row failed");
}

/// Upsert a GitHub connection via the real CRUD, then set selected_repos
/// (upsert_model leaves that column as NotSet).
async fn seed_gh_connection(
    db: &DatabaseConnection,
    user_id: Uuid,
    sealed: Sealed,
    selected_repos: Vec<String>,
) {
    let input = gh::UpsertPatInput {
        user_id,
        github_user_id: 9999,
        github_login: "tick-test-bot".to_string(),
        token_kind: "classic".to_string(),
        sealed,
        scopes: Some(vec!["repo".to_string()]),
        expires_at: None,
        last_four: "tttt".to_string(),
        validation_status: "valid".to_string(),
    };
    gh::upsert_pat(db, input).await.expect("seed_gh_connection: upsert_pat failed");
    gh::set_selected_repos(db, user_id, &selected_repos)
        .await
        .expect("seed_gh_connection: set_selected_repos failed");
}

/// DELETE FROM users WHERE id = $1 (CASCADE removes connections/sync_state/events).
async fn cleanup_user(db: &DatabaseConnection, user_id: Uuid) {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM users WHERE id = $1",
        [user_id.into()],
    );
    db.execute_raw(stmt).await.expect("cleanup_user failed");
}

/// COUNT(*) events for a test user — isolation-safe assertion helper.
async fn count_user_events(db: &DatabaseConnection, user_id: Uuid) -> u64 {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::bigint AS n FROM events WHERE user_id = $1",
        [user_id.into()],
    );
    let row = db.query_one_raw(stmt).await.expect("count_user_events failed");
    let n: i64 = row.expect("no count row").try_get("", "n").expect("n missing");
    n as u64
}

// ---------------------------------------------------------------------------
// Test 1 body — extracted to stay ≤25 lines per fn
// ---------------------------------------------------------------------------

async fn run_baseline_dedup(db: &DatabaseConnection, server: &MockServer) {
    let c = cipher();
    let user_id = Uuid::new_v4();
    seed_user_row(db, user_id, &user_id.to_string()).await;
    seed_gh_connection(db, user_id, c.seal("gh-token").unwrap(), vec!["o/r".to_string()]).await;
    assert_eq!(count_user_events(db, user_id).await, 0, "pre: no events");

    // Tick 1 — baseline: no cursor, emits nothing.
    mount(server, runs_fixture("2026-06-01T10:00:00Z", 100)).await;
    wf_sync::run_tick(db, &c, &make_opts("t1", &server.uri())).await.unwrap();
    assert_eq!(count_user_events(db, user_id).await, 0, "baseline: 0 events");

    // Tick 2 — new run after cursor: 1 event.
    server.reset().await;
    mount(server, runs_fixture("2026-06-01T11:00:00Z", 200)).await;
    wf_sync::run_tick(db, &c, &make_opts("t2", &server.uri())).await.unwrap();
    let n2 = count_user_events(db, user_id).await;
    assert_eq!(n2, 1, "tick2: 1 event (got {n2})");

    // Tick 3 — same fixture: dedup, no new event.
    server.reset().await;
    mount(server, runs_fixture("2026-06-01T11:00:00Z", 200)).await;
    wf_sync::run_tick(db, &c, &make_opts("t3", &server.uri())).await.unwrap();
    let n3 = count_user_events(db, user_id).await;
    assert_eq!(n3, 1, "tick3 dedup: still 1 (got {n3})");

    cleanup_user(db, user_id).await;
    assert_eq!(count_user_events(db, user_id).await, 0, "cleanup ok");
}

#[tokio::test]
async fn tick_baseline_then_events_then_dedup() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping tick_baseline_then_events_then_dedup: DATABASE_URL not set");
        return;
    };
    let db = wf_db::connect(&url, wf_db::ConnectOptions::default()).await.unwrap();
    let server = MockServer::start().await;
    run_baseline_dedup(&db, &server).await;
}

// ---------------------------------------------------------------------------
// Test 2 body
// ---------------------------------------------------------------------------

async fn run_two_users(db: &DatabaseConnection, server: &MockServer) {
    let c = cipher();
    let uid_a = Uuid::new_v4();
    let uid_b = Uuid::new_v4();

    seed_user_row(db, uid_a, &format!("a-{uid_a}")).await;
    seed_gh_connection(db, uid_a, c.seal("gh-token-a").unwrap(), vec!["o/r".to_string()]).await;

    seed_user_row(db, uid_b, &format!("b-{uid_b}")).await;
    seed_gh_connection(db, uid_b, c.seal("gh-token-b").unwrap(), vec!["o/r".to_string()]).await;

    // Baseline tick (establishes cursors, emits nothing).
    mount(server, runs_fixture("2026-06-01T10:00:00Z", 100)).await;
    wf_sync::run_tick(db, &c, &make_opts("two-base", &server.uri())).await.unwrap();

    // Second tick — each user gets 1 event.
    server.reset().await;
    mount(server, runs_fixture("2026-06-01T12:00:00Z", 300)).await;
    wf_sync::run_tick(db, &c, &make_opts("two-t2", &server.uri())).await.unwrap();

    let ea = count_user_events(db, uid_a).await;
    let eb = count_user_events(db, uid_b).await;
    eprintln!("two-users: a={ea} b={eb}");
    assert_eq!(ea, 1, "user_a: 1 event (got {ea})");
    assert_eq!(eb, 1, "user_b: 1 event (got {eb})");

    cleanup_user(db, uid_a).await;
    cleanup_user(db, uid_b).await;
}

#[tokio::test]
async fn tick_two_users_same_repo() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping tick_two_users_same_repo: DATABASE_URL not set");
        return;
    };
    let db = wf_db::connect(&url, wf_db::ConnectOptions::default()).await.unwrap();
    let server = MockServer::start().await;
    run_two_users(&db, &server).await;
}

// ---------------------------------------------------------------------------
// Test 3 body — split into baseline phase + concurrent phase
// ---------------------------------------------------------------------------

async fn lease_baseline_phase(db: &DatabaseConnection, server: &MockServer) -> Uuid {
    let c = cipher();
    let user_id = Uuid::new_v4();
    seed_user_row(db, user_id, &user_id.to_string()).await;
    seed_gh_connection(db, user_id, c.seal("gh-token").unwrap(), vec!["o/r".to_string()]).await;
    mount(server, runs_fixture("2026-06-01T10:00:00Z", 100)).await;
    wf_sync::run_tick(db, &c, &make_opts("lease-base", &server.uri())).await.unwrap();
    user_id
}

async fn lease_concurrent_phase(
    db: &DatabaseConnection,
    db2: wf_db::Db,
    user_id: Uuid,
    uri: String,
) {
    let c = cipher();
    let c2 = cipher();
    let opts_a = make_opts("lease-A", &uri);
    let opts_b = make_opts("lease-B", &uri);
    let handle_a = tokio::spawn(async move {
        wf_sync::run_tick(&db2, &c2, &opts_a).await.unwrap()
    });
    let sb = wf_sync::run_tick(db, &c, &opts_b).await.unwrap();
    let sa = handle_a.await.unwrap();
    eprintln!("lease: A claimed={} B claimed={}", sa.scopes_claimed, sb.scopes_claimed);
    let total = count_user_events(db, user_id).await;
    eprintln!("lease: total test-user events={total}");
    assert_eq!(total, 1, "lease: exactly 1 event (got {total})");
    cleanup_user(db, user_id).await;
}

#[tokio::test]
async fn tick_concurrent_lease_protection() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping tick_concurrent_lease_protection: DATABASE_URL not set");
        return;
    };
    let db = wf_db::connect(&url, wf_db::ConnectOptions::default()).await.unwrap();
    let db2 = wf_db::connect(&url, wf_db::ConnectOptions::default()).await.unwrap();
    let server = MockServer::start().await;
    let user_id = lease_baseline_phase(&db, &server).await;
    server.reset().await;
    mount(&server, runs_fixture("2026-06-01T13:00:00Z", 400)).await;
    let uri = server.uri();
    lease_concurrent_phase(&db, db2, user_id, uri).await;
}
