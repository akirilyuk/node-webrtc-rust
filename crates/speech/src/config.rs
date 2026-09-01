//! Voice agent configuration types.

use serde::{Deserialize, Serialize};

/// How speech events are delivered to the host application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventDeliveryMode {
    Callback,
    Stream,
    Both,
}

impl Default for EventDeliveryMode {
    fn default() -> Self {
        Self::Both
    }
}

/// Event delivery configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsConfig {
    #[serde(default)]
    pub mode: EventDeliveryMode,
}

impl Default for EventsConfig {
    fn default() -> Self {
        Self {
            mode: EventDeliveryMode::Both,
        }
    }
}

/// Barge-in: stop agent TTS and emit `barge_in`.
///
/// - `enabled` — master switch for flush + `barge_in` event.
/// - `use_vad` (default true) — when true, inbound VAD `SpeechStart` triggers barge-in
///   (`vad.enabled` must be true on the same agent). When false, only `flushTts()` from
///   your app triggers barge-in (no automatic interrupt on noise or test tones).
/// - `flush_tts` — clear pending outbound PCM when barge-in runs.
/// - `require_stt_partial` (default **true**) — while agent TTS is playing, defer barge-in until
///   STT returns a non-empty partial (semantic interrupt). Coughs and tones open the STT gate via
///   VAD but typically produce no transcript, so playback continues. Requires `stt` on the agent;
///   when STT is disabled, VAD barge-in behaves as if this flag were false.
/// - `min_stt_partial_tokens` — minimum whitespace-separated tokens (with ≥1 alphanumeric) in the
///   partial before barge (default **2**). Rejects mid-word fragments like `"St"` and single-token
///   noise. Legacy JSON/JS key `minSttPartialChars` maps to this same token count.
///
/// `agent_playback_guard_ms` — optional: for this many ms after agent TTS starts, VAD barge-in
/// does not flush playback (mitigates speaker→mic echo on some setups). Default **0** = barge
/// anytime the user speaks. Raise only if echo falsely interrupts agent TTS (try headphones first).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BargeInConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub use_vad: bool,
    #[serde(default = "default_true")]
    pub flush_tts: bool,
    #[serde(default = "default_true")]
    pub require_stt_partial: bool,
    /// Minimum STT partial token count to barge when `require_stt_partial` is true.
    ///
    /// Serde also accepts legacy `minSttPartialChars` / `min_stt_partial_chars` as this field.
    #[serde(
        default = "default_min_stt_partial_tokens",
        alias = "minSttPartialChars",
        alias = "min_stt_partial_chars"
    )]
    pub min_stt_partial_tokens: u32,
    #[serde(default = "default_agent_playback_guard_ms")]
    pub agent_playback_guard_ms: u32,
}

fn default_min_stt_partial_tokens() -> u32 {
    2
}

fn default_agent_playback_guard_ms() -> u32 {
    0
}

impl Default for BargeInConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            use_vad: true,
            flush_tts: true,
            require_stt_partial: true,
            min_stt_partial_tokens: default_min_stt_partial_tokens(),
            agent_playback_guard_ms: default_agent_playback_guard_ms(),
        }
    }
}

/// Prefer `minSttPartialTokens`; fall back to legacy `minSttPartialChars` (same numeric meaning:
/// token count, not character length). Default **2**.
pub fn resolve_min_stt_partial_tokens(tokens: Option<u32>, legacy_chars: Option<u32>) -> u32 {
    tokens
        .or(legacy_chars)
        .unwrap_or_else(default_min_stt_partial_tokens)
        .max(1)
}

/// Count whitespace-separated tokens that contain at least one alphanumeric character.
pub fn stt_partial_token_count(text: &str) -> usize {
    text.split_whitespace()
        .filter(|tok| tok.chars().any(|c| c.is_alphanumeric()))
        .count()
}

#[cfg(test)]
mod min_stt_partial_tokens_tests {
    use super::*;

    #[test]
    fn resolve_prefers_tokens_over_legacy_chars() {
        assert_eq!(
            resolve_min_stt_partial_tokens(Some(2), Some(9)),
            2,
            "explicit tokens must win over legacy chars"
        );
    }

    #[test]
    fn resolve_legacy_chars_maps_to_tokens() {
        assert_eq!(
            resolve_min_stt_partial_tokens(None, Some(3)),
            3,
            "legacy minSttPartialChars must set minSttPartialTokens"
        );
    }

    #[test]
    fn resolve_default_is_two_tokens() {
        assert_eq!(resolve_min_stt_partial_tokens(None, None), 2);
    }

