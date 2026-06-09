//! Jira 4a check. Offline: demonstrates site-URL normalization (deterministic).
//! Live (optional): set JIRA_SITE_URL, JIRA_EMAIL, JIRA_TOKEN to validate real
//! credentials against /rest/api/3/myself.
//!   cargo run -p wf-jira --example jira_validate

use wf_jira::{normalize_site_url, validate_credentials, JiraConnectInput, SiteUrlResult};

/// Offline demo: deterministic site-URL normalization for a fixed sample set.
fn demo_normalization() {
    println!("== site-URL normalization ==");
    for input in [
        "acme.atlassian.net",
        "https://ACME.Atlassian.NET/",
        "http://acme.atlassian.net",
        "https://acme.atlassian.net/wiki",
        "https://acme.atlassian.net.evil.com",
        "https://evil.com",
    ] {
        match normalize_site_url(input) {
            SiteUrlResult::Ok { origin } => println!("  OK   {input:<38} -> {origin}"),
            SiteUrlResult::Err { reason } => println!("  ERR  {input:<38} -> {reason}"),
        }
    }
}

/// Live validation against /rest/api/3/myself when all three env vars are set.
async fn live_validate() {
    match (
        std::env::var("JIRA_SITE_URL"),
        std::env::var("JIRA_EMAIL"),
        std::env::var("JIRA_TOKEN"),
    ) {
        (Ok(site_url), Ok(email), Ok(token)) => {
            println!("\n== live validate ({site_url}) ==");
            let input = JiraConnectInput { site_url, email, token };
            match validate_credentials(&input).await {
                Ok(v) => println!(
                    "  valid: account={} display={:?} origin={}",
                    v.account_id, v.display_name, v.origin
                ),
                Err(e) => println!("  {:?} (http={:?}): {}", e.status, e.http_status, e.message),
            }
        }
        _ => println!("\n(set JIRA_SITE_URL/JIRA_EMAIL/JIRA_TOKEN to live-validate)"),
    }
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    demo_normalization();
    live_validate().await;
}
