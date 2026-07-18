//! OpenTelemetry implementation (feature `otel`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use opentelemetry::global;
use opentelemetry::metrics::{Gauge, Histogram, Meter};
use opentelemetry::propagation::{Extractor, TextMapPropagator};
use opentelemetry::trace::{TraceContextExt, TracerProvider};
use opentelemetry::Context;
use opentelemetry::KeyValue;
use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing::field::Empty;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};

use crate::agent::AgentOtelState;
use crate::config::{SttVendor, TtsVendor, VoiceSessionContext};
use crate::error::{SpeechError, SpeechResult};
use crate::otel::VoiceSpan;
use crate::vad::VadTransition;

static OTEL_INIT: AtomicBool = AtomicBool::new(false);
static METER: OnceLock<Meter> = OnceLock::new();
static STT_LATENCY: OnceLock<Histogram<f64>> = OnceLock::new();
static TTS_LATENCY: OnceLock<Histogram<f64>> = OnceLock::new();
static POOL_WAIT: OnceLock<Histogram<f64>> = OnceLock::new();
static POOL_ENTRIES: OnceLock<Gauge<i64>> = OnceLock::new();

struct TraceparentExtractor<'a> {
    traceparent: &'a str,
}

impl Extractor for TraceparentExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        if key.eq_ignore_ascii_case("traceparent") {
            Some(self.traceparent)
        } else {
            None
        }
    }

    fn keys(&self) -> Vec<&str> {
        vec!["traceparent"]
    }
}

fn otel_disabled_by_env() -> bool {
    matches!(
        std::env::var("OTEL_SDK_DISABLED").ok().as_deref(),
        Some("true") | Some("1") | Some("yes")
    )
}

fn ensure_meter() -> &'static Meter {
    METER.get_or_init(|| global::meter("node-webrtc-rust-speech"))
}

fn stt_latency_histogram() -> &'static Histogram<f64> {
    STT_LATENCY.get_or_init(|| {
        ensure_meter()
            .f64_histogram("voice_stt_latency_ms")
            .with_description("STT utterance finalize latency in milliseconds")
            .build()
    })
}

fn tts_latency_histogram() -> &'static Histogram<f64> {
    TTS_LATENCY.get_or_init(|| {
        ensure_meter()
            .f64_histogram("voice_tts_latency_ms")
            .with_description("TTS synthesis latency in milliseconds")
            .build()
    })
}

fn pool_wait_histogram() -> &'static Histogram<f64> {
    POOL_WAIT.get_or_init(|| {
        ensure_meter()
            .f64_histogram("sherpa_pool_wait_ms")
            .with_description("Sherpa ONNX pool semaphore wait in milliseconds")
            .build()
    })
}

fn pool_entries_gauge() -> &'static Gauge<i64> {
    POOL_ENTRIES.get_or_init(|| {
        ensure_meter()
            .i64_gauge("sherpa_pool_entries")
            .with_description("Distinct Sherpa STT+TTS model directories loaded")
            .build()
    })
}

pub fn init_from_env() -> SpeechResult<()> {
    if OTEL_INIT.load(Ordering::SeqCst) || otel_disabled_by_env() {
        return Ok(());
    }

    let service_name = std::env::var("OTEL_SERVICE_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "node-webrtc-rust-voice".to_string());

    let resource = Resource::builder_empty()
        .with_attribute(opentelemetry::KeyValue::new("service.name", service_name))
        .build();

    let span_exporter = SpanExporter::builder()
        .with_http()
        .build()
        .map_err(|err| SpeechError::Internal(format!("otel span exporter: {err}")))?;

    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter)
        .with_resource(resource.clone())
        .build();

    global::set_tracer_provider(tracer_provider.clone());

    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .build()
        .map_err(|err| SpeechError::Internal(format!("otel metric exporter: {err}")))?;

    let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(metric_exporter).build();

    let meter_provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource)
        .build();

    global::set_meter_provider(meter_provider);

    let tracer = tracer_provider.tracer("node-webrtc-rust-speech");
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,node_webrtc_rust_speech=debug"));

    Registry::default()
        .with(filter)
        .with(telemetry)
        .try_init()
        .map_err(|err| SpeechError::Internal(format!("otel tracing subscriber: {err}")))?;

    let _ = ensure_meter();
    OTEL_INIT.store(true, Ordering::SeqCst);
    Ok(())
}

