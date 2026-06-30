//! Tiny per-user TTL caches for the hub payloads. Board/inbox/runs assembly
//! fans out to GitHub/Jira; the frontend polls every 20–60s, so a short TTL
//! keeps us well under upstream rate limits without touching `AppState`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use sea_orm::prelude::Uuid;
use tokio::sync::Mutex;
use wf_github::GithubQueueKey;
use wf_github::dashboard::types::GithubPullRequestQueue;

use crate::hub::types::{HubBoard, HubInbox, HubRuns};

struct Entry<T> {
    at: Instant,
    value: T,
}

type Store<T> = RwLock<HashMap<Uuid, Entry<T>>>;
type KeyedStore<K, T> = RwLock<HashMap<K, Entry<T>>>;
type RefreshLocks = RwLock<HashMap<(Uuid, &'static str), Arc<Mutex<()>>>>;

/// SWR freshness band for a cached value: serve `Fresh` directly, serve `Stale`
/// while a background refresh runs, recompute on `Missing`.
pub enum Freshness<T> {
    Fresh(T),
    Stale(T),
    Missing,
}

enum Band {
    Fresh,
    Stale,
    Missing,
}

fn band(elapsed: Duration, ttl: Duration, max_stale: Duration) -> Band {
    if elapsed < ttl {
        Band::Fresh
    } else if elapsed < max_stale {
        Band::Stale
    } else {
        Band::Missing
    }
}

fn board_store() -> &'static Store<HubBoard> {
    static S: OnceLock<Store<HubBoard>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn runs_store() -> &'static Store<HubRuns> {
    static S: OnceLock<Store<HubRuns>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn inbox_store() -> &'static Store<HubInbox> {
    static S: OnceLock<Store<HubInbox>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn review_store() -> &'static KeyedStore<(Uuid, GithubQueueKey), GithubPullRequestQueue> {
    static S: OnceLock<KeyedStore<(Uuid, GithubQueueKey), GithubPullRequestQueue>> =
        OnceLock::new();
    S.get_or_init(Default::default)
}

fn refresh_locks() -> &'static RefreshLocks {
    static S: OnceLock<RefreshLocks> = OnceLock::new();
    S.get_or_init(Default::default)
}

const BOARD_TTL: Duration = Duration::from_secs(30);
const RUNS_TTL: Duration = Duration::from_secs(60);
const INBOX_TTL: Duration = Duration::from_secs(15);
const REVIEW_TTL: Duration = Duration::from_secs(60);
/// Beyond this age a cached value is too old to serve stale — recompute inline.
/// Kept at 1h so an idle user's first poll back is still instant (served stale +
/// refreshed in the background); only a truly cold/long-idle hit blocks.
/// ponytail: 1h ceiling — lower it if stale data on return ever feels wrong.
const BOARD_MAX_STALE: Duration = Duration::from_secs(3600);
const RUNS_MAX_STALE: Duration = Duration::from_secs(3600);
const INBOX_MAX_STALE: Duration = Duration::from_secs(3600);

fn get<T: Clone>(store: &Store<T>, user: Uuid, ttl: Duration) -> Option<T> {
    let map = store.read().ok()?;
    let entry = map.get(&user)?;
    (entry.at.elapsed() < ttl).then(|| entry.value.clone())
}

fn put<T>(store: &Store<T>, user: Uuid, value: &T)
where
    T: Clone,
{
    if let Ok(mut map) = store.write() {
        map.insert(
            user,
            Entry {
                at: Instant::now(),
                value: value.clone(),
            },
        );
    }
}

fn get_swr<T: Clone>(
    store: &Store<T>,
    user: Uuid,
    ttl: Duration,
    max_stale: Duration,
) -> Freshness<T> {
    let Ok(map) = store.read() else { return Freshness::Missing };
    let Some(entry) = map.get(&user) else { return Freshness::Missing };
    match band(entry.at.elapsed(), ttl, max_stale) {
        Band::Fresh => Freshness::Fresh(entry.value.clone()),
        Band::Stale => Freshness::Stale(entry.value.clone()),
        Band::Missing => Freshness::Missing,
    }
}

fn get_keyed<K, T>(store: &KeyedStore<K, T>, key: &K, ttl: Duration) -> Option<T>
where
    K: Eq + std::hash::Hash,
    T: Clone,
{
    let map = store.read().ok()?;
    let entry = map.get(key)?;
    (entry.at.elapsed() < ttl).then(|| entry.value.clone())
}

