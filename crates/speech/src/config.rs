//! Voice agent configuration types.

use serde::{Deserialize, Serialize};

/// How speech events are delivered to the host application.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventDeliveryMode {
    Callback,
    Stream,
    #[default]
    Both,
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoiseSuppressionProvider {
    #[default]
    None,
    Rnnoise,
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

/// What the speaker gate does with a final whose speaker is not the enrolled caller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpeakerMismatchAction {
    /// Emit the final with `speaker: "other"`.
    #[default]
    Flag,
    /// Emit `user_speech_ignored` (`reason: "speaker_mismatch"`) instead of the final.
    Drop,
}

/// Opt-in caller verification. The caller's voice is learned from their first utterances and every
/// later utterance is checked against it. Embeddings live in process memory only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpeakerGateConfig {
    pub enabled: bool,
    /// Speaker embedding model (`nemo_en_titanet_small.onnx`). Required when `enabled`.
    pub model_path: Option<String>,
    /// Cosine similarity at or above which an utterance is the caller; in (0, 1).
    pub threshold: f32,
    /// Voiced speech (ms) collected for enrollment.
    pub enroll_speech_ms: u32,
    /// Speech (ms) of an utterance used for one check.
    pub check_speech_ms: u32,
    /// Clips with less speech than this (ms) are not scored: the verdict is `unknown` and the gate
    /// fails open (unless `suppress_short_barge_in`). Default 1500. Must not exceed
    /// `check_speech_ms` (`validate` rejects it).
    ///
    /// Measured 2026-10-30 (TitaNet-small, threshold 0.459, `speaker_bench` Part 1 at a fixed
    /// threshold, 128 genuine / 92 impostor clips per set, bystander at -6/-10/-15/-20 dB pooled):
    ///
    /// | check | set  | FRR    | FAR   |
    /// |-------|------|--------|-------|
    /// | 1.0 s | test | 11.7 % | 0.0 % |
    /// | 1.0 s | tune | 18.0 % | 1.1 % |
    /// | 1.25 s| test | 3.1 %  | 0.0 % |
    /// | 1.25 s| tune | 7.8 %  | 1.1 % |
    /// | 1.5 s | test | 0.8 %  | 0.0 % |
    /// | 1.5 s | tune | 1.6 %  | 2.2 % |
    ///
    /// At 1.25 s per bystander level, test (tune) FRR: -6 dB 9.4 % (21.9 %), -10 dB 3.1 % (9.4 %),
    /// -15 and -20 dB 0 % (0 %). Full reasoning: `packages/sdk/VOICE-VAD-AND-BARGE-IN.md`.
    pub min_check_speech_ms: u32,
    /// Opt-in. While an utterance is live and has no verdict, a barge-in is deferred. When the
    /// utterance ends with less speech than `min_check_speech_ms`, the verdict is `unknown` and
    /// the deferred barge-in is normally released. With this set, it is suppressed instead, like
    /// `other`, so a short bystander phrase cannot interrupt the agent. Short caller phrases
    /// stop interrupting too (they are still transcribed). Every other `unknown` (not enrolled,
    /// embed timeout or error) still proceeds. Default false.
    pub suppress_short_barge_in: bool,
    /// When the first check of an utterance says `other` and the utterance keeps going, score the
    /// next `check_speech_ms` windows of speech, up to `recheck_max_windows`. A `caller` re-check
    /// releases the held barge-in and labels the final `caller` (`speakerRechecked: true`); any
    /// other result keeps `other`. It never runs after a `caller` or `unknown` verdict. Covers a
    /// bystander who opens the VAD before the caller and runs into the caller's words. Default
    /// true; false is the one-check behaviour of 0.14.
    pub recheck_on_other: bool,
    /// Windows scored per utterance, the first check included, while the verdict stays `other`
    /// (`recheck_on_other`). `2` is the one re-check of 0.15.0 (same window count and barge bound; a
    /// late first verdict re-checks the latest window, not the one right after the first); `0` keeps checking until the
    /// final; `1` is rejected (use `recheck_on_other: false`). Default 8 (12 s of speech with the
    /// default 1.5 s check).
    pub recheck_max_windows: u32,
    pub on_mismatch: SpeakerMismatchAction,
    /// Bound (ms) on one embedding call and on a final waiting for its verdict.
    pub verify_timeout_ms: u32,
}

impl Default for SpeakerGateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model_path: None,
            threshold: 0.459,
            enroll_speech_ms: 3000,
            check_speech_ms: 1500,
            min_check_speech_ms: 1500,
            suppress_short_barge_in: false,
            recheck_on_other: true,
            recheck_max_windows: crate::speaker_gate::DEFAULT_RECHECK_MAX_WINDOWS,
            on_mismatch: SpeakerMismatchAction::Flag,
            verify_timeout_ms: 500,
        }
    }
}

