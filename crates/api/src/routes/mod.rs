use actix_web::web;

pub mod health;
pub mod me;

/// Registers all routes under the `/api` scope (migration plan §14). Later
/// phases add the `/me/**` GitHub + Jira route modules here.
pub fn configure(cfg: &mut web::ServiceConfig) {
    health::configure(cfg);
    me::configure(cfg);
    crate::github::routes::configure(cfg);
}
