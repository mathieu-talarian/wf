//! Per-request logging middleware.
//!
//! Replaces the Elysia `onRequest` / `onAfterResponse` hooks (migration plan
//! §12, §15): emits a `http.request` event before the handler and a
//! `http.response` event after, with method/url(/status). Implemented as
//! middleware rather than a fragile global hook.

use actix_web::body::MessageBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::middleware::Next;
use actix_web::Error;

pub async fn request_log(
    req: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    let method = req.method().to_string();
    let url = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());

    tracing::info!(target: "http.request", method = %method, url = %url);

    let res = next.call(req).await?;

    let status = res.status().as_u16();
    tracing::info!(target: "http.response", method = %method, url = %url, status);

    Ok(res)
}
