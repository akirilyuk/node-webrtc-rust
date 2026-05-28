//! OpenAI STT/TTS vendor adapter (live API behind `live` feature).

mod factory;
mod stt;
mod tts;

pub use factory::OpenAiFactory;