fn put_keyed<K, T>(store: &KeyedStore<K, T>, key: K, value: &T)
where
    K: Eq + std::hash::Hash,
    T: Clone,
{
    if let Ok(mut map) = store.write() {
        map.insert(
            key,
            Entry {
                at: Instant::now(),
                value: value.clone(),
            },
        );
    }
}

fn remove<T>(store: &Store<T>, user: Uuid) {
    if let Ok(mut map) = store.write() {
        map.remove(&user);
    }
}

pub fn get_board(user: Uuid) -> Option<HubBoard> {
    get(board_store(), user, BOARD_TTL)
}

pub fn put_board(user: Uuid, board: &HubBoard) {
    put(board_store(), user, board);
}

/// Drops the cached board (after a manual link/unlink, so the next read
/// reflects it immediately).
pub fn invalidate_board(user: Uuid) {
    remove(board_store(), user);
}

fn brief_store() -> &'static Store<crate::hub::types::HubBrief> {
    static S: OnceLock<Store<crate::hub::types::HubBrief>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// The morning brief regenerates at most every 6h per user.
const BRIEF_TTL: Duration = Duration::from_secs(6 * 3600);

pub fn get_brief(user: Uuid) -> Option<crate::hub::types::HubBrief> {
    get(brief_store(), user, BRIEF_TTL)
}

pub fn put_brief(user: Uuid, brief: &crate::hub::types::HubBrief) {
    put(brief_store(), user, brief);
}

pub fn get_runs(user: Uuid) -> Option<HubRuns> {
    get(runs_store(), user, RUNS_TTL)
}

pub fn get_runs_swr(user: Uuid) -> Freshness<HubRuns> {
    get_swr(runs_store(), user, RUNS_TTL, RUNS_MAX_STALE)
}

pub fn get_board_swr(user: Uuid) -> Freshness<HubBoard> {
    get_swr(board_store(), user, BOARD_TTL, BOARD_MAX_STALE)
}

pub fn put_runs(user: Uuid, runs: &HubRuns) {
    put(runs_store(), user, runs);
}

pub fn invalidate_runs(user: Uuid) {
    remove(runs_store(), user);
}

/// Per-`(user, kind)` single-flight lock, shared by the SWR cold path (held via
/// `lock().await`) and background revalidation (acquired via `try_lock_owned`).
pub fn refresh_lock(kind: &'static str, user: Uuid) -> Arc<Mutex<()>> {
    let key = (user, kind);
    if let Some(lock) = refresh_locks().read().ok().and_then(|m| m.get(&key).cloned()) {
        return lock;
    }
    refresh_locks()
        .write()
        .expect("refresh lock map poisoned")
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub fn get_inbox(user: Uuid) -> Option<HubInbox> {
    get(inbox_store(), user, INBOX_TTL)
}

pub fn get_inbox_swr(user: Uuid) -> Freshness<HubInbox> {
    get_swr(inbox_store(), user, INBOX_TTL, INBOX_MAX_STALE)
}

pub fn put_inbox(user: Uuid, inbox: &HubInbox) {
    put(inbox_store(), user, inbox);
}

/// Drops the cached inbox only. The morning brief keeps its own 6h TTL — a
/// link/note/Slack mutation shouldn't force a fresh OpenAI generation.
pub fn invalidate_inbox(user: Uuid) {
    remove(inbox_store(), user);
}

pub fn get_review_queue(user: Uuid, key: GithubQueueKey) -> Option<GithubPullRequestQueue> {
    get_keyed(review_store(), &(user, key), REVIEW_TTL)
}

pub fn put_review_queue(user: Uuid, key: GithubQueueKey, queue: &GithubPullRequestQueue) {
    put_keyed(review_store(), (user, key), queue);
}

pub fn invalidate_review_queues(user: Uuid) {
    if let Ok(mut map) = review_store().write() {
        map.retain(|(u, _), _| *u != user);
    }
}

pub fn invalidate_user(user: Uuid) {
    invalidate_board(user);
    invalidate_runs(user);
    invalidate_inbox(user);
    invalidate_review_queues(user);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_picks_fresh_stale_missing_by_age() {
        let (ttl, max) = (Duration::from_secs(10), Duration::from_secs(100));
        assert!(matches!(band(Duration::from_secs(5), ttl, max), Band::Fresh));
        assert!(matches!(band(Duration::from_secs(50), ttl, max), Band::Stale));
        assert!(matches!(band(Duration::from_secs(200), ttl, max), Band::Missing));
        // boundaries: ttl is exclusive (>= ttl is stale), max_stale exclusive too.
        assert!(matches!(band(Duration::from_secs(10), ttl, max), Band::Stale));
        assert!(matches!(band(Duration::from_secs(100), ttl, max), Band::Missing));
    }
}
