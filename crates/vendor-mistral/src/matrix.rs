//! Mistral Voxtral TTS capability matrix from the published docs.
//!
//! Generate speech: <https://docs.mistral.ai/studio/audio/text-to-speech/generate-speech>
//! Stream speech: <https://docs.mistral.ai/studio/audio/text-to-speech/stream-speech>
//! Voices: <https://docs.mistral.ai/studio/audio/text-to-speech/voices>

use std::time::Duration;

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};

pub const GENERATE_SPEECH_DOC: &str =
    "https://docs.mistral.ai/studio/audio/text-to-speech/generate-speech";
pub const STREAM_SPEECH_DOC: &str =
    "https://docs.mistral.ai/studio/audio/text-to-speech/stream-speech";
pub const VOICES_DOC: &str = "https://docs.mistral.ai/studio/audio/text-to-speech/voices";

/// Default API origin. `TtsConfig.endpoint` replaces it (tests point it at a local mock).
pub const DEFAULT_BASE_URL: &str = "https://api.mistral.ai";

/// Documented model ids and aliases. `voxtral-mini-tts-latest` does not point to v3 yet, so it
/// is not accepted.
pub const DOCUMENTED_TTS_MODELS: &[&str] = &["voxtral-mini-tts-3-0", "voxtral-mini-tts-3"];
pub const DEFAULT_TTS_MODEL: &str = "voxtral-mini-tts-3-0";

/// Voxtral 3 preset voice (Sheila, en-US). Mistral rejects a request without a voice (400).
pub const DEFAULT_TTS_VOICE: &str = "voice_478";

/// Mono output rate stated by the docs; the WAV header is what the decoder trusts.
pub const TTS_SAMPLE_RATE: u32 = 24_000;

/// TCP + TLS connect.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Wait for the response headers and then the first audio delta.
pub const FIRST_AUDIO_TIMEOUT: Duration = Duration::from_secs(15);
/// Wait between body chunks once audio has started.
pub const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole request, headers through `speech.audio.done`.
pub const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long an idle pooled connection is kept for the next request.
pub const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// TCP keepalive on pooled connections.
pub const TCP_KEEPALIVE: Duration = Duration::from_secs(30);

/// Bounds for one synthesis. [`Timeouts::default`] holds the constants above; tests shorten
/// them.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub first_audio: Duration,
    pub chunk_idle: Duration,
    pub total: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: CONNECT_TIMEOUT,
            first_audio: FIRST_AUDIO_TIMEOUT,
            chunk_idle: CHUNK_IDLE_TIMEOUT,
            total: HTTP_TOTAL_TIMEOUT,
        }
    }
}

/// An unknown id is rejected before anything is sent. There is no silent fallback to another
/// model.
pub fn validate_tts_model(model: &str) -> SpeechResult<()> {
    if DOCUMENTED_TTS_MODELS.contains(&model) {
        return Ok(());
    }
    Err(SpeechError::Config(format!(
        "unsupported Mistral TTS model `{model}`; documented models: {}",
        DOCUMENTED_TTS_MODELS.join(", ")
    )))
}

/// `POST {base}/v1/audio/speech`
pub fn tts_url(base: &str) -> String {
    format!("{}/v1/audio/speech", base.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_models_pass_and_latest_is_rejected() {
        for model in DOCUMENTED_TTS_MODELS {
            assert!(validate_tts_model(model).is_ok(), "{model}");
        }
        for model in ["voxtral-mini-tts-latest", "", "voxtral-mini-tts-2", "tts-1"] {
            let err = validate_tts_model(model).unwrap_err().to_string();
            assert!(
                err.contains("voxtral-mini-tts-3-0") && err.contains("voxtral-mini-tts-3"),
                "{err}"
            );
        }
        assert!(DOCUMENTED_TTS_MODELS.contains(&DEFAULT_TTS_MODEL));
    }

    #[test]
    fn url_trims_the_trailing_slash() {
        assert_eq!(
            tts_url("https://api.mistral.ai/"),
            "https://api.mistral.ai/v1/audio/speech"
        );
        assert_eq!(
            tts_url("http://127.0.0.1:9"),
            "http://127.0.0.1:9/v1/audio/speech"
        );
    }
}
