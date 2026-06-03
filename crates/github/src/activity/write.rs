//! Shared write-path error handling (port of `activity/activity.ts`'s
//! `writeEffect`/`writeDetail`). Non-2xx GitHub responses become
//! `GithubError::Write { status, message }` with the status passed through so
//! the route layer can surface 403/404/422 rather than a generic 502.

use reqwest::{RequestBuilder, Response};

use crate::errors::GithubError;

const PERMISSION_MESSAGE: &str = "Your GitHub token lacks write permission for this action. Reconnect a token with repo scope (or fine-grained actions:write / pull_requests:write).";

/// Resolve the human-facing detail for a failed write (port of `writeDetail`):
/// a fixed hint on 403, else GitHub's `response.data.message`, else `fallback`.
async fn write_detail(status: u16, resp: Response, fallback: &str) -> String {
    if status == 403 {
        return PERMISSION_MESSAGE.to_string();
    }
    let body: Option<serde_json::Value> = resp.json().await.ok();
    body.as_ref()
        .and_then(|v| v.get("message"))
        .and_then(|m| m.as_str())
        .map(String::from)
        .unwrap_or_else(|| fallback.to_string())
}

/// Send a write request, mapping transport errors and non-2xx responses to
/// `GithubError::Write` (transport → 502, mirroring `statusOf`'s default).
pub async fn write_send(req: RequestBuilder, fallback: &str) -> Result<Response, GithubError> {
    let resp = match req.send().await {
        Ok(r) => r,
        Err(_) => return Err(GithubError::Write { status: 502, message: fallback.to_string() }),
    };
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status().as_u16();
    let message = write_detail(status, resp, fallback).await;
    Err(GithubError::Write { status, message })
}
