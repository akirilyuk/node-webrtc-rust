//! JavaScript types for voice agent APIs.

use napi::bindgen_prelude::*;
use napi_derive::napi;
use node_webrtc_rust_speech::{
    BargeInConfig, EventDeliveryMode, EventsConfig, NoiseSuppressionConfig,
    NoiseSuppressionProvider, SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig,
    VadSampleRate, VoiceAgentConfig, VoiceSessionContext,
};

#[napi(string_enum)]
#[derive(Debug)]
pub enum JsEventDeliveryMode {
    #[napi(value = "callback")]
    Callback,
    #[napi(value = "stream")]
    Stream,
    #[napi(value = "both")]
    Both,
}

impl From<JsEventDeliveryMode> for EventDeliveryMode {
    fn from(value: JsEventDeliveryMode) -> Self {
        match value {
            JsEventDeliveryMode::Callback => Self::Callback,
            JsEventDeliveryMode::Stream => Self::Stream,
            JsEventDeliveryMode::Both => Self::Both,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsEventsConfig {
    pub mode: Option<JsEventDeliveryMode>,
}

impl From<JsEventsConfig> for EventsConfig {
    fn from(value: JsEventsConfig) -> Self {
        Self {
            mode: value
                .mode
                .map(Into::into)
                .unwrap_or(EventDeliveryMode::Both),
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsBargeInConfig {
    pub enabled: Option<bool>,
    pub use_vad: Option<bool>,
    pub flush_tts: Option<bool>,
    pub require_stt_partial: Option<bool>,
    /// Minimum whitespace-separated tokens in an STT partial before semantic barge (default 2).
    pub min_stt_partial_tokens: Option<u32>,
    /// Legacy alias for [`Self::min_stt_partial_tokens`] (same meaning: token count, not chars).
    pub min_stt_partial_chars: Option<u32>,
    pub agent_playback_guard_ms: Option<u32>,
}

impl From<JsBargeInConfig> for BargeInConfig {
    fn from(value: JsBargeInConfig) -> Self {
        Self {
            enabled: value.enabled.unwrap_or(true),
            use_vad: value.use_vad.unwrap_or(true),
            flush_tts: value.flush_tts.unwrap_or(true),
            require_stt_partial: value.require_stt_partial.unwrap_or(true),
            min_stt_partial_tokens: node_webrtc_rust_speech::resolve_min_stt_partial_tokens(
                value.min_stt_partial_tokens,
                value.min_stt_partial_chars,
            ),
            agent_playback_guard_ms: value
                .agent_playback_guard_ms
                .unwrap_or_else(default_agent_playback_guard_ms),
        }
    }
}

fn default_agent_playback_guard_ms() -> u32 {
    0
}

#[cfg(test)]
mod barge_in_config_from_tests {
    use super::*;

    #[test]
    fn legacy_min_stt_partial_chars_maps_to_tokens() {
        let cfg: BargeInConfig = JsBargeInConfig {
            min_stt_partial_chars: Some(3),
            min_stt_partial_tokens: None,
            ..Default::default()
        }
        .into();
        assert_eq!(cfg.min_stt_partial_tokens, 3);
    }

    #[test]
    fn min_stt_partial_tokens_wins_over_legacy_chars() {
        let cfg: BargeInConfig = JsBargeInConfig {
            min_stt_partial_chars: Some(9),
            min_stt_partial_tokens: Some(2),
            ..Default::default()
        }
        .into();
        assert_eq!(cfg.min_stt_partial_tokens, 2);
    }

    #[test]
    fn default_min_stt_partial_tokens_is_two() {
        let cfg: BargeInConfig = JsBargeInConfig::default().into();
        assert_eq!(cfg.min_stt_partial_tokens, 2);
    }
}

#[napi(string_enum)]
#[derive(Debug)]
pub enum JsVadSampleRate {
    #[napi(value = "8000")]
    Hz8000,
    #[napi(value = "16000")]
    Hz16000,
}

impl From<JsVadSampleRate> for VadSampleRate {
    fn from(value: JsVadSampleRate) -> Self {
        match value {
            JsVadSampleRate::Hz8000 => Self::Hz8000,
            JsVadSampleRate::Hz16000 => Self::Hz16000,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsVadConfig {
    pub enabled: Option<bool>,
    pub provider: Option<String>,
    pub threshold: Option<f64>,
    pub min_speech_duration_ms: Option<u32>,
    pub min_silence_duration_ms: Option<u32>,
    pub speech_pad_ms: Option<u32>,
    pub sample_rate: Option<JsVadSampleRate>,
    pub barge_in: Option<JsBargeInConfig>,
    pub gate_stt: Option<bool>,
    pub gate_stt_open_on_pending: Option<bool>,
    pub stt_gate_hold_ms: Option<u32>,
    pub stt_listen_timeout_ms: Option<u32>,
    pub utterance_finalize_timeout_ms: Option<u32>,
}

impl From<JsVadConfig> for VadConfig {
    fn from(value: JsVadConfig) -> Self {
        Self {
            enabled: value.enabled.unwrap_or(true),
            provider: value.provider.unwrap_or_else(|| "energy".to_string()),
            threshold: value.threshold.unwrap_or(0.15) as f32,
            min_speech_duration_ms: value.min_speech_duration_ms.unwrap_or(250),
            min_silence_duration_ms: value.min_silence_duration_ms.unwrap_or(1300),
            speech_pad_ms: value.speech_pad_ms.unwrap_or(300),
            sample_rate: value
                .sample_rate
                .map(Into::into)
                .unwrap_or(VadSampleRate::Hz16000),
            barge_in: value.barge_in.map(Into::into).unwrap_or_default(),
            gate_stt: value.gate_stt.unwrap_or(false),
            gate_stt_open_on_pending: value.gate_stt_open_on_pending.unwrap_or(true),
            stt_gate_hold_ms: value.stt_gate_hold_ms.unwrap_or(1000),
            stt_listen_timeout_ms: value.stt_listen_timeout_ms.unwrap_or(4000),
            utterance_finalize_timeout_ms: value.utterance_finalize_timeout_ms.unwrap_or(1500),
        }
    }
}

#[napi(string_enum)]
#[derive(Debug)]
pub enum JsSttVendor {
    #[napi(value = "openai")]
    Openai,
    #[napi(value = "deepgram")]
    Deepgram,
    #[napi(value = "google")]
    Google,
    #[napi(value = "assemblyai")]
    Assemblyai,
    #[napi(value = "local-sherpa")]
    LocalSherpa,
    #[napi(value = "mock")]
    Mock,
}

impl From<JsSttVendor> for SttVendor {
    fn from(value: JsSttVendor) -> Self {
        match value {
            JsSttVendor::Openai => Self::Openai,
            JsSttVendor::Deepgram => Self::Deepgram,
            JsSttVendor::Google => Self::Google,
            JsSttVendor::Assemblyai => Self::Assemblyai,
            JsSttVendor::LocalSherpa => Self::LocalSherpa,
            JsSttVendor::Mock => Self::Mock,
        }
    }
}

#[napi(string_enum)]
#[derive(Debug)]
pub enum JsTtsVendor {
    #[napi(value = "openai")]
    Openai,
    #[napi(value = "elevenlabs")]
    Elevenlabs,
    #[napi(value = "google")]
    Google,
    #[napi(value = "cartesia")]
    Cartesia,
    #[napi(value = "local-sherpa")]
    LocalSherpa,
    #[napi(value = "mock")]
    Mock,
}

impl From<JsTtsVendor> for TtsVendor {
    fn from(value: JsTtsVendor) -> Self {
        match value {
            JsTtsVendor::Openai => Self::Openai,
            JsTtsVendor::Elevenlabs => Self::Elevenlabs,
            JsTtsVendor::Google => Self::Google,
            JsTtsVendor::Cartesia => Self::Cartesia,
            JsTtsVendor::LocalSherpa => Self::LocalSherpa,
            JsTtsVendor::Mock => Self::Mock,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSttConfig {
    pub provider: JsSttVendor,
    pub model: Option<String>,
    pub model_path: Option<String>,
    pub language: Option<String>,
    pub api_key: Option<String>,
}

impl From<JsSttConfig> for SttConfig {
    fn from(value: JsSttConfig) -> Self {
        Self {
            provider: value.provider.into(),
            model: value.model,
            model_path: value.model_path,
            language: value.language,
            api_key: value.api_key,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsTtsConfig {
    pub provider: JsTtsVendor,
    pub model: Option<String>,
    pub model_path: Option<String>,
    pub voice: Option<String>,
    pub api_key: Option<String>,
    pub post_utterance_silence_ms: Option<u32>,
}

impl From<JsTtsConfig> for TtsConfig {
    fn from(value: JsTtsConfig) -> Self {
        Self {
            provider: value.provider.into(),
            model: value.model,
            model_path: value.model_path,
            voice: value.voice,
            api_key: value.api_key,
        }
    }
}

#[napi(string_enum)]
#[derive(Debug)]
pub enum JsNoiseSuppressionProvider {
    #[napi(value = "none")]
    None,
    #[napi(value = "rnnoise")]
    Rnnoise,
}

impl From<JsNoiseSuppressionProvider> for NoiseSuppressionProvider {
    fn from(value: JsNoiseSuppressionProvider) -> Self {
        match value {
            JsNoiseSuppressionProvider::None => Self::None,
            JsNoiseSuppressionProvider::Rnnoise => Self::Rnnoise,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsNoiseSuppressionConfig {
    pub provider: Option<JsNoiseSuppressionProvider>,
}

impl From<JsNoiseSuppressionConfig> for NoiseSuppressionConfig {
    fn from(value: JsNoiseSuppressionConfig) -> Self {
        Self {
            provider: value
                .provider
                .map(Into::into)
                .unwrap_or(NoiseSuppressionProvider::None),
        }
    }
}

#[cfg(test)]
mod noise_suppression_config_from_tests {
    use super::*;

    #[test]
    fn omitted_provider_maps_to_none() {
        let cfg: NoiseSuppressionConfig = JsNoiseSuppressionConfig {
            provider: None,
        }
        .into();
        assert_eq!(cfg.provider, NoiseSuppressionProvider::None);
    }

    #[test]
    fn rnnoise_provider_maps_to_rnnoise() {
        let cfg: NoiseSuppressionConfig = JsNoiseSuppressionConfig {
            provider: Some(JsNoiseSuppressionProvider::Rnnoise),
        }
        .into();
        assert_eq!(cfg.provider, NoiseSuppressionProvider::Rnnoise);
    }

    #[test]
    fn voice_agent_omitted_noise_suppression_defaults_to_none() {
        let cfg: VoiceAgentConfig = JsVoiceAgentConfig::default().into();
        assert_eq!(cfg.noise_suppression.provider, NoiseSuppressionProvider::None);
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsVoiceSessionContext {
    pub session_id: Option<String>,
    pub trace_id: Option<String>,
    pub project_id: Option<String>,
    pub org_id: Option<String>,
    pub build_id: Option<String>,
    pub traceparent: Option<String>,
}

impl From<JsVoiceSessionContext> for VoiceSessionContext {
    fn from(value: JsVoiceSessionContext) -> Self {
        Self {
            session_id: value.session_id,
            trace_id: value.trace_id,
            project_id: value.project_id,
            org_id: value.org_id,
            build_id: value.build_id,
            traceparent: value.traceparent,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsVoiceAgentConfig {
    pub vad: Option<JsVadConfig>,
    pub events: Option<JsEventsConfig>,
    pub stt: Option<JsSttConfig>,
    pub tts: Option<JsTtsConfig>,
    pub post_utterance_silence_ms: Option<u32>,
    pub noise_suppression: Option<JsNoiseSuppressionConfig>,
}

impl From<JsVoiceAgentConfig> for VoiceAgentConfig {
    fn from(value: JsVoiceAgentConfig) -> Self {
        let post_utterance_silence_ms = value
            .post_utterance_silence_ms
            .or_else(|| value.tts.as_ref().and_then(|tts| tts.post_utterance_silence_ms));
        Self {
            vad: value.vad.map(Into::into).unwrap_or_default(),
            events: value.events.map(Into::into).unwrap_or_default(),
            stt: value.stt.map(Into::into),
            tts: value.tts.map(Into::into),
            post_utterance_silence_ms,
            noise_suppression: value
                .noise_suppression
                .map(Into::into)
                .unwrap_or_default(),
        }
    }
}

#[napi(string_enum)]
#[derive(Debug)]
pub enum JsSpeechEventType {
    #[napi(value = "user_speaking_start")]
    UserSpeakingStart,
    #[napi(value = "user_speaking_end")]
    UserSpeakingEnd,
    #[napi(value = "user_speech_partial")]
    UserSpeechPartial,
    #[napi(value = "user_speech_final")]
    UserSpeechFinal,
    #[napi(value = "agent_speaking_start")]
    AgentSpeakingStart,
    #[napi(value = "agent_speaking_end")]
    AgentSpeakingEnd,
    #[napi(value = "vad_triggered")]
    VadTriggered,
    #[napi(value = "stt_stream_start")]
    SttStreamStart,
    #[napi(value = "stt_stream_end")]
    SttStreamEnd,
    #[napi(value = "user_stt_start")]
    UserSttStart,
    #[napi(value = "user_stt_end")]
    UserSttEnd,
    #[napi(value = "user_stt_not_found")]
    UserSttNotFound,
    #[napi(value = "barge_in")]
    BargeIn,
    #[napi(value = "error")]
    Error,
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSpeechEvent {
    pub event_type: JsSpeechEventType,
    pub text: Option<String>,
    pub error: Option<String>,
}

pub fn speech_err(err: node_webrtc_rust_speech::SpeechError) -> Error {
    Error::from_reason(err.to_string())
}
