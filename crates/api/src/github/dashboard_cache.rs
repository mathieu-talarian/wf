//! Stale-while-revalidate cache of the whole dashboard response, keyed by
//! (user, tab) (port of `pat/dashboard-cache.ts`). A fresh entry (< TTL) serves
//! directly; a stale one is served instantly while a single-flight background
//! refresh runs. Cleared explicitly on connect/disconnect/repo-change.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sea_orm::prelude::Uuid;
use wf_github::{GithubDashboard, GithubQueueKey};

const TTL: Duration = Duration::from_secs(30);

type Key = (Uuid, GithubQueueKey);

struct Entry {
    value: GithubDashboard,
    expires_at: Instant,
}

pub struct CacheHit {
    pub value: GithubDashboard,
    pub fresh: bool,
}

#[derive(Default)]
pub struct DashboardCache {
    entries: Mutex<HashMap<Key, Entry>>,
    refreshing: Mutex<HashSet<Key>>,
}

impl DashboardCache {
    /// Returns the entry even when stale (`fresh: false`); `None` only on a miss.
    pub fn peek(&self, user_id: Uuid, tab: GithubQueueKey) -> Option<CacheHit> {
        let entries = self.entries.lock().ok()?;
        let entry = entries.get(&(user_id, tab))?;
        Some(CacheHit {
            value: entry.value.clone(),
            fresh: entry.expires_at > Instant::now(),
        })
    }

    pub fn set(&self, user_id: Uuid, tab: GithubQueueKey, value: GithubDashboard) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert((user_id, tab), Entry { value, expires_at: Instant::now() + TTL });
        }
    }

    pub fn clear(&self, user_id: Uuid) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|(u, _), _| *u != user_id);
        }
    }

    /// Single-flight guard: only the first caller for a (user, tab) gets `true`
    /// and should run the revalidation. Pair with `end_refresh`.
    pub fn try_begin_refresh(&self, user_id: Uuid, tab: GithubQueueKey) -> bool {
        match self.refreshing.lock() {
            Ok(mut set) => set.insert((user_id, tab)),
            Err(_) => false,
        }
    }

    pub fn end_refresh(&self, user_id: Uuid, tab: GithubQueueKey) {
        if let Ok(mut set) = self.refreshing.lock() {
            set.remove(&(user_id, tab));
        }
    }
}
