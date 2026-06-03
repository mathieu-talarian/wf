//! `AppState` — the dependency container, replacing Effect's `ManagedRuntime`
//! (migration plan §3.2). Stored in actix `Data<AppState>` and pulled into
//! handlers by extractor. Grows token/dashboard caches and HTTP client
//! factories as Phases 3–4 land.

use std::sync::Arc;

use wf_core::{Config, TokenCipher};
use wf_db::Db;

use crate::auth::JwksVerifier;

// `config` and `cipher` are read from Phase 3 (GitHub/Jira PAT handlers).
#[allow(dead_code)]
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Db,
    pub jwks: Arc<JwksVerifier>,
    pub cipher: Arc<TokenCipher>,
}
