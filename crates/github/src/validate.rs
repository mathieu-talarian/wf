//! PAT validation state machine (port of `pat/validate.ts`).
//!
//! Flow: fetch identity (`GET /user`), then a best-effort search probe and, if
//! a PR is found, an enrichment probe across the endpoints the dashboard uses.
//! Only 401/403 on the probes invalidate the token; 404/other are expected and
//! ignored. The web app keys connection UI off the resulting status.

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use reqwest::Method;
use serde::Deserialize;

use crate::client::{is_auth_status, parse_repo_ref, send_checked, GithubClient, RepoRef};
use crate::errors::PatValidationError;
use crate::types::{PatTokenKind, PatValidationResult};

#[derive(Deserialize)]
struct AuthenticatedUser {
    id: i64,
    login: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    items: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    number: i64,
    repository_url: String,
}

#[derive(Deserialize)]
struct PullDetail {
    head: PullRef,
    base: PullBaseRef,
}
#[derive(Deserialize)]
struct PullRef {
    sha: String,
}
#[derive(Deserialize)]
struct PullBaseRef {
    #[serde(rename = "ref")]
    ref_name: String,
}

struct ProbeItem {
    ref_: RepoRef,
    number: i64,
}

fn parse_scopes(raw: Option<&str>) -> Option<Vec<String>> {
    let raw = raw?;
    Some(
        raw.split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Lenient expiry parse (the TS uses `new Date(raw)`); unparseable → None.
fn parse_expiry(raw: Option<&str>) -> Option<DateTime<Utc>> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    // Numeric offset, e.g. "2030-01-02 03:04:05 -0800".
    if let Ok(dt) = DateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S %z") {
        return Some(dt.with_timezone(&Utc));
    }
    // GitHub's common form "2030-01-02 03:04:05 UTC" — parse naive, assume UTC.
    let naive_part = raw.strip_suffix(" UTC").unwrap_or(raw);
    if let Ok(ndt) = NaiveDateTime::parse_from_str(naive_part, "%Y-%m-%d %H:%M:%S") {
        return Some(Utc.from_utc_datetime(&ndt));
    }
    None
}

async fn fetch_identity(
    client: &GithubClient,
) -> Result<PatValidationResult, PatValidationError> {
    let resp = send_checked(client.request(Method::GET, "/user")).await?;
    let scopes = parse_scopes(header(&resp, "x-oauth-scopes"));
    let expires_at = parse_expiry(header(&resp, "github-authentication-token-expiration"));
    let user: AuthenticatedUser = resp
        .json()
        .await
        .map_err(|_| PatValidationError::unreachable())?;
    Ok(PatValidationResult {
        github_user_id: user.id,
        login: user.login,
        token_kind: PatTokenKind::detect(client.token()),
        scopes,
        expires_at,
    })
}

fn header<'a>(resp: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    resp.headers().get(name).and_then(|v| v.to_str().ok()).filter(|s| !s.is_empty())
}

async fn probe_search(
    client: &GithubClient,
    login: &str,
) -> Result<Option<ProbeItem>, PatValidationError> {
    let resp = send_checked(
        client
            .request(Method::GET, "/search/issues")
            .query(&[
                ("q", format!("is:pr involves:{login}").as_str()),
                ("per_page", "1"),
                ("advanced_search", "true"),
            ]),
    )
    .await?;
    let search: SearchResponse = resp.json().await.map_err(|_| PatValidationError::unreachable())?;
    Ok(search.items.into_iter().next().and_then(|item| {
        parse_repo_ref(&item.repository_url).map(|ref_| ProbeItem {
            ref_,
            number: item.number,
        })
    }))
}

/// Fires a probe call: only 401/403 invalidate (returns Err); transport errors,
/// 404, and other statuses are ignored (returns Ok) — port of `probeCall`.
async fn probe_call(client: &GithubClient, path: &str, query: &[(&str, &str)]) -> Result<(), PatValidationError> {
    let resp = match client.request(Method::GET, path).query(query).send().await {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    if is_auth_status(resp.status()) {
        let status = resp.status().as_u16();
        return Err(PatValidationError::of(
            crate::client::classify_status(status, crate::client::has_sso_header(&resp)),
            Some(status),
        ));
    }
    Ok(())
}

/// PR detail for enrichment; auth errors propagate, anything else → None
/// (port of `probeDetail`).
async fn probe_detail(
    client: &GithubClient,
    item: &ProbeItem,
) -> Result<Option<(String, String)>, PatValidationError> {
    let path = format!("/repos/{}/{}/pulls/{}", item.ref_.owner, item.ref_.repo, item.number);
    let resp = match client.request(Method::GET, &path).send().await {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    if is_auth_status(resp.status()) {
        let status = resp.status().as_u16();
        return Err(PatValidationError::of(
            crate::client::classify_status(status, crate::client::has_sso_header(&resp)),
            Some(status),
        ));
    }
    if !resp.status().is_success() {
        return Ok(None);
    }
    match resp.json::<PullDetail>().await {
        Ok(d) => Ok(Some((d.head.sha, d.base.ref_name))),
        Err(_) => Ok(None),
    }
}

async fn probe_enrichment(
    client: &GithubClient,
    item: &ProbeItem,
) -> Result<(), PatValidationError> {
    let Some((sha, base_ref)) = probe_detail(client, item).await? else {
        return Ok(());
    };
    let (o, r) = (&item.ref_.owner, &item.ref_.repo);
    probe_call(client, &format!("/repos/{o}/{r}/pulls/{}/reviews", item.number), &[("per_page", "1")]).await?;
    probe_call(client, &format!("/repos/{o}/{r}/actions/runs"), &[("head_sha", &sha), ("per_page", "1")]).await?;
    probe_call(client, &format!("/repos/{o}/{r}/commits/{sha}/status"), &[]).await?;
    probe_call(client, &format!("/repos/{o}/{r}/commits/{sha}/check-runs"), &[("per_page", "1")]).await?;
    probe_call(client, &format!("/repos/{o}/{r}/branches/{base_ref}/protection/required_status_checks"), &[]).await?;
    Ok(())
}

/// Validates a token against GitHub, returning identity + metadata.
pub async fn validate_token(token: &str) -> Result<PatValidationResult, PatValidationError> {
    let client = GithubClient::new(token);
    let identity = fetch_identity(&client).await?;
    if let Some(item) = probe_search(&client, &identity.login).await? {
        probe_enrichment(&client, &item).await?;
    }
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scopes_trimming_and_filtering() {
        assert_eq!(parse_scopes(None), None);
        assert_eq!(
            parse_scopes(Some("repo, workflow , ,read:org")),
            Some(vec!["repo".to_string(), "workflow".to_string(), "read:org".to_string()])
        );
    }

    #[test]
    fn parses_expiry_formats_and_rejects_garbage() {
        assert!(parse_expiry(None).is_none());
        assert!(parse_expiry(Some("not a date")).is_none());
        assert!(parse_expiry(Some("2030-01-02T03:04:05Z")).is_some());
        assert!(parse_expiry(Some("2030-01-02 03:04:05 UTC")).is_some());
    }
}
