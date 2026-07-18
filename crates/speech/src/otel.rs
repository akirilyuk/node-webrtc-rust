//! OpenTelemetry spans and metrics for the voice pipeline.
//!
//! Enable with Cargo feature `otel` (off by default). When disabled, all hooks are no-ops.

#[cfg(feature = "otel")]
mod enabled;

#[cfg(feature = "otel")]
pub use enabled::extract_trace_id;

/// Span guard for scoped voice operations (no-op without `otel`).
pub struct VoiceSpan {
    #[cfg(feature = "otel")]
    _entered: tracing::span::EnteredSpan,
}

impl VoiceSpan {
    pub fn noop() -> Self {
        #[cfg(feature = "otel")]
        {
            Self {
                _entered: tracing::Span::none().entered(),
            }
        }
        #[cfg(not(feature = "otel"))]
        {
            Self {}
        }
    }
}

#[cfg(feature = "otel")]
pub use enabled::{
    acquire_sherpa_permit, begin_session, end_session, init_from_env, is_enabled, record_barge_in,
    record_gate_hold_end, record_gate_hold_start, record_sherpa_pool_wait_ms,
    record_stt_latency_ms, record_tts_latency_ms, record_vad_transition, set_sherpa_pool_entries,
    voice_span,
};

#[cfg(not(feature = "otel"))]
pub fn init_from_env() -> crate::error::SpeechResult<()> {
    Ok(())
}

#[cfg(not(feature = "otel"))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(not(feature = "otel"))]
pub fn begin_session(
    state: &mut crate::agent::AgentOtelState,
    ctx: crate::config::VoiceSessionContext,
    _stt_vendor: Option<crate::config::SttVendor>,
    _tts_vendor: Option<crate::config::TtsVendor>,
) {
    state.session_context = ctx;
}

#[cfg(not(feature = "otel"))]
pub fn end_session(_state: &mut crate::agent::AgentOtelState) {}

#[cfg(not(feature = "otel"))]
pub fn voice_span(
    _name: &'static str,
    _ctx: &crate::config::VoiceSessionContext,
    _vendor: Option<&str>,
) -> VoiceSpan {
    VoiceSpan::noop()
}

#[cfg(not(feature = "otel"))]
pub fn record_vad_transition(
    _ctx: &crate::config::VoiceSessionContext,
    _transition: &crate::vad::VadTransition,
) {
}

#[cfg(not(feature = "otel"))]
pub fn record_gate_hold_start(_ctx: &crate::config::VoiceSessionContext, _hold_ms: u32) {}

#[cfg(not(feature = "otel"))]
pub fn record_gate_hold_end(_ctx: &crate::config::VoiceSessionContext) {}

#[cfg(not(feature = "otel"))]
pub fn record_barge_in(_ctx: &crate::config::VoiceSessionContext) {}

#[cfg(not(feature = "otel"))]
pub fn record_stt_latency_ms(_ms: f64, _vendor: Option<crate::config::SttVendor>) {}

#[cfg(not(feature = "otel"))]
pub fn record_tts_latency_ms(_ms: f64, _vendor: Option<crate::config::TtsVendor>) {}

#[cfg(not(feature = "otel"))]
pub fn record_sherpa_pool_wait_ms(_ms: f64) {}

#[cfg(not(feature = "otel"))]
pub fn set_sherpa_pool_entries(_count: i64) {}

#[cfg(not(feature = "otel"))]
pub async fn acquire_sherpa_permit(
    semaphore: &tokio::sync::Semaphore,
) -> Result<tokio::sync::SemaphorePermit<'_>, tokio::sync::AcquireError> {
    semaphore.acquire().await
}

#[cfg(not(feature = "otel"))]
pub fn extract_trace_id(_traceparent: &str) -> Option<String> {
    None
}