impl SpeakerGateConfig {
    /// Config errors for an enabled gate.
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if self
            .model_path
            .as_deref()
            .is_none_or(|p| p.trim().is_empty())
        {
            return Err("speakerGate.modelPath is required when speakerGate.enabled".into());
        }
        if !(self.threshold > 0.0 && self.threshold < 1.0) {
            return Err(format!(
                "speakerGate.threshold must be in (0, 1), got {}",
                self.threshold
            ));
        }
        if self.check_speech_ms < self.min_check_speech_ms {
            return Err(format!(
                "speakerGate.checkSpeechMs ({}) must be at least speakerGate.minCheckSpeechMs ({}): \
                 otherwise every utterance is shorter than the minimum and is never checked",
                self.check_speech_ms, self.min_check_speech_ms
            ));
        }
        if self.recheck_on_other && self.recheck_max_windows == 1 {
            return Err(
                "speakerGate.recheckMaxWindows must be 0 (no cap) or at least 2; use \
                 speakerGate.recheckOnOther: false for one check"
                    .into(),
            );
        }
        Ok(())
    }
}

/// Supported VAD sample rates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VadSampleRate {
    #[serde(rename = "8000")]
    Hz8000 = 8000,
    #[serde(rename = "16000")]
    #[default]
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

/// Voice activity detection configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VadConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `"energy"` (RMS, default), `"adaptive"` (energy that follows the noise floor and the
    /// caller level, see [`AdaptiveVadConfig`]) or `"silero"` (needs the `silero-vad` feature;
    /// treats continuous background voices as speech and does not end turns in babble, so it is
    /// not recommended for noisy calls and is not in the default build).
    #[serde(default = "default_vad_provider")]
    pub provider: String,
    /// Energy: RMS level (~0.05–0.2 typical). Silero: speech probability (~0.5 typical).
    /// Adaptive: only the warm-up threshold (first `adaptive.warmupMs`).
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
    /// When STT decode backlog exceeds this slack (ms), defer C1 until backlog drains (default 200).
    #[serde(default = "default_stt_listen_backlog_slack_ms")]
    pub stt_listen_backlog_slack_ms: u32,
    /// Hard cap on C1 wait when decode backlog is high; `0` = 3 × [`Self::stt_listen_timeout_ms`].
    #[serde(default)]
    pub stt_listen_hard_timeout_ms: u32,
    /// Max time C1 keeps waiting while the STT stream open is refused and retried
    /// (`SttProvider::stream_open_pending`). 0 = no extra wait (old behaviour).
    #[serde(default = "default_stt_open_wait_max_ms")]
    pub stt_open_wait_max_ms: u32,
    /// Grace after the last partial or VAD `SpeechEnd` before forcing `user_speech_final` (default 1500 ms).
    #[serde(default = "default_utterance_finalize_timeout_ms")]
    pub utterance_finalize_timeout_ms: u32,
    /// Parameters of the `"adaptive"` provider; ignored by the others.
    #[serde(default)]
    pub adaptive: AdaptiveVadConfig,
}

/// Parameters of the `"adaptive"` VAD provider. Defaults are provisional until the bench picks them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptiveVadConfig {
    /// Start of the call, in ms, where the plain energy rule (`vad.threshold`) decides.
    #[serde(default = "default_adaptive_warmup_ms")]
    pub warmup_ms: u32,
    /// Length of the window of inactive frames the noise level is read from.
    #[serde(default = "default_adaptive_noise_window_ms")]
    pub noise_window_ms: u32,
    /// Percentile (1..=99, nearest rank) of the window that is taken as the noise level.
    #[serde(default = "default_adaptive_noise_percentile")]
    pub noise_percentile: u32,
    /// Speech must be this many dB above the noise level.
    #[serde(default = "default_adaptive_noise_margin_db")]
    pub noise_margin_db: f32,
    /// Also raise the threshold to halfway between noise and the caller's level.
    #[serde(default = "default_true")]
    pub use_speech_level: bool,
    /// Once speaking, stay active while the level is within this many dB below the threshold.
    #[serde(default = "default_adaptive_hysteresis_db")]
    pub hysteresis_db: f32,
    /// Lower clamp of the threshold, dBFS.
    #[serde(default = "default_adaptive_min_threshold_dbfs")]
    pub min_threshold_dbfs: f32,
    /// Upper clamp of the threshold, dBFS.
    #[serde(default = "default_adaptive_max_threshold_dbfs")]
    pub max_threshold_dbfs: f32,
    /// If an utterance stays open longer than `stuckActiveMs`, its active frames also feed the noise window until the utterance ends, so a level step in the background noise cannot hold the VAD open.
    #[serde(default = "default_adaptive_stuck_active_ms")]
    pub stuck_active_ms: u32,
}

