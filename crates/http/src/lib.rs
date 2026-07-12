//! One process-wide outbound HTTP transport for every provider client.
//!
//! `reqwest::Client` owns an `Arc`-backed connection pool, so a single shared
//! instance reuses keep-alive sockets + TLS sessions across all call sites
//! (auth is per-request, so the transport is token-independent). `reqwest-tracing`'s
//! `TracingMiddleware` emits one OTEL **client** span per request — exported
//! through the app's `tracing` → OTLP pipeline — and injects W3C `traceparent`.
//!
//! Providers store the returned [`HttpClient`] and build requests from it as
//! usual (`.request(..)`, `.get(..)`, `.post(..)`); cloning it is an `Arc` bump.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::TracingMiddleware;

/// Overall + connect ceilings so a hung upstream can't stall a tick.
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The outbound client type used by every provider: a `reqwest::Client` wrapped
/// with the tracing middleware. Clone freely — it shares the underlying pool.
pub type HttpClient = ClientWithMiddleware;

fn build(follow_redirects: bool) -> ClientWithMiddleware {
    let mut builder = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .pool_max_idle_per_host(16)
        .pool_idle_timeout(Duration::from_secs(120));
    if !follow_redirects {
        builder = builder.redirect(reqwest::redirect::Policy::none());
    }
    let inner = builder.build().expect("reqwest client builds");
    ClientBuilder::new(inner).with(TracingMiddleware::default()).build()
}

/// Shared default client (follows redirects) — GitHub, Slack.
pub fn shared() -> HttpClient {
    static C: OnceLock<HttpClient> = OnceLock::new();
    C.get_or_init(|| build(true)).clone()
}

/// Shared client that never follows redirects, so Basic credentials are never
/// replayed to a redirect target (Jira: defense-in-depth alongside site-URL
/// normalization).
pub fn shared_no_redirect() -> HttpClient {
    static C: OnceLock<HttpClient> = OnceLock::new();
    C.get_or_init(|| build(false)).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke: both builders construct (the `.expect()` TLS-backend path) and the
    /// `OnceLock` clone path works — catches a feature/TLS misconfig at test time.
    #[test]
    fn shared_clients_build() {
        let _ = shared();
        let _ = shared_no_redirect();
        // Second call returns the cached instance (clone), not a fresh build.
        let _ = shared();
    }
}
