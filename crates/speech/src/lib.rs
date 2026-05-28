//! Voice agent orchestration for STT/TTS pipelines.

pub mod agent;
pub mod config;
pub mod error;
pub mod events;
pub mod pcm;
pub mod pipeline;
pub mod registry;
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

pub fn version() -> &'static str {
    agent::version()
}