fn default_adaptive_warmup_ms() -> u32 {
    400
}
fn default_adaptive_noise_window_ms() -> u32 {
    3000
}
fn default_adaptive_noise_percentile() -> u32 {
    70
}
fn default_adaptive_noise_margin_db() -> f32 {
    6.0
}
fn default_adaptive_hysteresis_db() -> f32 {
    3.0
}
fn default_adaptive_min_threshold_dbfs() -> f32 {
    -50.0
}
fn default_adaptive_max_threshold_dbfs() -> f32 {
    -10.0
}
fn default_adaptive_stuck_active_ms() -> u32 {
    8000
}

impl Default for AdaptiveVadConfig {
    fn default() -> Self {
        Self {
            warmup_ms: default_adaptive_warmup_ms(),
            noise_window_ms: default_adaptive_noise_window_ms(),
            noise_percentile: default_adaptive_noise_percentile(),
            noise_margin_db: default_adaptive_noise_margin_db(),
            use_speech_level: true,
            hysteresis_db: default_adaptive_hysteresis_db(),
            min_threshold_dbfs: default_adaptive_min_threshold_dbfs(),
            max_threshold_dbfs: default_adaptive_max_threshold_dbfs(),
            stuck_active_ms: default_adaptive_stuck_active_ms(),
        }
    }
}

impl AdaptiveVadConfig {
    /// Reject out-of-range values; errors name the camelCase field.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=99).contains(&self.noise_percentile) {
            return Err("vad.adaptive.noisePercentile must be 1..=99".into());
        }
        if self.noise_window_ms < 500 {
            return Err("vad.adaptive.noiseWindowMs must be >= 500".into());
        }
        if self.warmup_ms > self.noise_window_ms {
            return Err("vad.adaptive.warmupMs must be <= noiseWindowMs".into());
        }
        if !(0.0..=30.0).contains(&self.noise_margin_db) {
            return Err("vad.adaptive.noiseMarginDb must be 0..=30".into());
        }
        if !(0.0..=30.0).contains(&self.hysteresis_db) {
            return Err("vad.adaptive.hysteresisDb must be 0..=30".into());
        }
        if self
            .min_threshold_dbfs
            .partial_cmp(&self.max_threshold_dbfs)
            != Some(std::cmp::Ordering::Less)
        {
            return Err("vad.adaptive.minThresholdDbfs must be < maxThresholdDbfs".into());
        }
        if self.max_threshold_dbfs > 0.0 {
            return Err("vad.adaptive.maxThresholdDbfs must be <= 0".into());
        }
        if self.stuck_active_ms < 1000 {
            return Err("vad.adaptive.stuckActiveMs must be >= 1000".into());
        }
        Ok(())
    }
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

// Brief gaps inside a phrase: ~1.1 s silence before "maybe done", then sttGateHold grace.
fn default_min_silence_ms() -> u32 {
    1100
}

fn default_speech_pad_ms() -> u32 {
    300
}

fn default_stt_gate_hold_ms() -> u32 {
    400
}

fn default_stt_listen_timeout_ms() -> u32 {
    4000
}

fn default_stt_open_wait_max_ms() -> u32 {
    120_000
}

fn default_stt_listen_backlog_slack_ms() -> u32 {
    200
}

/// Resolved C1 hard timeout: explicit `stt_listen_hard_timeout_ms` or 3 × `stt_listen_timeout_ms`.
pub fn effective_stt_listen_hard_timeout_ms(vad: &VadConfig) -> u64 {
    if vad.stt_listen_hard_timeout_ms > 0 {
        vad.stt_listen_hard_timeout_ms as u64
    } else {
        vad.stt_listen_timeout_ms as u64 * 3
    }
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
            stt_listen_backlog_slack_ms: default_stt_listen_backlog_slack_ms(),
            stt_listen_hard_timeout_ms: 0,
            stt_open_wait_max_ms: default_stt_open_wait_max_ms(),
            utterance_finalize_timeout_ms: default_utterance_finalize_timeout_ms(),
            adaptive: AdaptiveVadConfig::default(),
        }
    }
}

/// Supported STT vendor identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SttVendor {
    Openai,
    Deepgram,
    Elevenlabs,
    Fishaudio,
    Mistral,
    Google,
    Assemblyai,
    Groq,
    Azure,
    Aws,
    #[serde(rename = "local-sherpa")]
    LocalSherpa,
    /// In-cluster Sherpa transport. `@internal` — not a public SDK vendor.
    #[serde(rename = "cluster-sherpa")]
    ClusterSherpa,
    Mock,
}

