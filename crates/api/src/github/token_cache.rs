//! In-memory, 60s-TTL cache of the decrypted PAT keyed by user (port of
//! `pat/token-cache.ts`). Saves a DB select + AES-open on every GitHub request
//! during a burst. Cleared on any connect/disconnect/repo-selection write so it
//! never serves a stale token.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sea_orm::prelude::Uuid;

const TTL: Duration = Duration::from_secs(60);

// Fields are consumed by the data/activity paths (next chunk).
#[allow(dead_code)]
#[derive(Clone)]
pub struct CachedPat {
    pub token: String,
    pub login: String,
    pub selected_repos: Vec<String>,
}

struct Entry {
    value: CachedPat,
    expires_at: Instant,
}

#[derive(Default)]
pub struct TokenCache {
    map: Mutex<HashMap<Uuid, Entry>>,
}

impl TokenCache {
    pub fn get(&self, user_id: Uuid) -> Option<CachedPat> {
        let mut map = self.map.lock().ok()?;
        match map.get(&user_id) {
            Some(e) if e.expires_at > Instant::now() => Some(e.value.clone()),
            Some(_) => {
                map.remove(&user_id);
                None
            }
            None => None,
        }
    }

    pub fn set(&self, user_id: Uuid, value: CachedPat) {
        if let Ok(mut map) = self.map.lock() {
            map.insert(
                user_id,
                Entry {
                    value,
                    expires_at: Instant::now() + TTL,
                },
            );
        }
    }

    pub fn clear(&self, user_id: Uuid) {
        if let Ok(mut map) = self.map.lock() {
            map.remove(&user_id);
        }
    }
}
