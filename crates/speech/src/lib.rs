//! # node-webrtc-rust-speech
//!
//! Voice agent orchestration: VAD, STT/TTS vendor adapters, barge-in, and speech events
//! for one WebRTC peer connection.
//!
//! ## TypeScript SDK
//!
//! Application code should use [`@node-webrtc-rust/sdk/voice`](../../packages/sdk/VOICE-API.md)
//! (`VoiceAgent`, presets, `speechEvents()`). NAPI bindings mirror the config types in
//! [`config`] and drive [`agent::VoiceAgent`].
//!
//! ## Core flow
//!
//! 1. **Inbound** — WebRTC stereo 48 kHz PCM → mono 16 kHz → optional VAD → optional STT gate → STT.
//! 2. **Outbound** — TTS synthesize → buffer → 20 ms frames to PCM writer → `agent_speaking_*` events.
//! 3. **Barge-in** — During agent TTS the inbound track is always processed (VAD every frame).
//!    STT is fed only when VAD opens the gate (pending/speaking/hold). Semantic barge (`require_stt_partial`)
//!    flushes TTS on a qualifying STT partial; VAD-only barge applies when STT is disabled.
//!
//! ## `gate_stt` utterance close
//!
//! When [`VadConfig::gate_stt`] is true (recommended preset in TS: `VOICE_AGENT_VAD_PRESET`):
//!
//! - STT receives audio only while the gate is open (speech, pending, hold, or closing).
//! - After VAD `SpeechEnd`, [`VadConfig::stt_gate_hold_ms`] keeps the gate open for word gaps.
//! - When hold expires, an endpoint tail (400–600 ms) is pushed to STT, then `finalize_utterance`.
//! - `user_speaking_end` is emitted **with** `user_speech_final`, not on the first short pause.
//!
//! See [VOICE-VAD-AND-BARGE-IN.md](../../packages/sdk/VOICE-VAD-AND-BARGE-IN.md) for tuning.
//!
//! ## Modules
//!
//! | Module | Role |
//! | ------ | ---- |
//! | [`agent`] | [`VoiceAgent`] session: attach, start, `process_inbound_pcm`, TTS/STT |
//! | [`config`] | `VadConfig`, `BargeInConfig`, `VoiceAgentConfig`, vendor enums |
//! | [`events`] | [`SpeechEvent`], [`SpeechEventBus`] |
//! | [`vad`] | [`VadEngine`], [`VadTransition`], barge-in handler |
//! | [`stt_pre_roll`] | Pre-roll ring when `gate_stt` is enabled |
//! | [`pipeline`] | `SttProvider` / `TtsProvider` traits and vendor factory |
//! | [`pcm`] | Resample and duration helpers |
//! | [`tts_buffer`] | Outbound TTS PCM queue |
//!
//! Debug: set env `VOICE_DEBUG=1` for stderr `[voice-debug]` lines.

pub mod agent;
pub mod config;
pub mod error;
pub mod events;
pub mod pcm;
pub mod pipeline;
pub mod registry;
pub mod stt_pre_roll;
pub mod tts_buffer;
pub mod vad;

pub use agent::{PcmReader, PcmWriter, VoiceAgent};
pub use config::{
    BargeInConfig, EventDeliveryMode, EventsConfig, SttConfig, SttVendor, TtsConfig, TtsVendor,
    VadConfig, VadSampleRate, VoiceAgentConfig,
};
pub use error::{SpeechError, SpeechResult};
pub use events::{SpeechEvent, SpeechEventKind, SpeechEventBus};
pub use pcm::{stereo_48k_to_mono_16k, pcm_rms_i16};
pub use pipeline::{SttProvider, SttTranscript, TtsAudioChunk, TtsProvider, VendorFactory};
pub use registry::VendorRegistry;
pub use tts_buffer::TtsBuffer;
pub use vad::{handle_barge_in, VadEngine, VadTransition, VoiceActivityDetector};

/// Library version string (also exposed via NAPI / SDK `version`).
pub fn version() -> &'static str {
    agent::version()
}
