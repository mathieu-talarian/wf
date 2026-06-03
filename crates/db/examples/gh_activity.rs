//! Live check of the Activity reads against api.github.com, using the real
//! stored PAT + its selected repos.
//!   cargo run -p wf-db --example gh_activity
//! (Do NOT `source ./.env` first — dotenvy loads it; sourcing mangles DATABASE_URL.)

use anyhow::{Context, Result};
use sea_orm::EntityTrait;
use wf_core::crypto::{Sealed, TokenCipher};
use wf_db::entities::github_pat_connections as gh;
use wf_db::{connect, ConnectOptions};
use wf_github::{
    fetch_branch_prompts, fetch_workflow_inputs, fetch_workflows, list_environments,
    list_repo_branch_names, list_workflow_runs, parse_repo_ref,
};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let db = connect(&std::env::var("DATABASE_URL")?, ConnectOptions::default()).await?;
    let row = gh::Entity::find().one(&db).await?.context("no PAT row")?;

    let enc = std::env::var("GITHUB_TOKEN_ENCRYPTION_KEY")?;
    let token = {
        use base64::Engine;
        let raw = base64::engine::general_purpose::STANDARD.decode(enc)?;
        let arr: [u8; 32] = raw.try_into().map_err(|_| anyhow::anyhow!("key not 32 bytes"))?;
        TokenCipher::new(&arr).open(&Sealed {
            ciphertext: row.access_token_ciphertext.clone(),
            iv: row.access_token_iv.clone(),
            auth_tag: row.access_token_auth_tag.clone(),
        })?
    };

    let login = row.github_login.clone();
    let repos: Vec<String> = row
        .selected_repos
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    println!("login={login} repos={}", repos.len());

    println!("\n== branch prompts ==");
    for rb in fetch_branch_prompts(&token, &login, &repos).await {
        println!("  {} default={} branches={} err={:?}", rb.repo_full_name, rb.default_branch, rb.branches.len(), rb.error);
        for b in rb.branches.iter().take(3) {
            println!("     - {}  ({})", b.name, b.last_commit_date);
        }
    }

    println!("\n== workflows ==");
    let workflows = fetch_workflows(&token, &repos).await;
    let mut first: Option<(String, String, i64)> = None; // (fullName, path, id)
    for rw in &workflows {
        println!("  {} workflows={} err={:?}", rw.repo_full_name, rw.workflows.len(), rw.error);
        for w in rw.workflows.iter().take(3) {
            println!("     - #{} {} [{}]", w.id, w.name, w.path);
            // Only real workflow files have inputs; skip synthetic "dynamic/*"
            // entries (Dependabot/Copilot) whose path is not a repo file.
            if first.is_none() && w.path.starts_with(".github/") {
                first = Some((rw.repo_full_name.clone(), w.path.clone(), w.id));
            }
        }
    }

    if let Some((full_name, path, id)) = first {
        let r = parse_repo_ref(&full_name).context("bad repo full name")?;
        println!("\n== workflow inputs ({full_name} :: {path}) ==");
        let inputs = fetch_workflow_inputs(&token, &r, &path).await?;
        println!("  dispatchable={} inputs={}", inputs.dispatchable, inputs.inputs.len());
        for i in &inputs.inputs {
            println!("     - {} type={:?} required={} default={:?} options={:?}", i.name, i.r#type, i.required, i.default, i.options);
        }

        println!("\n== repo branches ({full_name}) ==");
        let names = list_repo_branch_names(&token, &r).await?;
        println!("  {} branches: {:?}", names.len(), names.iter().take(8).collect::<Vec<_>>());

        println!("\n== environments ({full_name}) ==");
        let envs = list_environments(&token, &r).await?;
        println!("  {:?}", envs);

        let default_branch = workflows
            .iter()
            .find(|w| w.repo_full_name == full_name)
            .map(|w| w.default_branch.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "main".to_string());
        println!("\n== workflow runs (#{id} on {default_branch}) ==");
        let runs = list_workflow_runs(&token, &r, id, &default_branch).await?;
        println!("  {} dispatch run(s)", runs.len());
        for run in runs.iter().take(5) {
            println!("     - #{} {} {:?} ({})", run.run_number, run.status, run.conclusion, run.created_at);
        }
    } else {
        println!("\n(no workflows found in selected repos; skipping inputs/runs)");
    }

    Ok(())
}
