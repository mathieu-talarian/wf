//! OpenTelemetry wiring (spec §12).
//!
//! **Local dev** (`OTEL_EXPORTER_OTLP_ENDPOINT` unset): we install *only* the
//! pretty stdout layer — no OTLP exporters, no background batchers, nothing
//! shipped off-box. Spans and events render human-readably to the console.
//!
//! **Deployed** (Cloud Run): `service.yaml` sets `OTEL_EXPORTER_OTLP_ENDPOINT`
//! (and the other `OTEL_*` vars), which switches on the full **OTLP / gRPC**
//! pipeline to the OpenTelemetry Collector sidecar on `localhost:4317`, which
//! fans telemetry out to Google Cloud:
//!   * traces  -> Telemetry API (visible in Cloud Trace)
//!   * metrics -> Google Managed Service for Prometheus (visible in Cloud Monitoring)
//!   * logs    -> Cloud Logging
//!
//! Setting `OTEL_EXPORTER_OTLP_ENDPOINT` locally opts a dev into the export path
//! too; it's the single switch that distinguishes the two modes.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{Aggregation, Instrument, SdkMeterProvider, Stream};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{filter, fmt, EnvFilter, Layer};
use wf_core::Config;

/// Name reported as the OTel *instrumentation scope* for spans/metrics we emit.
pub(crate) const INSTRUMENTATION_SCOPE: &str = "wf-api";

/// Holds the provider handles so we can flush them on shutdown. OTLP exporters
/// batch in the background; dropping/forgetting them can lose the final spans,
/// metrics, and logs, so we explicitly `shutdown()` before the process exits.
///
/// All three are `None` on the local pretty-print-only path (no exporters built),
/// where `shutdown()` is a no-op.
pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<SdkLoggerProvider>,
}

impl TelemetryGuard {
    /// Flush and stop the exporters. Safe to call exactly once on shutdown.
    pub fn shutdown(self) {
        if let Some(provider) = self.tracer_provider {
            if let Err(err) = provider.shutdown() {
                eprintln!("error shutting down tracer provider: {err}");
            }
        }
        if let Some(provider) = self.meter_provider {
            if let Err(err) = provider.shutdown() {
                eprintln!("error shutting down meter provider: {err}");
            }
        }
        if let Some(provider) = self.logger_provider {
            if let Err(err) = provider.shutdown() {
                eprintln!("error shutting down logger provider: {err}");
            }
        }
    }
}

/// Build the OTel resource. `Resource::builder()` already merges in the standard
/// env detectors, so `OTEL_SERVICE_NAME` / `OTEL_RESOURCE_ATTRIBUTES` are picked
/// up automatically; we set `service.name` from config (which itself defaults to
/// `OTEL_SERVICE_NAME`) and tag the build version.
fn resource(config: &Config) -> Resource {
    Resource::builder()
        .with_service_name(config.otel_service_name.clone())
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .build()
}

/// `tracing` targets we must NOT forward to OTLP logs.
///
/// The OTLP exporter stack (reqwest/hyper/h2/tower) and the OTel SDK itself emit
/// their own `tracing` events. If we forwarded those as OTLP logs, an export
/// error would log an event, which exports, which can error — a feedback loop.
fn is_self_telemetry(target: &str) -> bool {
    const NOISY_PREFIXES: [&str; 6] =
        ["opentelemetry", "hyper", "h2", "tower", "reqwest", "tonic"];
    NOISY_PREFIXES.iter().any(|p| target.starts_with(p))
}

/// Refine inbound-HTTP latency histogram bucket boundaries (in seconds). The SDK
/// default boundaries are tuned for slow operations and give coarse percentiles
/// for typical API latencies.
fn latency_histogram_buckets(instrument: &Instrument) -> Option<Stream> {
    const DURATION_HISTOGRAMS: [&str; 1] = ["http.server.request.duration"];

    if DURATION_HISTOGRAMS.contains(&instrument.name()) {
        Stream::builder()
            .with_aggregation(Aggregation::ExplicitBucketHistogram {
                boundaries: vec![
                    0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                ],
                record_min_max: true,
            })
            .build()
            .ok()
    } else {
        None
    }
}

/// Boxed error returned by the telemetry setup path.
type SetupError = Box<dyn std::error::Error + Send + Sync>;