impl SttVendor {
    /// Serde wire name (matches JSON / SDK `provider` values).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Deepgram => "deepgram",
            Self::Elevenlabs => "elevenlabs",
            Self::Fishaudio => "fishaudio",
            Self::Mistral => "mistral",
            Self::Google => "google",
            Self::Assemblyai => "assemblyai",
            Self::Groq => "groq",
            Self::Azure => "azure",
            Self::Aws => "aws",
            Self::LocalSherpa => "local-sherpa",
            Self::ClusterSherpa => "cluster-sherpa",
            Self::Mock => "mock",
        }
    }
}

/// Supported TTS vendor identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsVendor {
    Openai,
    Deepgram,
    Elevenlabs,
    Google,
    Cartesia,
    Fishaudio,
    Mistral,
    Groq,
    Azure,
    Aws,
    #[serde(rename = "local-sherpa")]
    LocalSherpa,
    /// In-cluster Sherpa transport. `@internal` — not a public SDK vendor.
    #[serde(rename = "cluster-sherpa")]
    ClusterSherpa,
    Mock,
}

impl TtsVendor {
    /// Serde wire name (matches JSON / SDK `provider` values).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Deepgram => "deepgram",
            Self::Elevenlabs => "elevenlabs",
            Self::Google => "google",
            Self::Cartesia => "cartesia",
            Self::Fishaudio => "fishaudio",
            Self::Mistral => "mistral",
            Self::Groq => "groq",
            Self::Azure => "azure",
            Self::Aws => "aws",
            Self::LocalSherpa => "local-sherpa",
            Self::ClusterSherpa => "cluster-sherpa",
            Self::Mock => "mock",
        }
    }
}

/// Transport a cloud speech vendor uses to reach its API.
///
/// Customers pick one per leg (STT / TTS) where the vendor offers more than one. The allowed
/// values and the value `auto` resolves to come from the voice catalog
/// (`packages/voice-catalog/catalog/vendors.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechTransport {
    /// The vendor default from the voice catalog (`defaultTtsTransport` / `defaultSttTransport`).
    Auto,
    /// A persistent WebSocket session.
    Websocket,
    /// Streamed HTTP: HTTP/2 when the vendor negotiates it, else HTTP/1.1.
    Http,
}

impl SpeechTransport {
    /// Lowercase wire / catalog value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Websocket => "websocket",
            Self::Http => "http",
        }
    }

    /// Parse a catalog / wire value (`"auto"`, `"websocket"`, `"http"`).
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "websocket" => Some(Self::Websocket),
            "http" => Some(Self::Http),
            _ => None,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Transport to the vendor API. `None` / `auto` = the catalog default. See [`SpeechTransport`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<SpeechTransport>,
}

/// Whether the TTS engine input gets sentence-start capitalization.
///
/// STT finals often start lowercase (`Okay. eight one`). The Piper/VITS engine (espeak-ng)
/// does not treat `. e` as a sentence end, so it renders one sentence with a late first
/// chunk and no pause.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SentenceCase {
    /// The first letter after `. `, `! `, `? ` is uppercased in the text handed to the TTS
    /// engine only; the caller's text is unchanged everywhere else. Not after a single letter
    /// (`I.`) and not before a fragment of fewer than 2 words, which stay merged for espeak.
    #[default]
    Auto,
    /// The engine gets the text as given.
    Off,
}

impl SentenceCase {
    /// Wire value used by the cluster speech protocol (`"auto"` | `"off"`).
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
        }
    }

    /// Parse the wire value: `"off"` is [`SentenceCase::Off`], anything else (including an
    /// empty string from older clients) is [`SentenceCase::Auto`].
    pub fn from_wire(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("off") {
            Self::Off
        } else {
            Self::Auto
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Transport to the vendor API. `None` / `auto` = the catalog default. See [`SpeechTransport`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<SpeechTransport>,
    /// Sentence-start capitalization of the text handed to the TTS engine. See [`SentenceCase`].
    #[serde(default)]
    pub sentence_case: SentenceCase,
}

/// Options for [`crate::agent::VoiceAgent::update_tts_config`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpdateTtsConfigOptions {
    /// When true, cancel in-flight synthesis and flush outbound TTS before applying config.
    pub cancel_inflight: bool,
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

/// When spoken-language ID runs relative to the utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageIdTiming {
    /// One identify at utterance close (default).
    EndOfUtterance,
    /// One identify as soon as `min_speech_ms` is buffered during the utterance.
    Early,
    /// Repeated identifies during a long utterance.
    Continuous,
}

impl LanguageIdTiming {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "end_of_utterance" => Some(Self::EndOfUtterance),
            "early" => Some(Self::Early),
            "continuous" => Some(Self::Continuous),
            _ => None,
        }
    }
}

