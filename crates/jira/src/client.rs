//! Low-level Jira Cloud REST client (port of `jira/client.ts`): a Basic-auth
//! reqwest wrapper bound to one validated site origin. `redirect: none` ensures
//! credentials are never replayed to a redirect target (defense-in-depth
//! alongside site-URL normalization). Every non-2xx becomes a `JiraApiError`.

use base64::Engine;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Method, Response};
use serde::de::DeserializeOwned;

use crate::errors::JiraApiError;

#[derive(Debug, Clone)]
pub struct JiraCreds {
    pub site_url: String,
    pub email: String,
    pub token: String,
}

pub struct JiraClient {
    http: reqwest::Client,
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
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client builds");
        Self { http, site_url: creds.site_url.clone(), auth_header: auth_header(creds) }
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

        let resp = req.send().await.map_err(|_| JiraApiError::transport())?;
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
        serde_json::from_str::<T>(json_text)
            .map_err(|_| JiraApiError::new(status, "Invalid Jira response", vec![]))
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

    pub async fn del(&self, path: &str) -> Result<(), JiraApiError> {
        self.request::<serde_json::Value>(Method::DELETE, path, &[], None).await?;
        Ok(())
    }
}