    #[test]
    fn serde_legacy_min_stt_partial_chars_alias() {
        let cfg: BargeInConfig =
            serde_json::from_str(r#"{"minSttPartialChars":4}"#).expect("deserialize");
        assert_eq!(cfg.min_stt_partial_tokens, 4);
    }

    #[test]
    fn token_count_ignores_punctuation_only_and_mid_word_fragments() {
        assert_eq!(stt_partial_token_count("St"), 1);
        assert_eq!(stt_partial_token_count("stop now"), 2);
        assert_eq!(stt_partial_token_count("Stop now, please"), 3);
        assert_eq!(stt_partial_token_count("  ...  "), 0);
    }
}

fn default_true() -> bool {
    true
}

/// Inbound noise suppression backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoiseSuppressionProvider {
    None,
    Rnnoise,
}

impl Default for NoiseSuppressionProvider {
    fn default() -> Self {
        Self::None
    }
}

/// Inbound PCM noise suppression (before VAD / downsample).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoiseSuppressionConfig {
    #[serde(default)]
    pub provider: NoiseSuppressionProvider,
}

impl Default for NoiseSuppressionConfig {
    fn default() -> Self {
        Self {
            provider: NoiseSuppressionProvider::None,
        }
    }
}

/// Supported VAD sample rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VadSampleRate {
    #[serde(rename = "8000")]
    Hz8000 = 8000,
    #[serde(rename = "16000")]
    Hz16000 = 16000,
}

impl VadSampleRate {
    pub fn as_u32(self) -> u32 {
        match self {
            Self::Hz8000 => 8000,
            Self::Hz16000 => 16000,
        }
    }
}

impl Default for VadSampleRate {
    fn default() -> Self {
        Self::Hz16000
    }
}

/// Voice activity detection configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VadConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `"energy"` (RMS, shipped in default native build) or `"silero"` (needs `silero-vad` feature).
    #[serde(default = "default_vad_provider")]
    pub provider: String,
    /// Energy: RMS level (~0.05–0.2 typical). Silero: speech probability (~0.5 typical).
    #[serde(default = "default_vad_threshold")]
    pub threshold: f32,
    #[serde(default = "default_min_speech_ms")]
    pub min_speech_duration_ms: u32,
    #[serde(default = "default_min_silence_ms")]
    pub min_silence_duration_ms: u32,
    /// Extra mono PCM retained in the STT pre-roll ring (added to `min_speech_duration_ms` for capacity).
    #[serde(default = "default_speech_pad_ms")]
    pub speech_pad_ms: u32,
    #[serde(default)]
    pub sample_rate: VadSampleRate,
    #[serde(default)]
    pub barge_in: BargeInConfig,
    /// When true, STT receives audio only while the gate is open (see VoiceAgent): VAD
    /// speaking, optional pending speech, or post-speech hold.
    #[serde(default)]
    pub gate_stt: bool,
    /// When `gate_stt` is true, also feed STT during VAD pending speech (before `SpeechStart`).
    #[serde(default = "default_true")]
    pub gate_stt_open_on_pending: bool,
    /// After VAD speech end, keep passing inbound audio to STT for this long (trailing phonemes / word gaps).
    /// With `gate_stt`, `user_speaking_end` is emitted when this hold expires (default 1000 ms).
    #[serde(default = "default_stt_gate_hold_ms")]
    pub stt_gate_hold_ms: u32,
    /// After `vad_triggered`, emit `user_stt_not_found` when no STT partial arrives within this window (default 4000 ms).
    #[serde(default = "default_stt_listen_timeout_ms")]
    pub stt_listen_timeout_ms: u32,
    /// Grace after the last partial or VAD `SpeechEnd` before forcing `user_speech_final` (default 1500 ms).
    #[serde(default = "default_utterance_finalize_timeout_ms")]
    pub utterance_finalize_timeout_ms: u32,
}

fn default_vad_provider() -> String {
    "energy".to_string()
}

fn default_vad_threshold() -> f32 {
    0.15
}

fn default_min_speech_ms() -> u32 {
    250
}

// Brief gaps inside a phrase: ~1.3 s silence before "maybe done", then sttGateHold grace.
fn default_min_silence_ms() -> u32 {
    1300
}

fn default_speech_pad_ms() -> u32 {
    300
}

fn default_stt_gate_hold_ms() -> u32 {
    1000
}

fn default_stt_listen_timeout_ms() -> u32 {
    4000
}

