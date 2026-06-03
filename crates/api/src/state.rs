//! `AppState` — the dependency container, replacing Effect's `ManagedRuntime`
//! (migration plan §3.2). Stored in actix `Data<AppState>` and pulled into
//! handlers by extractor. Grows db/clients/caches/jwks as later phases land.

use std::sync::Arc;

use wf_core::Config;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    // Phase 2+: db, jwks, cipher, token_cache, dashboard_cache, http clients.
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}
