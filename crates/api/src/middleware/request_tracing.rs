//! Custom actix-web tracing + metrics middleware (spec §12, §15).
//!
//! Replaces the old `request_log` event pair so we fully control:
//!   * **Trace-context propagation** — continue the upstream trace from Cloud
//!     Run, which sends both the W3C `traceparent` header and Google's legacy
//!     `X-Cloud-Trace-Context`. We try W3C first, then fall back to the Google
//!     header, so the app's spans share the same trace id Cloud Run logged.
//!   * **Span status + exceptions** — set OTel `Status::Ok`/`Status::Error` from
//!     the HTTP outcome, and record an `exception` span event on failures.
//!   * **HTTP server metrics** — `http.server.request.duration` (histogram) and
//!     `http.server.active_requests` (up/down counter), per OTel semconv, for
//!     *every* endpoint.
//!   * **HTTP semantic attributes** — using the current stable OTel conventions.

use std::future::{ready, Future, Ready};
use std::pin::Pin;
use std::rc::Rc;
use std::time::Instant;

use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::http::header::HeaderMap;
use actix_web::Error;
use opentelemetry::metrics::{Histogram, UpDownCounter};
use opentelemetry::propagation::Extractor;
use opentelemetry::trace::{
    SpanContext, SpanId, Status, TraceContextExt, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{Context, KeyValue};
use tracing::{Instrument, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::telemetry::INSTRUMENTATION_SCOPE;

/// OTel HTTP server metric instruments, shared across requests (cheap to clone).
#[derive(Clone)]
struct HttpMetrics {
    /// `http.server.request.duration` — request latency histogram (seconds).
    duration: Histogram<f64>,
    /// `http.server.active_requests` — in-flight request gauge.
    active: UpDownCounter<i64>,
}

impl HttpMetrics {
    fn new() -> Self {
        let meter = opentelemetry::global::meter(INSTRUMENTATION_SCOPE);
        Self {
            duration: meter
                .f64_histogram("http.server.request.duration")
                .with_unit("s")
                .with_description("Duration of inbound HTTP requests.")
                .build(),
            active: meter
                .i64_up_down_counter("http.server.active_requests")
                .with_description("Number of in-flight inbound HTTP requests.")
                .build(),
        }
    }
}

/// `RequestTracing` middleware factory. Add with `.wrap(RequestTracing::new())`.
pub struct RequestTracing {
    metrics: HttpMetrics,
}

impl RequestTracing {
    pub fn new() -> Self {
        Self {
            metrics: HttpMetrics::new(),
        }
    }
}

impl Default for RequestTracing {
    fn default() -> Self {
        Self::new()
    }
}

impl<S, B> Transform<S, ServiceRequest> for RequestTracing
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = RequestTracingMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(RequestTracingMiddleware {
            service: Rc::new(service),
            metrics: self.metrics.clone(),
        }))
    }
}

pub struct RequestTracingMiddleware<S> {
    service: Rc<S>,
    metrics: HttpMetrics,
}

impl<S, B> Service<ServiceRequest> for RequestTracingMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<ServiceResponse<B>, Error>>>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let span = build_request_span(&req);

        // Continue the upstream trace (Cloud Run traceparent / X-Cloud-Trace-Context).
        // set_parent must run before the span is entered, so do it here.
        if let Err(err) = span.set_parent(extract_remote_context(req.headers())) {
            tracing::debug!(?err, "could not attach remote trace context");
        }

        let method = req.method().as_str().to_owned();
        let fallback_route = req.path().to_owned();
        let outcome_span = span.clone();
        let metrics = self.metrics.clone();
        let service = self.service.clone();

        Box::pin(async move {
            let active_attrs = [KeyValue::new("http.request.method", method.clone())];
            metrics.active.add(1, &active_attrs);

            let start = Instant::now();
            let outcome = service.call(req).instrument(span).await;
            let elapsed = start.elapsed().as_secs_f64();

            metrics.active.add(-1, &active_attrs);

            let (route, status_code) = summarize(&outcome, &fallback_route);
            apply_span_outcome(&outcome_span, &outcome, &route, status_code);
            record_duration(&metrics, elapsed, method, route, status_code);

            outcome
        })
    }
}

/// Records the `http.server.request.duration` histogram with method/route/status
/// attributes (split out of `call` to keep the service future compact).
fn record_duration(
    metrics: &HttpMetrics,
    elapsed: f64,
    method: String,
    route: String,
    status_code: u16,
) {
    metrics.duration.record(
        elapsed,
        &[
            KeyValue::new("http.request.method", method),
            KeyValue::new("http.route", route),
            KeyValue::new("http.response.status_code", i64::from(status_code)),
        ],
    );
}

/// Build the server root span with stable OTel HTTP semantic-convention fields.
/// Dynamic values (route, status, exceptions) are attached later via the OTel
/// span extension API once the request ends.
fn build_request_span(req: &ServiceRequest) -> Span {
    let method = req.method().as_str().to_owned();
    let path = req.path().to_owned();
    let scheme = req.connection_info().scheme().to_owned();
    let user_agent = req
        .headers()
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();

    // Span name is "{method} {path}". The route template ("/api/hello/{name}")
    // isn't known until after routing — which this outer middleware runs before —
    // and tracing-opentelemetry fixes the span name at creation, so the template
    // goes on the `http.route` attribute instead (set in `apply_span_outcome`).
    tracing::info_span!(
        "http.request",
        otel.name = format!("{method} {path}"),
        otel.kind = "server",
        http.request.method = %method,
        url.path = %path,
        url.scheme = %scheme,
        user_agent.original = %user_agent,
        network.protocol.version = ?req.version(),
    )
}