/// Spoken language identification (offline clip, e.g. Sherpa Whisper tiny).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageIdConfig {
    /// When unset, LID is on when `model_path` is non-empty. Set `false` to disable explicitly.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Directory with Whisper encoder/decoder ONNX for `SpokenLanguageIdentification`.
    #[serde(default)]
    pub model_path: Option<String>,
    /// Optional ISO 639-1 allowlist; other detected codes are ignored.
    #[serde(default)]
    pub allowlist: Option<Vec<String>>,
    /// Minimum buffered speech (ms) before identify runs at VAD `SpeechEnd` (default mode)
    /// or before the first mid-utterance identify when `continuous` is true. Default 1000.
    #[serde(default)]
    pub min_speech_ms: Option<u32>,
    /// When `Some(true)`, start identify after ~`min_speech_ms` of speech while the user is
    /// still speaking (re-run after each pass completes during a long utterance).
    /// Default (unset/false): buffer inbound PCM during the utterance; identify at VAD
    /// `SpeechEnd` when buffered speech reaches `min_speech_ms` (STT close window gates final).
    #[serde(default)]
    pub continuous: Option<bool>,
    /// When to run language ID: `end_of_utterance` (default), `early` (one identify as soon as
    /// `min_speech_ms` is buffered, never again for that utterance) or `continuous` (repeated
    /// passes). Takes precedence over the deprecated `continuous` flag.
    #[serde(default)]
    pub timing: Option<LanguageIdTiming>,
    /// Maximum PCM clip (ms) fed to the identifier per identify pass. Default 5000.
    #[serde(default)]
    pub lid_max_clip_ms: Option<u32>,
    /// Fault-path bound (ms) when gating `user_speech_final` on a hung in-flight identify.
    /// Default 3000. Must not fire under normal mock/test identifiers.
    #[serde(default)]
    pub lid_gate_max_wait_ms: Option<u32>,
    /// When `Some(true)`, defer LID while local TTS is active, gate `user_speech_final` on
    /// in-flight LID, and make the TTS synthesis worker wait for LID (local Whisper vs local
    /// Piper CPU conflict). When `Some(false)`, LID still starts at VAD `SpeechEnd` but the
    /// final and TTS are never delayed; `user_language` arrives when identify completes.
    /// Default (unset): enabled only when `tts.provider` is `LocalSherpa`; remote/streaming
    /// TTS vendors must not be delayed by LID.
    #[serde(default)]
    pub tts_exclusion: Option<bool>,
}

fn default_language_id_min_speech_ms() -> u32 {
    1000
}

fn default_lid_max_clip_ms() -> u32 {
    5000
}

fn default_lid_gate_max_wait_ms() -> u32 {
    3000
}

pub fn resolved_lid_max_clip_ms(config: &LanguageIdConfig) -> u32 {
    config
        .lid_max_clip_ms
        .unwrap_or_else(default_lid_max_clip_ms)
        .max(1)
}

pub fn resolved_lid_gate_max_wait_ms(config: &LanguageIdConfig) -> u32 {
    config
        .lid_gate_max_wait_ms
        .unwrap_or_else(default_lid_gate_max_wait_ms)
        .max(1)
}

/// True when LID must not overlap local TTS synthesis (gate final, defer at active TTS, TTS worker wait).
pub fn lid_tts_exclusion_enabled(
    language_id: &Option<LanguageIdConfig>,
    tts: &Option<TtsConfig>,
) -> bool {
    if let Some(lid) = language_id {
        if let Some(explicit) = lid.tts_exclusion {
            return explicit;
        }
    }
    tts.as_ref()
        .is_some_and(|cfg| cfg.provider == TtsVendor::LocalSherpa)
}

/// True when `language_id` should run (non-empty `model_path` and not explicitly disabled).
pub fn language_id_enabled(config: &Option<LanguageIdConfig>) -> bool {
    match config {
        None => false,
        Some(cfg) => {
            if cfg.enabled == Some(false) {
                return false;
            }
            cfg.model_path
                .as_ref()
                .is_some_and(|path| !path.trim().is_empty())
        }
    }
}

pub fn resolved_language_id_min_speech_ms(config: &LanguageIdConfig) -> u32 {
    config
        .min_speech_ms
        .unwrap_or_else(default_language_id_min_speech_ms)
        .max(1)
}

