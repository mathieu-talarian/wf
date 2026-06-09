//! `AppState` — the dependency container, replacing Effect's `ManagedRuntime`
//! (migration plan §3.2). Stored in actix `Data<AppState>` and pulled into
//! handlers by extractor. Grows the dashboard cache + HTTP client factories as
//! later chunks land.

use std::sync::Arc;

use wf_core::{Config, TokenCipher};
use wf_db::Db;

use crate::auth::JwksVerifier;
use crate::github::dashboard_cache::DashboardCache;
use crate::github::token_cache::TokenCache;

// `config` is read from Phase 3+ handlers (e.g. web app URL); kept on state.
#[allow(dead_code)]
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Db,
    pub jwks: Arc<JwksVerifier>,
    pub cipher: Arc<TokenCipher>,
    pub token_cache: Arc<TokenCache>,
    pub dashboard_cache: Arc<DashboardCache>,
}