pub fn is_enabled() -> bool {
    OTEL_INIT.load(Ordering::SeqCst) && !otel_disabled_by_env()
}

fn parent_context(ctx: &VoiceSessionContext) -> Context {
    let propagator = TraceContextPropagator::new();
    if let Some(traceparent) = ctx.traceparent.as_deref().filter(|value| !value.is_empty()) {
        propagator.extract(&TraceparentExtractor { traceparent })
    } else {
        Context::current()
    }
}

fn apply_session_fields(span: tracing::Span, ctx: &VoiceSessionContext) -> tracing::Span {
    if let Some(value) = ctx.session_id.as_deref().filter(|v| !v.is_empty()) {
        span.record("session_id", value);
    }
    if let Some(value) = ctx.trace_id.as_deref().filter(|v| !v.is_empty()) {
        span.record("trace_id", value);
    }
    if let Some(value) = ctx.project_id.as_deref().filter(|v| !v.is_empty()) {
        span.record("project_id", value);
    }
    if let Some(value) = ctx.org_id.as_deref().filter(|v| !v.is_empty()) {
        span.record("org_id", value);
    }
    if let Some(value) = ctx.build_id.as_deref().filter(|v| !v.is_empty()) {
        span.record("build_id", value);
    }
    span
}

fn apply_vendor_field(
    span: tracing::Span,
    span_name: &'static str,
    vendor: Option<&str>,
) -> tracing::Span {
    if let Some(value) = vendor.filter(|v| !v.is_empty()) {
        match span_name {
            "voice.stt" => {
                span.record("stt.vendor", value);
            }
            "voice.tts" => {
                span.record("tts.vendor", value);
            }
            _ => {}
        }
    }
    span
}

pub fn begin_session(
    state: &mut AgentOtelState,
    ctx: VoiceSessionContext,
    stt_vendor: Option<SttVendor>,
    tts_vendor: Option<TtsVendor>,
) {
    let _ = init_from_env();
    state.session_context = ctx.clone();
    state.stt_vendor = stt_vendor;
    state.tts_vendor = tts_vendor;

    let span = tracing::info_span!(
        "voice.session",
        session_id = Empty,
        trace_id = Empty,
        project_id = Empty,
        org_id = Empty,
        build_id = Empty,
        stt.vendor = Empty,
        tts.vendor = Empty,
    );
    span.set_parent(parent_context(&ctx));
    let span = apply_session_fields(span, &ctx);
    if let Some(vendor) = stt_vendor {
        span.record("stt.vendor", vendor.as_str());
    }
    if let Some(vendor) = tts_vendor {
        span.record("tts.vendor", vendor.as_str());
    }
    state.session_span = Some(span);
}

pub fn end_session(state: &mut AgentOtelState) {
    state.session_span.take();
}

pub fn voice_span(
    name: &'static str,
    ctx: &VoiceSessionContext,
    vendor: Option<&str>,
) -> VoiceSpan {
    let span = match name {
        "voice.stt" => tracing::info_span!(
            "voice.stt",
            session_id = Empty,
            trace_id = Empty,
            project_id = Empty,
            org_id = Empty,
            build_id = Empty,
            stt.vendor = Empty,
        ),
        "voice.tts" => tracing::info_span!(
            "voice.tts",
            session_id = Empty,
            trace_id = Empty,
            project_id = Empty,
            org_id = Empty,
            build_id = Empty,
            tts.vendor = Empty,
        ),
        _ => tracing::info_span!(
            "voice.operation",
            session_id = Empty,
            trace_id = Empty,
            project_id = Empty,
            org_id = Empty,
            build_id = Empty,
            voice.operation = name,
        ),
    };
    let span = apply_session_fields(span, ctx);
    let span = apply_vendor_field(span, name, vendor);
    VoiceSpan {
        _entered: span.entered(),
    }
}

