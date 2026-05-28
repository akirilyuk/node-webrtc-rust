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

/// Barge-in behavior when inbound speech is detected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BargeInConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub flush_tts: bool,
}

impl Default for BargeInConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            flush_tts: true,
        }
    }
}

fn default_true() -> bool {
    true
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
    #[serde(default = "default_silero_provider")]
    pub provider: String,
    #[serde(default = "default_vad_threshold")]
    pub threshold: f32,
    #[serde(default = "default_min_speech_ms")]
    pub min_speech_duration_ms: u32,
    #[serde(default = "default_min_silence_ms")]
    pub min_silence_duration_ms: u32,
    #[serde(default = "default_speech_pad_ms")]
    pub speech_pad_ms: u32,
    #[serde(default)]
    pub sample_rate: VadSampleRate,
    #[serde(default)]
    pub barge_in: BargeInConfig,
    #[serde(default)]
    pub gate_stt: bool,
}

fn default_silero_provider() -> String {
    "silero".to_string()
}

fn default_vad_threshold() -> f32 {
    0.5
}

fn default_min_speech_ms() -> u32 {
    250
}

fn default_min_silence_ms() -> u32 {
    100
}

fn default_speech_pad_ms() -> u32 {
    30
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: default_silero_provider(),
            threshold: default_vad_threshold(),
            min_speech_duration_ms: default_min_speech_ms(),
            min_silence_duration_ms: default_min_silence_ms(),
            speech_pad_ms: default_speech_pad_ms(),
            sample_rate: VadSampleRate::default(),
            barge_in: BargeInConfig::default(),
            gate_stt: false,
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
    Mock,
}

/// Supported TTS vendor identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsVendor {
    Openai,
    Elevenlabs,
    Google,
    Cartesia,
    Mock,
}

/// STT vendor configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SttConfig {
    pub provider: SttVendor,
    #[serde(default)]
    pub model: Option<String>,
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
    pub voice: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Full voice agent configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceAgentConfig {
    #[serde(default)]
    pub vad: VadConfig,
    #[serde(default)]
    pub events: EventsConfig,
    pub stt: Option<SttConfig>,
    pub tts: Option<TtsConfig>,
}

impl Default for VoiceAgentConfig {
    fn default() -> Self {
        Self {
            vad: VadConfig::default(),
            events: EventsConfig::default(),
            stt: Some(SttConfig {
                provider: SttVendor::Mock,
                model: None,
                language: Some("en".to_string()),
                api_key: None,
            }),
            tts: Some(TtsConfig {
                provider: TtsVendor::Mock,
                model: None,
                voice: None,
                api_key: None,
            }),
        }
    }
}