fn default_utterance_finalize_timeout_ms() -> u32 {
    1500
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: default_vad_provider(),
            threshold: default_vad_threshold(),
            min_speech_duration_ms: default_min_speech_ms(),
            min_silence_duration_ms: default_min_silence_ms(),
            speech_pad_ms: default_speech_pad_ms(),
            sample_rate: VadSampleRate::default(),
            barge_in: BargeInConfig::default(),
            gate_stt: false,
            gate_stt_open_on_pending: true,
            stt_gate_hold_ms: default_stt_gate_hold_ms(),
            stt_listen_timeout_ms: default_stt_listen_timeout_ms(),
            utterance_finalize_timeout_ms: default_utterance_finalize_timeout_ms(),
        }
    }
}

/// Supported STT vendor identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SttVendor {
    Openai,
    Deepgram,
    Google,
    Assemblyai,
    #[serde(rename = "local-sherpa")]
    LocalSherpa,
    Mock,
}

impl SttVendor {
    /// Serde wire name (matches JSON / SDK `provider` values).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Deepgram => "deepgram",
            Self::Google => "google",
            Self::Assemblyai => "assemblyai",
            Self::LocalSherpa => "local-sherpa",
            Self::Mock => "mock",
        }
    }
}

/// Supported TTS vendor identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsVendor {
    Openai,
    Elevenlabs,
    Google,
    Cartesia,
    #[serde(rename = "local-sherpa")]
    LocalSherpa,
    Mock,
}

impl TtsVendor {
    /// Serde wire name (matches JSON / SDK `provider` values).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Elevenlabs => "elevenlabs",
            Self::Google => "google",
            Self::Cartesia => "cartesia",
            Self::LocalSherpa => "local-sherpa",
            Self::Mock => "mock",
        }
    }
}

/// STT vendor configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SttConfig {
    pub provider: SttVendor,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_path: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

/// TTS vendor configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsConfig {
    pub provider: TtsVendor,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_path: Option<String>,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Resolved post-TTS silence duration for outbound pacing.
pub fn resolved_post_utterance_silence_ms(config: &VoiceAgentConfig) -> u32 {
    let explicit = config.post_utterance_silence_ms;
    match explicit {
        Some(0) => 0,
        Some(ms) => ms,
        None => config
            .vad
            .stt_gate_hold_ms
            .saturating_add(config.vad.min_silence_duration_ms)
            .saturating_add(250),
    }
}

/// Full voice agent configuration (mirrored in TypeScript `VoiceAgentConfig`).
///
/// Defaults include mock STT/TTS for unit tests; production apps set real vendors via NAPI/TS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceAgentConfig {
    #[serde(default)]
    pub vad: VadConfig,
    #[serde(default)]
    pub noise_suppression: NoiseSuppressionConfig,
    #[serde(default)]
    pub events: EventsConfig,
    pub stt: Option<SttConfig>,
    pub tts: Option<TtsConfig>,
    /// Trailing outbound silence (ms) after TTS. Also accepted under `tts.postUtteranceSilenceMs` in deploy JSON.
    #[serde(default)]
    pub post_utterance_silence_ms: Option<u32>,
}

/// Session-scoped observability attributes and W3C trace propagation.
///
/// Passed to [`crate::VoiceAgent::start`] so voice spans join an upstream trace when
/// `traceparent` is set (standard W3C `traceparent` header value).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSessionContext {
    pub session_id: Option<String>,
    pub trace_id: Option<String>,
    pub project_id: Option<String>,
    pub org_id: Option<String>,
    pub build_id: Option<String>,
    /// W3C `traceparent` header (e.g. `00-<trace-id>-<span-id>-01`).
    pub traceparent: Option<String>,
}

/// Options for [`crate::VoiceAgent::send_text_to_tts_with_options`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTextToTtsOptions {
    /// When true, return as soon as the utterance is queued. When false (default), await synthesis
    /// and outbound playback for this call before resolving.
    #[serde(default)]
    pub non_blocking: bool,
}

impl Default for SendTextToTtsOptions {
    fn default() -> Self {
        Self {
            non_blocking: false,
        }
    }
}

impl Default for VoiceAgentConfig {
    fn default() -> Self {
        Self {
            vad: VadConfig::default(),
            noise_suppression: NoiseSuppressionConfig::default(),
            events: EventsConfig::default(),
            stt: Some(SttConfig {
                provider: SttVendor::Mock,
                model: None,
                model_path: None,
                language: Some("en".to_string()),
                api_key: None,
            }),
            tts: Some(TtsConfig {
                provider: TtsVendor::Mock,
                model: None,
                model_path: None,
                voice: None,
                api_key: None,
            }),
            post_utterance_silence_ms: None,
        }
    }
}