/// Build the OTLP/gRPC **traces** provider (batch exporter on a background thread).
fn build_tracer_provider(resource: &Resource, endpoint: &str) -> Result<SdkTracerProvider, SetupError> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;
    Ok(SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(exporter)
        .build())
}

/// Build the OTLP/gRPC **metrics** provider, with refined latency histogram buckets.
fn build_meter_provider(resource: &Resource, endpoint: &str) -> Result<SdkMeterProvider, SetupError> {
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;
    Ok(SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_periodic_exporter(exporter)
        .with_view(latency_histogram_buckets)
        .build())
}

/// Build the OTLP/gRPC **logs** provider (batch exporter on a background thread).
fn build_logger_provider(resource: &Resource, endpoint: &str) -> Result<SdkLoggerProvider, SetupError> {
    let exporter = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;
    Ok(SdkLoggerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(exporter)
        .build())
}

/// Install the export-path subscriber. Layer stack:
/// * EnvFilter — global level filter; respects RUST_LOG (defaults to info).
/// * fmt(.pretty) — human-readable stdout, omitted on Cloud Run (K_SERVICE):
///   logs already reach Cloud Logging via the OTLP logs pipeline, and keeping
///   stdout too would double every log line.
/// * OpenTelemetry (spans) — converts `tracing` spans into OTel spans.
/// * OpenTelemetry (logs) — converts `tracing` events into OTel logs, with a
///   per-layer filter to break the export feedback loop.
fn install_export_subscriber(
    env_filter: EnvFilter,
    tracer_provider: &SdkTracerProvider,
    logger_provider: &SdkLoggerProvider,
) {
    let tracer = tracer_provider.tracer(INSTRUMENTATION_SCOPE);
    let span_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let log_layer = OpenTelemetryTracingBridge::new(logger_provider)
        .with_filter(filter::filter_fn(|meta| !is_self_telemetry(meta.target())));
    let on_cloud_run = std::env::var("K_SERVICE").is_ok();
    let stdout_layer = (!on_cloud_run).then(|| fmt::layer().pretty());

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(span_layer)
        .with(log_layer)
        .init();
}

/// Wire the full OTLP / gRPC (tonic) traces+metrics+logs pipeline. All three
/// signals multiplex over the one `endpoint`.
fn init_otlp(
    config: &Config,
    endpoint: &str,
    env_filter: EnvFilter,
) -> Result<TelemetryGuard, SetupError> {
    let resource = resource(config);

    // W3C trace-context propagator so incoming `traceparent` headers (which Cloud
    // Run injects) can be extracted and continued; without it propagation no-ops.
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let tracer_provider = build_tracer_provider(&resource, endpoint)?;
    let meter_provider = build_meter_provider(&resource, endpoint)?;
    // Make `opentelemetry::global::meter(..)` resolve to this provider so the
    // HTTP-metrics middleware can build instruments from anywhere.
    opentelemetry::global::set_meter_provider(meter_provider.clone());
    let logger_provider = build_logger_provider(&resource, endpoint)?;

    install_export_subscriber(env_filter, &tracer_provider, &logger_provider);

    // Share this tracer with anything using the OTel API directly.
    opentelemetry::global::set_tracer_provider(tracer_provider.clone());

    Ok(TelemetryGuard {
        tracer_provider: Some(tracer_provider),
        meter_provider: Some(meter_provider),
        logger_provider: Some(logger_provider),
    })
}

/// Initialise tracing + metrics + logs and install the global subscriber.
///
/// Returns a [`TelemetryGuard`] whose `shutdown()` must be called before exit to
/// flush pending telemetry.
pub fn init(config: &Config) -> Result<TelemetryGuard, SetupError> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // Local dev: no OTLP endpoint configured -> pretty stdout only. We build no
    // exporters and no OTel providers, so there are no background batchers and
    // nothing is shipped off-box. `opentelemetry::global` keeps its default no-op
    // meter/tracer/propagator, which the middleware tolerates.
    let Some(endpoint) = config.otel_exporter_otlp_endpoint.as_deref() else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().pretty())
            .init();
        return Ok(TelemetryGuard {
            tracer_provider: None,
            meter_provider: None,
            logger_provider: None,
        });
    };

    // Export path: endpoint configured (Cloud Run sets it via `service.yaml`;
    // locally it's an explicit opt-in).
    init_otlp(config, endpoint, env_filter)
}
