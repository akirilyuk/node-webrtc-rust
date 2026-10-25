//! JavaScript types for voice agent APIs.

use napi::bindgen_prelude::*;
use napi_derive::napi;
use node_webrtc_rust_speech::{
    BargeInConfig, EventDeliveryMode, EventsConfig, NoiseSuppressionConfig,
    NoiseSuppressionProvider, SentenceCase, SpeechTransport, SttConfig, SttVendor, TtsConfig,
    TtsVendor, VadConfig, VadSampleRate, VoiceAgentConfig, VoiceSessionContext,
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
    pub stt_listen_backlog_slack_ms: Option<u32>,
    pub stt_listen_hard_timeout_ms: Option<u32>,
    /// Max time a turn keeps waiting while the cluster STT stream open is refused and retried (default 120000 ms; 0 = no extra wait).
    pub stt_open_wait_max_ms: Option<u32>,
    pub utterance_finalize_timeout_ms: Option<u32>,
}

impl From<JsVadConfig> for VadConfig {
    fn from(value: JsVadConfig) -> Self {
        Self {
            enabled: value.enabled.unwrap_or(true),
            provider: value.provider.unwrap_or_else(|| "energy".to_string()),
            threshold: value.threshold.unwrap_or(0.15) as f32,
            min_speech_duration_ms: value.min_speech_duration_ms.unwrap_or(250),
            min_silence_duration_ms: value.min_silence_duration_ms.unwrap_or(1100),
            speech_pad_ms: value.speech_pad_ms.unwrap_or(300),
            sample_rate: value
                .sample_rate
                .map(Into::into)
                .unwrap_or(VadSampleRate::Hz16000),
            barge_in: value.barge_in.map(Into::into).unwrap_or_default(),
            gate_stt: value.gate_stt.unwrap_or(false),
            gate_stt_open_on_pending: value.gate_stt_open_on_pending.unwrap_or(true),
            stt_gate_hold_ms: value.stt_gate_hold_ms.unwrap_or(400),
            stt_listen_timeout_ms: value.stt_listen_timeout_ms.unwrap_or(4000),
            stt_listen_backlog_slack_ms: value.stt_listen_backlog_slack_ms.unwrap_or(200),
            stt_listen_hard_timeout_ms: value.stt_listen_hard_timeout_ms.unwrap_or(0),
            stt_open_wait_max_ms: value.stt_open_wait_max_ms.unwrap_or(120_000),
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
    #[napi(value = "elevenlabs")]
    Elevenlabs,
    #[napi(value = "fishaudio")]
    Fishaudio,
    #[napi(value = "mistral")]
    Mistral,
    #[napi(value = "google")]
    Google,
    #[napi(value = "assemblyai")]
    Assemblyai,
    #[napi(value = "groq")]
    Groq,
    #[napi(value = "azure")]
    Azure,
    #[napi(value = "aws")]
    Aws,
    #[napi(value = "local-sherpa")]
    LocalSherpa,
    /// @internal cloud runner transport. Not a public vendor.
    #[napi(value = "cluster-sherpa")]
    ClusterSherpa,
    #[napi(value = "mock")]
    Mock,
}

impl From<JsSttVendor> for SttVendor {
    fn from(value: JsSttVendor) -> Self {
        match value {
            JsSttVendor::Openai => Self::Openai,
            JsSttVendor::Deepgram => Self::Deepgram,
            JsSttVendor::Elevenlabs => Self::Elevenlabs,
            JsSttVendor::Fishaudio => Self::Fishaudio,
            JsSttVendor::Mistral => Self::Mistral,
            JsSttVendor::Google => Self::Google,
            JsSttVendor::Assemblyai => Self::Assemblyai,
            JsSttVendor::Groq => Self::Groq,
            JsSttVendor::Azure => Self::Azure,
            JsSttVendor::Aws => Self::Aws,
            JsSttVendor::LocalSherpa => Self::LocalSherpa,
            JsSttVendor::ClusterSherpa => Self::ClusterSherpa,
            JsSttVendor::Mock => Self::Mock,
        }
    }
}

#[napi(string_enum)]
#[derive(Debug)]
pub enum JsTtsVendor {
    #[napi(value = "openai")]
    Openai,
    #[napi(value = "deepgram")]
    Deepgram,
    #[napi(value = "elevenlabs")]
    Elevenlabs,
    #[napi(value = "google")]
    Google,
    #[napi(value = "cartesia")]
    Cartesia,
    #[napi(value = "fishaudio")]
    Fishaudio,
    #[napi(value = "mistral")]
    Mistral,
    #[napi(value = "groq")]
    Groq,
    #[napi(value = "azure")]
    Azure,
    #[napi(value = "aws")]
    Aws,
    #[napi(value = "local-sherpa")]
    LocalSherpa,
    /// @internal cloud runner transport. Not a public vendor.
    #[napi(value = "cluster-sherpa")]
    ClusterSherpa,
    #[napi(value = "mock")]
    Mock,
}

impl From<JsTtsVendor> for TtsVendor {
    fn from(value: JsTtsVendor) -> Self {
        match value {
            JsTtsVendor::Openai => Self::Openai,
            JsTtsVendor::Deepgram => Self::Deepgram,
            JsTtsVendor::Elevenlabs => Self::Elevenlabs,
            JsTtsVendor::Google => Self::Google,
            JsTtsVendor::Cartesia => Self::Cartesia,
            JsTtsVendor::Fishaudio => Self::Fishaudio,
            JsTtsVendor::Mistral => Self::Mistral,
            JsTtsVendor::Groq => Self::Groq,
            JsTtsVendor::Azure => Self::Azure,
            JsTtsVendor::Aws => Self::Aws,
            JsTtsVendor::LocalSherpa => Self::LocalSherpa,
            JsTtsVendor::ClusterSherpa => Self::ClusterSherpa,
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
    pub endpoint: Option<String>,
    /// `"auto"`, `"websocket"` or `"http"`; omitted = `"auto"` (the voice-catalog default).
    pub transport: Option<String>,
}

/// Parse the JS `transport` string. Unknown values fail the config with the accepted list.
fn parse_js_transport(leg: &str, raw: Option<String>) -> Result<Option<SpeechTransport>> {
    match raw {
        None => Ok(None),
        Some(value) => SpeechTransport::parse(&value).map(Some).ok_or_else(|| {
            Error::from_reason(format!(
                "invalid {leg} transport '{value}'; expected one of: auto, websocket, http"
            ))
        }),
    }
}

impl TryFrom<JsSttConfig> for SttConfig {
    type Error = Error;

    fn try_from(value: JsSttConfig) -> Result<Self> {
        Ok(Self {
            provider: value.provider.into(),
            model: value.model,
            model_path: value.model_path,
            language: value.language,
            api_key: value.api_key,
            endpoint: value.endpoint,
            transport: parse_js_transport("stt", value.transport)?,
        })
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
    pub endpoint: Option<String>,
    pub post_utterance_silence_ms: Option<u32>,
    /// `"auto"` (default) or `"off"`; see `SentenceCase`.
    pub sentence_case: Option<String>,
    /// `"auto"`, `"websocket"` or `"http"`; omitted = `"auto"` (the voice-catalog default).
    pub transport: Option<String>,
}

impl TryFrom<JsTtsConfig> for TtsConfig {
    type Error = Error;

    fn try_from(value: JsTtsConfig) -> Result<Self> {
        Ok(Self {
            provider: value.provider.into(),
            model: value.model,
            model_path: value.model_path,
            voice: value.voice,
            api_key: value.api_key,
            endpoint: value.endpoint,
            sentence_case: value
                .sentence_case
                .as_deref()
                .map(SentenceCase::from_wire)
                .unwrap_or_default(),
            transport: parse_js_transport("tts", value.transport)?,
        })
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
mod transport_config_from_tests {
    use super::*;

    fn js_tts(transport: Option<&str>) -> JsTtsConfig {
        JsTtsConfig {
            provider: JsTtsVendor::Deepgram,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
            post_utterance_silence_ms: None,
            sentence_case: None,
            transport: transport.map(str::to_owned),
        }
    }

    fn js_stt(transport: Option<&str>) -> JsSttConfig {
        JsSttConfig {
            provider: JsSttVendor::Deepgram,
            model: None,
            model_path: None,
            language: None,
            api_key: None,
            endpoint: None,
            transport: transport.map(str::to_owned),
        }
    }

    #[test]
    fn tts_transport_http_arrives_as_http() {
        let cfg = TtsConfig::try_from(js_tts(Some("http"))).unwrap();
        assert_eq!(cfg.transport, Some(SpeechTransport::Http));
        assert_eq!(cfg.provider, TtsVendor::Deepgram);
    }

    #[test]
    fn stt_transport_websocket_arrives_as_websocket() {
        let cfg = SttConfig::try_from(js_stt(Some("websocket"))).unwrap();
        assert_eq!(cfg.transport, Some(SpeechTransport::Websocket));
    }

    #[test]
    fn omitted_transport_is_none_and_auto_is_auto() {
        assert_eq!(TtsConfig::try_from(js_tts(None)).unwrap().transport, None);
        assert_eq!(
            TtsConfig::try_from(js_tts(Some("auto"))).unwrap().transport,
            Some(SpeechTransport::Auto)
        );
    }

    #[test]
    fn invalid_transport_string_fails_with_a_clear_error() {
        let err = TtsConfig::try_from(js_tts(Some("grpc"))).unwrap_err();
        assert_eq!(
            err.reason,
            "invalid tts transport 'grpc'; expected one of: auto, websocket, http"
        );
        let err = SttConfig::try_from(js_stt(Some("HTTP"))).unwrap_err();
        assert!(
            err.reason.contains("invalid stt transport 'HTTP'"),
            "{}",
            err.reason
        );
    }

    #[test]
    fn invalid_transport_in_voice_agent_config_fails() {
        let cfg = JsVoiceAgentConfig {
            tts: Some(js_tts(Some("carrier-pigeon"))),
            ..Default::default()
        };
        assert!(VoiceAgentConfig::try_from(cfg).is_err());
        let ok = JsVoiceAgentConfig {
            tts: Some(js_tts(Some("http"))),
            ..Default::default()
        };
        let cfg = VoiceAgentConfig::try_from(ok).unwrap();
        assert_eq!(cfg.tts.unwrap().transport, Some(SpeechTransport::Http));
    }
}

#[cfg(test)]
mod noise_suppression_config_from_tests {
    use super::*;

    #[test]
    fn omitted_provider_maps_to_none() {
        let cfg: NoiseSuppressionConfig = JsNoiseSuppressionConfig { provider: None }.into();
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
        let cfg: VoiceAgentConfig = JsVoiceAgentConfig::default().try_into().unwrap();
        assert_eq!(
            cfg.noise_suppression.provider,
            NoiseSuppressionProvider::None
        );
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
pub struct JsLanguageIdConfig {
    pub enabled: Option<bool>,
    pub model_path: Option<String>,
    pub allowlist: Option<Vec<String>>,
    pub min_speech_ms: Option<u32>,
    pub continuous: Option<bool>,
    pub timing: Option<String>,
    pub lid_max_clip_ms: Option<u32>,
    pub lid_gate_max_wait_ms: Option<u32>,
    pub tts_exclusion: Option<bool>,
}

impl From<JsLanguageIdConfig> for node_webrtc_rust_speech::config::LanguageIdConfig {
    fn from(value: JsLanguageIdConfig) -> Self {
        Self {
            enabled: value.enabled,
            model_path: value.model_path,
            allowlist: value.allowlist,
            min_speech_ms: value.min_speech_ms,
            continuous: value.continuous,
            timing: value
                .timing
                .as_deref()
                .and_then(node_webrtc_rust_speech::config::LanguageIdTiming::parse),
            lid_max_clip_ms: value.lid_max_clip_ms,
            lid_gate_max_wait_ms: value.lid_gate_max_wait_ms,
            tts_exclusion: value.tts_exclusion,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsReplayConfig {
    pub max_age_ms: Option<u32>,
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsVoiceAgentConfig {
    pub vad: Option<JsVadConfig>,
    pub events: Option<JsEventsConfig>,
    pub stt: Option<JsSttConfig>,
    pub tts: Option<JsTtsConfig>,
    pub language_id: Option<JsLanguageIdConfig>,
    pub post_utterance_silence_ms: Option<u32>,
    pub noise_suppression: Option<JsNoiseSuppressionConfig>,
    pub replay: Option<JsReplayConfig>,
}

impl TryFrom<JsVoiceAgentConfig> for VoiceAgentConfig {
    type Error = Error;

    fn try_from(value: JsVoiceAgentConfig) -> Result<Self> {
        let post_utterance_silence_ms = value.post_utterance_silence_ms.or_else(|| {
            value
                .tts
                .as_ref()
                .and_then(|tts| tts.post_utterance_silence_ms)
        });
        Ok(Self {
            vad: value.vad.map(Into::into).unwrap_or_default(),
            events: value.events.map(Into::into).unwrap_or_default(),
            stt: value.stt.map(TryInto::try_into).transpose()?,
            tts: value.tts.map(TryInto::try_into).transpose()?,
            language_id: value.language_id.map(Into::into),
            post_utterance_silence_ms,
            noise_suppression: value.noise_suppression.map(Into::into).unwrap_or_default(),
            replay: value
                .replay
                .map(|r| node_webrtc_rust_speech::stt_hold::ReplayConfig {
                    max_age_ms: r.max_age_ms,
                })
                .unwrap_or_default(),
        })
    }
}

#[napi(string_enum)]
#[derive(Debug, PartialEq)]
pub enum JsSpeechEventType {
    #[napi(value = "user_speaking_start")]
    UserSpeakingStart,
    #[napi(value = "user_speaking_end")]
    UserSpeakingEnd,
    #[napi(value = "user_speech_partial")]
    UserSpeechPartial,
    #[napi(value = "user_speech_final")]
    UserSpeechFinal,
    #[napi(value = "user_language")]
    UserLanguage,
    #[napi(value = "language_id_skipped")]
    LanguageIdSkipped,
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
    #[napi(value = "stt_config_updated")]
    SttConfigUpdated,
    #[napi(value = "tts_config_updated")]
    TtsConfigUpdated,
    #[napi(value = "stt_hold_started")]
    SttHoldStarted,
    #[napi(value = "stt_hold_ended")]
    SttHoldEnded,
    #[napi(value = "tts_wait")]
    TtsWait,
    #[napi(value = "tts_phrase_cache_hit")]
    TtsPhraseCacheHit,
    #[napi(value = "agent_speak_failed")]
    AgentSpeakFailed,
    #[napi(value = "agent_reply_start")]
    AgentReplyStart,
    #[napi(value = "agent_reply_end")]
    AgentReplyEnd,
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSpeechEvent {
    pub event_type: JsSpeechEventType,
    pub text: Option<String>,
    pub language: Option<String>,
    pub error: Option<String>,
    pub utterance_id: Option<String>,
    pub replay: Option<bool>,
    pub replaces_utterance_id: Option<String>,
    pub language_mismatch: Option<bool>,
    pub voice: Option<String>,
    pub model_path: Option<String>,
    pub endpoint: Option<String>,
    pub hold_mode: Option<String>,
    pub hold_outcome: Option<String>,
    pub buffered_ms: Option<u32>,
    pub dropped_ms: Option<u32>,
    pub reason: Option<String>,
    pub speech_ms: Option<u32>,
    /// `agent_speaking_start`: ms from the speak request to the first PCM chunk from the TTS vendor.
    pub first_chunk_ms: Option<u32>,
    /// `agent_speaking_start`: ms from the speak request to the first outbound PCM frame of that reply.
    pub first_audio_ms: Option<u32>,
    /// `agent_speaking_start`: letters capitalized at the TTS engine input for this reply (sentenceCase); absent when the TTS vendor does not apply it.
    pub sentence_case_adjusted: Option<u32>,
    /// `tts_wait`: ms the synthesis start waited on refusals.
    pub wait_ms: Option<u32>,
    /// `tts_wait`: refused attempts that were retried.
    pub attempts: Option<u32>,
    /// `tts_phrase_cache_hit`: chars (Unicode scalars) of the text served from the cache.
    pub chars: Option<u32>,
    /// `agent_reply_start` / `agent_reply_end`, and `tts_phrase_cache_hit` / `tts_wait` / `agent_speak_failed` of a reply sent with a `replyId`.
    pub reply_id: Option<String>,
    /// `agent_reply_end`: ms of the reply's audio written to the outbound track.
    pub played_ms: Option<u32>,
    /// `agent_reply_end`: ms of the reply's audio queued for synthesis so far.
    pub synthesized_ms: Option<u32>,
    /// `agent_reply_end`: no more audio would have been queued for the reply.
    pub synthesis_complete: Option<bool>,
    /// `agent_reply_end`: true when the reply was cut off before its last frame was written.
    pub interrupted: Option<bool>,
}

#[napi(string_enum)]
#[derive(Debug)]
pub enum JsSttHoldMode {
    #[napi(value = "buffer_replay")]
    BufferReplay,
    #[napi(value = "first_utterance")]
    FirstUtterance,
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsBeginSttHoldOptions {
    pub mode: JsSttHoldMode,
    pub max_buffer_ms: Option<u32>,
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsReleaseSttHoldOptions {
    pub replay: bool,
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsUpdateTtsOptions {
    pub cancel_inflight: Option<bool>,
}

pub fn speech_err(err: node_webrtc_rust_speech::SpeechError) -> Error {
    Error::from_reason(err.to_string())
}

#[cfg(test)]
mod vad_config_defaults_tests {
    use super::*;

    #[test]
    fn omitted_end_of_turn_fields_fall_back_to_1100_and_400() {
        let cfg: VadConfig = JsVadConfig::default().into();
        assert_eq!(cfg.min_silence_duration_ms, 1100);
        assert_eq!(cfg.stt_gate_hold_ms, 400);
    }

    #[test]
    fn binding_fallbacks_match_the_core_vad_defaults() {
        let cfg: VadConfig = JsVadConfig::default().into();
        let core = VadConfig::default();
        assert_eq!(cfg.min_silence_duration_ms, core.min_silence_duration_ms);
        assert_eq!(cfg.stt_gate_hold_ms, core.stt_gate_hold_ms);
    }

    #[test]
    fn explicit_values_override_the_fallbacks() {
        let cfg: VadConfig = JsVadConfig {
            min_silence_duration_ms: Some(1300),
            stt_gate_hold_ms: Some(1000),
            ..Default::default()
        }
        .into();
        assert_eq!(cfg.min_silence_duration_ms, 1300);
        assert_eq!(cfg.stt_gate_hold_ms, 1000);
    }
}
