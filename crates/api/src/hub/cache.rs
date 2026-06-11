//! Tiny per-user TTL caches for the hub payloads. Board/inbox/runs assembly
//! fans out to GitHub/Jira; the frontend polls every 20–60s, so a short TTL
//! keeps us well under upstream rate limits without touching `AppState`.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

use sea_orm::prelude::Uuid;

use crate::hub::types::{HubBoard, HubRuns};

struct Entry<T> {
    at: Instant,
    value: T,
}

type Store<T> = RwLock<HashMap<Uuid, Entry<T>>>;

fn board_store() -> &'static Store<HubBoard> {
    static S: OnceLock<Store<HubBoard>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn runs_store() -> &'static Store<HubRuns> {
    static S: OnceLock<Store<HubRuns>> = OnceLock::new();
    S.get_or_init(Default::default)
}

const BOARD_TTL: Duration = Duration::from_secs(30);
const RUNS_TTL: Duration = Duration::from_secs(15);

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
        map.insert(user, Entry { at: Instant::now(), value: value.clone() });
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
    if let Ok(mut map) = board_store().write() {
        map.remove(&user);
    }
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

pub fn put_runs(user: Uuid, runs: &HubRuns) {
    put(runs_store(), user, runs);
}