/// Effective timing: explicit `timing` wins; else `continuous: true` means `Continuous`;
/// else `EndOfUtterance`.
pub fn language_id_timing(config: &Option<LanguageIdConfig>) -> LanguageIdTiming {
    match config.as_ref() {
        Some(cfg) => match (cfg.timing, cfg.continuous) {
            (Some(t), _) => t,
            (None, Some(true)) => LanguageIdTiming::Continuous,
            _ => LanguageIdTiming::EndOfUtterance,
        },
        None => LanguageIdTiming::EndOfUtterance,
    }
}

/// True when the effective timing is `continuous` (explicit, or the `continuous: true` alias).
pub fn language_id_continuous(config: &Option<LanguageIdConfig>) -> bool {
    language_id_timing(config) == LanguageIdTiming::Continuous
}

/// True when the effective timing is `early`.
pub fn language_id_early(config: &Option<LanguageIdConfig>) -> bool {
    language_id_timing(config) == LanguageIdTiming::Early
}

/// Returns true when `language` passes the optional allowlist (case-insensitive ISO 639-1).
pub fn language_id_allowlist_accepts(config: &LanguageIdConfig, language: &str) -> bool {
    match &config.allowlist {
        None => true,
        Some(list) if list.is_empty() => true,
        Some(list) => {
            let normalized = language.trim().to_ascii_lowercase();
            list.iter()
                .any(|code| code.trim().eq_ignore_ascii_case(&normalized))
        }
    }
}

#[cfg(test)]
mod language_id_config_tests {
    use super::*;

    #[test]
    fn disabled_without_model_path() {
        assert!(!language_id_enabled(&None));
        assert!(!language_id_enabled(&Some(LanguageIdConfig {
            enabled: None,
            model_path: None,
            allowlist: None,
            min_speech_ms: None,
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            timing: None,
            tts_exclusion: None,
        })));
    }

    #[test]
    fn enabled_with_model_path() {
        assert!(language_id_enabled(&Some(LanguageIdConfig {
            enabled: None,
            model_path: Some("/models/whisper-tiny".into()),
            allowlist: None,
            min_speech_ms: None,
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            timing: None,
            tts_exclusion: None,
        })));
    }

    #[test]
    fn explicit_enabled_false_disables_even_with_path() {
        assert!(!language_id_enabled(&Some(LanguageIdConfig {
            enabled: Some(false),
            model_path: Some("/models/whisper-tiny".into()),
            allowlist: None,
            min_speech_ms: None,
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            timing: None,
            tts_exclusion: None,
        })));
    }

    #[test]
    fn allowlist_filters_codes() {
        let cfg = LanguageIdConfig {
            enabled: None,
            model_path: Some("/x".into()),
            allowlist: Some(vec!["en".into(), "de".into()]),
            min_speech_ms: None,
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            timing: None,
            tts_exclusion: None,
        };
        assert!(language_id_allowlist_accepts(&cfg, "en"));
        assert!(language_id_allowlist_accepts(&cfg, "DE"));
        assert!(!language_id_allowlist_accepts(&cfg, "fr"));
    }

    #[test]
    fn min_speech_ms_defaults_to_1000() {
        let cfg = LanguageIdConfig {
            enabled: None,
            model_path: None,
            allowlist: None,
            min_speech_ms: None,
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            timing: None,
            tts_exclusion: None,
        };
        assert_eq!(resolved_language_id_min_speech_ms(&cfg), 1000);
    }

    #[test]
    fn continuous_defaults_to_false() {
        assert!(!language_id_continuous(&None));
        assert!(!language_id_continuous(&Some(LanguageIdConfig {
            enabled: None,
            model_path: Some("/x".into()),
            allowlist: None,
            min_speech_ms: None,
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            timing: None,
            tts_exclusion: None,
        })));
        assert!(!language_id_continuous(&Some(LanguageIdConfig {
            enabled: None,
            model_path: Some("/x".into()),
            allowlist: None,
            min_speech_ms: None,
            continuous: Some(false),
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            timing: None,
            tts_exclusion: None,
        })));
    }

    #[test]
    fn continuous_true_only_when_explicit() {
        assert!(language_id_continuous(&Some(LanguageIdConfig {
            enabled: None,
            model_path: Some("/x".into()),
            allowlist: None,
            min_speech_ms: None,
            continuous: Some(true),
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            timing: None,
            tts_exclusion: None,
        })));
    }

