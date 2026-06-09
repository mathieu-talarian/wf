//! End-to-end Jira read smoke test against a real Jira Cloud site. Closes the
//! Jira live-verification gap once credentials exist.
//!   JIRA_SITE_URL=acme.atlassian.net JIRA_EMAIL=you@x.com JIRA_TOKEN=... \
//!     cargo run -p wf-jira --example jira_smoke
//! Optionally JIRA_ISSUE_KEY=PROJ-123 to also fetch one issue's detail.

use wf_jira::{
    fetch_dashboard_queues, fetch_issue_detail, list_boards, list_projects, validate_credentials,
    JiraClient, JiraConnectInput, JiraCreds, JiraValidated, QueueJqlCtx,
};

/// Read JIRA_SITE_URL / JIRA_EMAIL / JIRA_TOKEN from the environment, or exit(2).
fn load_creds() -> (String, String, String) {
    let (Ok(site_url), Ok(email), Ok(token)) = (
        std::env::var("JIRA_SITE_URL"),
        std::env::var("JIRA_EMAIL"),
        std::env::var("JIRA_TOKEN"),
    ) else {
        eprintln!("set JIRA_SITE_URL / JIRA_EMAIL / JIRA_TOKEN to run the smoke test");
        std::process::exit(2);
    };
    (site_url, email, token)
}

/// 1) validate → account id + canonical origin (exits 1 on failure).
async fn validate(site_url: String, email: &str, token: &str) -> JiraValidated {
    let input = JiraConnectInput { site_url, email: email.to_string(), token: token.to_string() };
    let validated = match validate_credentials(&input).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("validate failed: {:?} (http={:?}): {}", e.status, e.http_status, e.message);
            std::process::exit(1);
        }
    };
    println!("validated: account={} display={:?}", validated.account_id, validated.display_name);
    validated
}

/// 2) projects + boards.
async fn print_projects_and_boards(client: &JiraClient) {
    match list_projects(client).await {
        Ok(ps) => println!("\nprojects: {} (e.g. {:?})", ps.len(), ps.iter().take(5).map(|p| &p.key).collect::<Vec<_>>()),
        Err(e) => println!("\nprojects ERROR: {e}"),
    }
    match list_boards(client).await {
        Ok(bs) => println!("boards: {} (e.g. {:?})", bs.len(), bs.iter().take(5).map(|b| &b.name).collect::<Vec<_>>()),
        Err(e) => println!("boards ERROR: {e}"),
    }
}

/// 3) dashboard queues (per-queue degradation surfaces as `error`).
async fn print_dashboard_queues(client: &JiraClient, account_id: String) {
    let ctx = QueueJqlCtx { account_id, selected_projects: vec![] };
    println!("\ndashboard queues:");
    for q in fetch_dashboard_queues(client, &ctx).await {
        println!(
            "  {:<16?} total={:?} issues={} error={:?}",
            q.key,
            q.approximate_total,
            q.issues.len(),
            q.error
        );
    }
}

/// 4) one issue's detail, if JIRA_ISSUE_KEY is set.
async fn print_issue_detail(client: &JiraClient) {
    if let Ok(key) = std::env::var("JIRA_ISSUE_KEY") {
        println!("\nissue {key}:");
        match fetch_issue_detail(client, &key).await {
            Ok(d) => println!(
                "  {} [{}] status={} assignee={:?} comments={}",
                d.summary.key, d.summary.status.name, d.summary.summary, d.summary.assignee.map(|a| a.display_name), d.comments.len()
            ),
            Err(e) => println!("  ERROR: {e}"),
        }
    }
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let (site_url, email, token) = load_creds();

    let validated = validate(site_url, &email, &token).await;
    let client = JiraClient::new(&JiraCreds { site_url: validated.origin.clone(), email, token });

    print_projects_and_boards(&client).await;
    print_dashboard_queues(&client, validated.account_id.clone()).await;
    print_issue_detail(&client).await;

    println!("\njira smoke OK");
}