/// Pull the route template and status code out of the request outcome.
fn summarize<B>(
    outcome: &Result<ServiceResponse<B>, Error>,
    fallback_route: &str,
) -> (String, u16) {
    match outcome {
        Ok(response) => {
            let route = response
                .request()
                .match_pattern()
                .unwrap_or_else(|| fallback_route.to_owned());
            (route, response.status().as_u16())
        }
        Err(err) => (
            fallback_route.to_owned(),
            err.as_response_error().status_code().as_u16(),
        ),
    }
}

/// Attach the route, response status, OTel `Status`, and exception event.
fn apply_span_outcome<B>(
    span: &Span,
    outcome: &Result<ServiceResponse<B>, Error>,
    route: &str,
    status_code: u16,
) {
    span.set_attribute("http.route", route.to_owned());
    span.set_attribute("http.response.status_code", i64::from(status_code));

    let is_server_error = status_code >= 500;
    if is_server_error {
        let message = match outcome {
            Err(err) => err.to_string(),
            Ok(_) => actix_web::http::StatusCode::from_u16(status_code)
                .ok()
                .and_then(|s| s.canonical_reason())
                .unwrap_or("server error")
                .to_owned(),
        };
        span.set_status(Status::error(message.clone()));
        span.add_event(
            "exception",
            vec![
                KeyValue::new("exception.type", "http.server_error"),
                KeyValue::new("exception.message", message),
            ],
        );
    } else {
        span.set_status(Status::Ok);
    }
}

/// Extract the remote span context from request headers.
///
/// Order: W3C `traceparent` (via the globally-installed propagator, which Cloud
/// Run injects) first; if absent/invalid, Google's `X-Cloud-Trace-Context`.
fn extract_remote_context(headers: &HeaderMap) -> Context {
    let w3c = opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderMapExtractor(headers))
    });
    if w3c.span().span_context().is_valid() {
        return w3c;
    }
    extract_x_cloud_trace_context(headers).unwrap_or(w3c)
}

/// Adapts actix's `HeaderMap` to the OpenTelemetry text-map [`Extractor`].
struct HeaderMapExtractor<'a>(&'a HeaderMap);

impl Extractor for HeaderMapExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|key| key.as_str()).collect()
    }
}

/// Parse Google Cloud's `X-Cloud-Trace-Context: TRACE_ID/SPAN_ID;o=OPTIONS`.
/// `TRACE_ID` is 32 hex chars; `SPAN_ID` is a **decimal** u64; `o=1` => sampled.
pub(crate) fn extract_x_cloud_trace_context(headers: &HeaderMap) -> Option<Context> {
    let raw = headers.get("x-cloud-trace-context")?.to_str().ok()?;
    let (trace_hex, rest) = raw.split_once('/')?;
    let trace_id = TraceId::from_hex(trace_hex).ok()?;

    let (span_decimal, options) = match rest.split_once(';') {
        Some((span, opts)) => (span, Some(opts)),
        None => (rest, None),
    };
    let span_id = SpanId::from_bytes(span_decimal.parse::<u64>().ok()?.to_be_bytes());

    let sampled = options.is_some_and(|opts| opts.contains("o=1"));
    let flags = if sampled {
        TraceFlags::SAMPLED
    } else {
        TraceFlags::default()
    };

    let span_context = SpanContext::new(
        trace_id,
        span_id,
        flags,
        /* is_remote */ true,
        TraceState::default(),
    );
    Some(Context::new().with_remote_span_context(span_context))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http::header::{HeaderName, HeaderValue};

    #[test]
    fn parses_x_cloud_trace_context() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-cloud-trace-context"),
            HeaderValue::from_static("105445aa7843bc8bf206b12000100000/1234567890;o=1"),
        );

        let cx = extract_x_cloud_trace_context(&headers).expect("should parse");
        let sc = cx.span().span_context().clone();

        assert!(sc.is_valid());
        assert_eq!(sc.trace_id().to_string(), "105445aa7843bc8bf206b12000100000");
        // 1234567890 decimal == 0x499602d2, left-padded to 16 hex chars.
        assert_eq!(sc.span_id().to_string(), "00000000499602d2");
        assert!(sc.is_sampled());
        assert!(sc.is_remote());
    }

    #[test]
    fn unsampled_when_no_options() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-cloud-trace-context"),
            HeaderValue::from_static("105445aa7843bc8bf206b12000100000/42"),
        );
        let cx = extract_x_cloud_trace_context(&headers).expect("should parse");
        assert!(!cx.span().span_context().is_sampled());
    }

    #[test]
    fn returns_none_for_missing_or_malformed() {
        assert!(extract_x_cloud_trace_context(&HeaderMap::new()).is_none());

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-cloud-trace-context"),
            HeaderValue::from_static("not-a-valid-context"),
        );
        assert!(extract_x_cloud_trace_context(&headers).is_none());
    }
}