    #[test]
    fn tts_exclusion_defaults_from_tts_vendor() {
        let lid = LanguageIdConfig {
            enabled: Some(true),
            model_path: Some("/x".into()),
            allowlist: None,
            min_speech_ms: None,
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            timing: None,
            tts_exclusion: None,
        };
        let local = Some(TtsConfig {
            provider: TtsVendor::LocalSherpa,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
            sentence_case: Default::default(),
            transport: None,
        });
        let mock = Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
            sentence_case: Default::default(),
            transport: None,
        });
        let cluster = Some(TtsConfig {
            provider: TtsVendor::ClusterSherpa,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
            sentence_case: Default::default(),
            transport: None,
        });
        assert!(lid_tts_exclusion_enabled(&Some(lid.clone()), &local));
        assert!(!lid_tts_exclusion_enabled(&Some(lid.clone()), &mock));
        // Remote cluster TTS must not be gated by local LID (only LocalSherpa enables it).
        assert!(!lid_tts_exclusion_enabled(&Some(lid.clone()), &cluster));
        assert!(!lid_tts_exclusion_enabled(
            &Some(LanguageIdConfig {
                tts_exclusion: Some(false),
                ..lid.clone()
            }),
            &local,
        ));
        assert!(lid_tts_exclusion_enabled(
            &Some(LanguageIdConfig {
                tts_exclusion: Some(true),
                ..lid
            }),
            &mock,
        ));
    }
    #[test]
    fn timing_resolves_with_continuous_alias() {
        let mk = |timing, continuous| {
            Some(LanguageIdConfig {
                enabled: None,
                model_path: None,
                allowlist: None,
                min_speech_ms: None,
                continuous,
                lid_max_clip_ms: None,
                lid_gate_max_wait_ms: None,
                timing,
                tts_exclusion: None,
            })
        };
        assert_eq!(language_id_timing(&None), LanguageIdTiming::EndOfUtterance);
        assert_eq!(
            language_id_timing(&mk(None, None)),
            LanguageIdTiming::EndOfUtterance
        );
        assert_eq!(
            language_id_timing(&mk(None, Some(true))),
            LanguageIdTiming::Continuous
        );
        assert_eq!(
            language_id_timing(&mk(Some(LanguageIdTiming::Early), None)),
            LanguageIdTiming::Early
        );
        // Explicit timing wins over the deprecated flag.
        assert_eq!(
            language_id_timing(&mk(Some(LanguageIdTiming::EndOfUtterance), Some(true))),
            LanguageIdTiming::EndOfUtterance
        );
        assert!(language_id_early(&mk(Some(LanguageIdTiming::Early), None)));
        assert_eq!(
            LanguageIdTiming::parse("early"),
            Some(LanguageIdTiming::Early)
        );
        assert_eq!(LanguageIdTiming::parse("bogus"), None);
    }
}

#[cfg(test)]
mod vendor_wire_tests {
    use super::*;

