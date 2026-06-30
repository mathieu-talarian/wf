//! Low-level Jira Cloud REST client (port of `jira/client.ts`): a Basic-auth
//! reqwest wrapper bound to one validated site origin. `redirect: none` ensures
//! credentials are never replayed to a redirect target (defense-in-depth
//! alongside site-URL normalization). Every non-2xx becomes a `JiraApiError`.

use std::time::Duration;

use base64::Engine;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Method, Response};
use reqwest_middleware::RequestBuilder;
use serde::de::DeserializeOwned;

use crate::errors::JiraApiError;

/// Transient statuses worth one retry (rate-limit / upstream blip).
fn is_transient(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

/// Retry delay: honor `Retry-After` seconds (capped at 15s), else a short pause.
fn retry_delay(resp: Option<&Response>) -> Duration {
    resp.and_then(|r| r.headers().get(reqwest::header::RETRY_AFTER))
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|s| Duration::from_secs(s).min(Duration::from_secs(15)))
        .unwrap_or(Duration::from_millis(500))
}

/// One retry for idempotent GETs on transport errors or transient statuses;
/// honors `Retry-After`. ponytail: 1 retry, 15s cap — then the scope backoff wins.
async fn send_once_retry(req: RequestBuilder, retryable: bool) -> reqwest_middleware::Result<Response> {
    let retry = if retryable { req.try_clone() } else { None };
    let resp = req.send().await;
    let needs_retry = resp.as_ref().map(|r| is_transient(r.status().as_u16())).unwrap_or(true);
    match retry {
        Some(c) if needs_retry => {
            tokio::time::sleep(retry_delay(resp.as_ref().ok())).await;
            c.send().await
        }
        _ => resp,
    }
}

#[derive(Debug, Clone)]
pub struct JiraCreds {
    pub site_url: String,
    pub email: String,
    pub token: String,
}

pub struct JiraClient {
    http: wf_http::HttpClient,
    site_url: String,
    auth_header: String,
}

fn auth_header(creds: &JiraCreds) -> String {
    let raw = format!("{}:{}", creds.email, creds.token);
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
}

async fn to_api_error(status: u16, resp: Response) -> JiraApiError {
    let text = resp.text().await.unwrap_or_default();
    let body: Option<serde_json::Value> =
        if text.is_empty() { None } else { serde_json::from_str(&text).ok() };
    let messages: Vec<String> = body
        .as_ref()
        .and_then(|v| v.get("errorMessages"))
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|m| m.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let message =
        messages.first().cloned().unwrap_or_else(|| format!("Jira responded {status}"));
    JiraApiError::new(status, message, messages)
}

impl JiraClient {
    pub fn new(creds: &JiraCreds) -> Self {
        Self { http: wf_http::shared_no_redirect(), site_url: creds.site_url.clone(), auth_header: auth_header(creds) }
    }

    pub fn site_url(&self) -> &str {
        &self.site_url
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<T, JiraApiError> {
        let retryable = method == Method::GET;
        let mut req = self
            .http
            .request(method, format!("{}{}", self.site_url, path))
            .header(AUTHORIZATION, &self.auth_header)
            .header(ACCEPT, "application/json");
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(b) = body {
            req = req.header(CONTENT_TYPE, "application/json").json(b);
        }

        let resp = send_once_retry(req, retryable).await.map_err(|_| JiraApiError::transport())?;
        let status = resp.status().as_u16();
        // `redirect: none` surfaces 3xx as a response — never replay creds.
        if (300..400).contains(&status) {
            return Err(JiraApiError::redirect());
        }
        if status >= 400 {
            return Err(to_api_error(status, resp).await);
        }
        let text = resp.text().await.map_err(|_| JiraApiError::transport())?;
        let json_text = if text.is_empty() { "null" } else { text.as_str() };
        serde_json::from_str::<T>(json_text).map_err(|_| JiraApiError::new(status, "Invalid Jira response", vec![]))
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, JiraApiError> {
        self.request(Method::GET, path, query, None).await
    }

    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, JiraApiError> {
        self.request(Method::POST, path, &[], Some(body)).await
    }

    pub async fn put<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, JiraApiError> {
        self.request(Method::PUT, path, &[], Some(body)).await
    }
}
