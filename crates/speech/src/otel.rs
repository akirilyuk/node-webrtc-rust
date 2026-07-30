//! OpenTelemetry spans and metrics for the voice pipeline.
//!
//! Enable with Cargo feature `otel` (off by default). When disabled, all hooks are no-ops.

use crate::config::{SttConfig, TtsConfig};

/// Shared Sherpa / voice TTS metric attributes (also used when `otel` feature is disabled).
///
/// `tts_model` is the catalog model id (e.g. `en-amy-medium`). `tts_model_dir` is the
/// resolved filesystem dir **basename** only (not an absolute path).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SherpaTtsMetricAttrs {
    pub tts_vendor: String,
    pub tts_model: String,
    pub tts_model_dir: String,
    pub tts_language: String,
    pub tts_voice: String,
    pub project_id: String,
}

/// Shared STT metric attributes for latency (and related) series.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SttMetricAttrs {
    pub stt_vendor: String,
    pub stt_model: String,
    pub stt_language: String,
    pub project_id: String,
}

/// Basename of a model path (drops absolute prefixes for low-cardinality labels).
pub fn path_basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// Prefer catalog `model` id; fall back to basename of `model_path`.
pub fn catalog_model_label(model: Option<&str>, model_path: Option<&str>) -> String {
    if let Some(id) = model.map(str::trim).filter(|value| !value.is_empty()) {
        return id.to_string();
    }
    model_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(path_basename)
        .unwrap_or_default()
}

impl SttMetricAttrs {
    pub fn from_stt_config(config: &SttConfig, project_id: &str) -> Self {
        Self {
            stt_vendor: config.provider.as_str().to_string(),
            stt_model: catalog_model_label(config.model.as_deref(), config.model_path.as_deref()),
            stt_language: config
                .language
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("")
                .to_string(),
            project_id: project_id.to_string(),
        }
    }
}

impl SherpaTtsMetricAttrs {
    pub fn from_tts_config(config: &TtsConfig, project_id: &str) -> Self {
        Self {
            tts_vendor: config.provider.as_str().to_string(),
            tts_model: catalog_model_label(config.model.as_deref(), config.model_path.as_deref()),
            tts_model_dir: config
                .model_path
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(path_basename)
                .unwrap_or_default(),
            tts_language: String::new(),
            tts_voice: config
                .voice
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("")
                .to_string(),
            project_id: project_id.to_string(),
        }
    }
}

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
    record_sherpa_tts_phrase_cache_hit, record_sherpa_tts_phrase_cache_miss,
    record_sherpa_tts_queue_wait_ms, record_sherpa_tts_synth_wall_ms, record_stt_latency_ms,
    record_tts_latency_ms, record_vad_transition, set_sherpa_pool_entries,
    set_sherpa_tts_phrase_cache_entries, voice_span,
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
pub fn record_stt_latency_ms(_ms: f64, _attrs: &SttMetricAttrs) {}

#[cfg(not(feature = "otel"))]
pub fn record_tts_latency_ms(_ms: f64, _attrs: &SherpaTtsMetricAttrs) {}

#[cfg(not(feature = "otel"))]
pub fn record_sherpa_pool_wait_ms(_ms: f64, _attrs: Option<&SherpaTtsMetricAttrs>) {}

#[cfg(not(feature = "otel"))]
pub fn set_sherpa_pool_entries(_count: i64) {}

#[cfg(not(feature = "otel"))]
pub fn record_sherpa_tts_phrase_cache_hit(_attrs: &SherpaTtsMetricAttrs) {}

#[cfg(not(feature = "otel"))]
pub fn record_sherpa_tts_phrase_cache_miss(_attrs: &SherpaTtsMetricAttrs) {}

#[cfg(not(feature = "otel"))]
pub fn set_sherpa_tts_phrase_cache_entries(_count: i64, _attrs: &SherpaTtsMetricAttrs) {}

#[cfg(not(feature = "otel"))]
pub fn record_sherpa_tts_queue_wait_ms(_ms: f64, _attrs: &SherpaTtsMetricAttrs) {}

#[cfg(not(feature = "otel"))]
pub fn record_sherpa_tts_synth_wall_ms(_ms: f64, _attrs: &SherpaTtsMetricAttrs) {}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_basename_strips_dirs() {
        assert_eq!(path_basename("/models/vits-piper-en_US-amy-medium"), "vits-piper-en_US-amy-medium");
        assert_eq!(path_basename("amy"), "amy");
    }

    #[test]
    fn catalog_model_label_prefers_model_id() {
        assert_eq!(
            catalog_model_label(Some("en-amy-medium"), Some("/models/foo")),
            "en-amy-medium"
        );
        assert_eq!(
            catalog_model_label(None, Some("/models/vits-piper-en_US-amy-medium")),
            "vits-piper-en_US-amy-medium"
        );
        assert_eq!(catalog_model_label(Some("  "), None), "");
    }
}
