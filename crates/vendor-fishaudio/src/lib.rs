mod client;
mod factory;
mod matrix;
mod pcm;
mod stt;
mod stt_matrix;
mod tts;

pub use factory::FishAudioFactory;
pub use matrix::{
    http_tts_url, implemented_tts_transports, validate_model_id, ws_live_url, CHUNK_IDLE_TIMEOUT,
    CONNECT_TIMEOUT, DEFAULT_BASE_URL, DEFAULT_MODEL_ID, DOCUMENTED_MODELS, DRAMA3_DOC,
    FIRST_AUDIO_TIMEOUT, HTTP_DOC, HTTP_TOTAL_TIMEOUT, LATENCY, PCM_SAMPLE_RATE, REALTIME_DOC,
    WS_DOC,
};
pub use stt::FishAudioStt;
pub use stt_matrix::{
    default_stt_model, implemented_stt_transports, realtime_stt_url, validate_stt_model,
    DEFAULT_STT_MODEL, DOCUMENTED_STT_MODELS, STT_DOC, STT_PCM_SAMPLE_RATE,
    STT_PROACTIVE_RECONNECT,
};

/// Test-only access to the two transports, used by `tests/live_test.rs` to pin which path
/// produced the audio (the provider silently falls back from HTTP to WS). Not public API.
#[cfg(feature = "live")]
#[doc(hidden)]
pub mod __test {
    use node_webrtc_rust_speech::error::SpeechResult;
    use node_webrtc_rust_speech::pipeline::TtsProgressiveSink;

    use crate::client::{FishAudioClient, StreamFailure, StreamOutcome};

    pub struct Client(FishAudioClient);

    fn finish(result: Result<StreamOutcome, StreamFailure>) -> SpeechResult<Vec<u8>> {
        result
            .map(|outcome| outcome.mono_48k_stereo)
            .map_err(|failure| failure.error)
    }

    impl Client {
        pub fn new(api_key: Option<String>, base_url: Option<String>) -> SpeechResult<Self> {
            FishAudioClient::new(api_key, base_url).map(Self)
        }

        /// `wss://…/v1/tts/live` only, no fallback to HTTP. Returns 48 kHz stereo s16le.
        pub async fn synthesize_ws(
            &self,
            text: &str,
            reference_id: Option<&str>,
            model: &str,
            sink: Option<&TtsProgressiveSink>,
        ) -> SpeechResult<Vec<u8>> {
            finish(self.0.synthesize_ws(text, reference_id, model, sink).await)
        }

        /// Chunked `POST /v1/tts` only. Returns 48 kHz stereo s16le.
        pub async fn synthesize_http(
            &self,
            text: &str,
            reference_id: Option<&str>,
            model: &str,
            sink: Option<&TtsProgressiveSink>,
        ) -> SpeechResult<Vec<u8>> {
            finish(
                self.0
                    .synthesize_http(text, reference_id, model, sink)
                    .await,
            )
        }
    }
}
