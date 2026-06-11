//! Minimal Anthropic Messages API client (reqwest, no SDK). One short
//! completion call; errors surface as 502 via `AppError`.

use serde_json::json;

use crate::error::AppError;
use crate::state::AppState;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const VERSION: &str = "2023-06-01";
/// Drafts and briefs are short, latency-sensitive completions — haiku-class.
const MODEL: &str = "claude-haiku-4-5-20251001";
const MAX_TOKENS: u32 = 600;

/// Returns the API key or the contract's 503 (`ai-unconfigured`).
pub fn require_key(state: &AppState) -> Result<String, AppError> {
    state.config.anthropic_api_key.clone().ok_or_else(AppError::ai_unconfigured)
}

/// One system+user completion; returns the first text block.
pub async fn complete(
    state: &AppState,
    system: &str,
    user_prompt: &str,
) -> Result<String, AppError> {
    let key = require_key(state)?;
    let body = json!({
        "model": MODEL,
        "max_tokens": MAX_TOKENS,
        "system": system,
        "messages": [{ "role": "user", "content": user_prompt }],
    });
    let resp = reqwest::Client::new()
        .post(API_URL)
        .header("x-api-key", key)
        .header("anthropic-version", VERSION)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::ai_api(format!("Anthropic request failed: {e}")))?;
    let status = resp.status().as_u16();
    let payload: serde_json::Value = resp
        .json()
        .await
        .map_err(|_| AppError::ai_api(format!("Anthropic responded {status} (unreadable body)")))?;
    if status >= 400 {
        let detail = payload
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(AppError::ai_api(format!("Anthropic responded {status}: {detail}")));
    }
    payload
        .pointer("/content/0/text")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| AppError::ai_api("Anthropic response had no text block".to_string()))
}