    #[test]
    fn tts_config_sentence_case_defaults_to_auto() {
        let cfg: TtsConfig = serde_json::from_str(r#"{"provider":"mock"}"#).unwrap();
        assert_eq!(cfg.sentence_case, SentenceCase::Auto);
    }

    #[test]
    fn tts_config_sentence_case_off_deserializes() {
        let cfg: TtsConfig =
            serde_json::from_str(r#"{"provider":"mock","sentenceCase":"off"}"#).unwrap();
        assert_eq!(cfg.sentence_case, SentenceCase::Off);
    }

    #[test]
    fn tts_config_sentence_case_bogus_is_rejected() {
        let err =
            serde_json::from_str::<TtsConfig>(r#"{"provider":"mock","sentenceCase":"bogus"}"#);
        assert!(err.is_err());
    }

    #[test]
    fn stt_tts_endpoint_serde_omits_when_none() {
        let stt = SttConfig {
            provider: SttVendor::ClusterSherpa,
            model: None,
            model_path: Some("/models/sherpa/stt/en".into()),
            language: Some("en".into()),
            api_key: None,
            endpoint: None,
            transport: None,
        };
        let json = serde_json::to_string(&stt).unwrap();
        assert!(!json.contains("endpoint"));
        let with_ep = SttConfig {
            endpoint: Some("http://speech-stt:50051".into()),
            ..stt.clone()
        };
        let parsed: SttConfig =
            serde_json::from_str(&serde_json::to_string(&with_ep).unwrap()).unwrap();
        assert_eq!(parsed.endpoint.as_deref(), Some("http://speech-stt:50051"));
    }

    #[test]
    fn cluster_sherpa_serde_round_trip() {
        for (json, stt) in [
            ("\"cluster-sherpa\"", SttVendor::ClusterSherpa),
            ("\"local-sherpa\"", SttVendor::LocalSherpa),
        ] {
            let parsed: SttVendor = serde_json::from_str(json).unwrap();
            assert_eq!(parsed, stt);
            assert_eq!(parsed.as_str(), json.trim_matches('"'));
            assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
        }
        for (json, tts) in [
            ("\"cluster-sherpa\"", TtsVendor::ClusterSherpa),
            ("\"local-sherpa\"", TtsVendor::LocalSherpa),
        ] {
            let parsed: TtsVendor = serde_json::from_str(json).unwrap();
            assert_eq!(parsed, tts);
            assert_eq!(parsed.as_str(), json.trim_matches('"'));
            assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
        }
    }

    #[test]
    fn fishaudio_tts_vendor_serde_round_trip() {
        let parsed: TtsVendor = serde_json::from_str("\"fishaudio\"").unwrap();
        assert_eq!(parsed, TtsVendor::Fishaudio);
        assert_eq!(parsed.as_str(), "fishaudio");
        assert_eq!(serde_json::to_string(&parsed).unwrap(), "\"fishaudio\"");
    }

    #[test]
    fn fishaudio_stt_vendor_serde_round_trip() {
        let parsed: SttVendor = serde_json::from_str("\"fishaudio\"").unwrap();
        assert_eq!(parsed, SttVendor::Fishaudio);
        assert_eq!(parsed.as_str(), "fishaudio");
        assert_eq!(serde_json::to_string(&parsed).unwrap(), "\"fishaudio\"");
    }

    #[test]
    fn mistral_tts_vendor_serde_round_trip() {
        let parsed: TtsVendor = serde_json::from_str("\"mistral\"").unwrap();
        assert_eq!(parsed, TtsVendor::Mistral);
        assert_eq!(parsed.as_str(), "mistral");
        assert_eq!(serde_json::to_string(&parsed).unwrap(), "\"mistral\"");
    }

    #[test]
    fn mistral_stt_vendor_serde_round_trip() {
        let parsed: SttVendor = serde_json::from_str("\"mistral\"").unwrap();
        assert_eq!(parsed, SttVendor::Mistral);
        assert_eq!(parsed.as_str(), "mistral");
        assert_eq!(serde_json::to_string(&parsed).unwrap(), "\"mistral\"");
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
    /// Opt-in caller verification (barge-in and finals).
    #[serde(default)]
    pub speaker_gate: SpeakerGateConfig,
    #[serde(default)]
    pub events: EventsConfig,
    pub stt: Option<SttConfig>,
    pub tts: Option<TtsConfig>,
    #[serde(default)]
    pub language_id: Option<LanguageIdConfig>,
    /// Trailing outbound silence (ms) after TTS. Also accepted under `tts.postUtteranceSilenceMs` in deploy JSON.
    #[serde(default)]
    pub post_utterance_silence_ms: Option<u32>,
    /// `replay_last_utterance` tuning (`replay.maxAgeMs`).
    #[serde(default)]
    pub replay: crate::stt_hold::ReplayConfig,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTextToTtsOptions {
    /// When true, return as soon as the utterance is queued. When false (default), await synthesis
    /// and outbound playback for this call before resolving.
    #[serde(default)]
    pub non_blocking: bool,
    /// When false, VAD and STT-partial barge-in do not flush or cancel this utterance while it
    /// is synthesizing or playing. Default true. Explicit `flush_tts` / `stop` still cancel it.
    #[serde(default = "default_send_text_interruptible")]
    pub interruptible: bool,
    /// Tags this reply's events: `agent_reply_start` / `agent_reply_end`, and
    /// `tts_phrase_cache_hit` / `tts_wait` / `agent_speak_failed`. Without it none of these
    /// events carry a reply id and no reply events are emitted.
    #[serde(default)]
    pub reply_id: Option<String>,
}

fn default_send_text_interruptible() -> bool {
    true
}

impl Default for SendTextToTtsOptions {
    fn default() -> Self {
        Self {
            non_blocking: false,
            interruptible: true,
            reply_id: None,
        }
    }
}

impl Default for VoiceAgentConfig {
    fn default() -> Self {
        Self {
            vad: VadConfig::default(),
            noise_suppression: NoiseSuppressionConfig::default(),
            speaker_gate: SpeakerGateConfig::default(),
            events: EventsConfig::default(),
            stt: Some(SttConfig {
                provider: SttVendor::Mock,
                model: None,
                model_path: None,
                language: Some("en".to_string()),
                api_key: None,
                endpoint: None,
                transport: None,
            }),
            tts: Some(TtsConfig {
                provider: TtsVendor::Mock,
                model: None,
                model_path: None,
                voice: None,
                api_key: None,
                endpoint: None,
                sentence_case: Default::default(),
                transport: None,
            }),
            language_id: None,
            post_utterance_silence_ms: None,
            replay: crate::stt_hold::ReplayConfig::default(),
        }
    }
}