pub fn record_vad_transition(ctx: &VoiceSessionContext, transition: &VadTransition) {
    let transition_name = match transition {
        VadTransition::SpeechStart => "speech_start",
        VadTransition::SpeechEnd => "speech_end",
    };
    let span = tracing::info_span!(
        "voice.vad",
        session_id = Empty,
        trace_id = Empty,
        project_id = Empty,
        org_id = Empty,
        build_id = Empty,
        vad.transition = transition_name,
    );
    let _entered = apply_session_fields(span, ctx).entered();
}

pub fn record_gate_hold_start(ctx: &VoiceSessionContext, hold_ms: u32) {
    let span = tracing::info_span!(
        "voice.gate_hold",
        session_id = Empty,
        trace_id = Empty,
        project_id = Empty,
        org_id = Empty,
        build_id = Empty,
        gate_hold_ms = hold_ms,
    );
    let _entered = apply_session_fields(span, ctx).entered();
}

pub fn record_gate_hold_end(ctx: &VoiceSessionContext) {
    let span = tracing::info_span!(
        "voice.gate_hold",
        session_id = Empty,
        trace_id = Empty,
        project_id = Empty,
        org_id = Empty,
        build_id = Empty,
        gate_hold = "expired",
    );
    let _entered = apply_session_fields(span, ctx).entered();
}

pub fn record_barge_in(ctx: &VoiceSessionContext) {
    let span = tracing::info_span!(
        "voice.barge_in",
        session_id = Empty,
        trace_id = Empty,
        project_id = Empty,
        org_id = Empty,
        build_id = Empty,
    );
    let _entered = apply_session_fields(span, ctx).entered();
}

pub fn record_stt_latency_ms(ms: f64, vendor: Option<SttVendor>) {
    if is_enabled() {
        let attrs = vendor
            .map(|v| vec![KeyValue::new("stt.vendor", v.as_str())])
            .unwrap_or_default();
        stt_latency_histogram().record(ms, &attrs);
    }
}

pub fn record_tts_latency_ms(ms: f64, vendor: Option<TtsVendor>) {
    if is_enabled() {
        let attrs = vendor
            .map(|v| vec![KeyValue::new("tts.vendor", v.as_str())])
            .unwrap_or_default();
        tts_latency_histogram().record(ms, &attrs);
    }
}

pub fn record_sherpa_pool_wait_ms(ms: f64) {
    if is_enabled() {
        pool_wait_histogram().record(ms, &[]);
    }
}

pub fn set_sherpa_pool_entries(count: i64) {
    if is_enabled() {
        pool_entries_gauge().record(count, &[]);
    }
}

pub async fn acquire_sherpa_permit(
    semaphore: &tokio::sync::Semaphore,
) -> Result<tokio::sync::SemaphorePermit<'_>, tokio::sync::AcquireError> {
    let start = Instant::now();
    let permit = semaphore.acquire().await?;
    record_sherpa_pool_wait_ms(start.elapsed().as_secs_f64() * 1000.0);
    Ok(permit)
}

/// Parse W3C `traceparent` for tests and validation.
pub fn extract_trace_id(traceparent: &str) -> Option<String> {
    let propagator = TraceContextPropagator::new();
    let cx = propagator.extract(&TraceparentExtractor { traceparent });
    let span = cx.span();
    let trace_id = span.span_context().trace_id();
    if trace_id == opentelemetry::trace::TraceId::INVALID {
        None
    } else {
        Some(format!("{trace_id:x}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_trace_id_from_valid_traceparent() {
        let trace_id = extract_trace_id("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .expect("trace id");
        assert_eq!(trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
    }

    #[test]
    fn extract_trace_id_rejects_invalid_traceparent() {
        assert!(extract_trace_id("not-a-traceparent").is_none());
    }
}
