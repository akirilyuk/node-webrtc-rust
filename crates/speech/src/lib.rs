//! Voice agent orchestration for STT/TTS pipelines.

pub mod agent;
pub mod config;
pub mod error;
pub mod events;
pub mod pipeline;
pub mod registry;
pub mod tts_buffer;

pub use agent::{PcmReader, PcmWriter, VoiceAgent};
pub use config::{
    BargeInConfig, EventDeliveryMode, EventsConfig, SttConfig, SttVendor, TtsConfig, TtsVendor,
    VadConfig, VadSampleRate, VoiceAgentConfig,
};
pub use error::{SpeechError, SpeechResult};
pub use events::{SpeechEvent, SpeechEventKind, SpeechEventBus};
pub use pipeline::{SttProvider, SttTranscript, TtsAudioChunk, TtsProvider, VendorFactory};
pub use registry::VendorRegistry;
pub use tts_buffer::TtsBuffer;
