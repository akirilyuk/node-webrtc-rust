use async_trait::async_trait;
use node_webrtc_rust_speech::config::{SpeechTransport, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, WEBRTC_PCM_SAMPLE_RATE};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

use crate::client::{FishAudioClient, StreamOutcome};
use crate::matrix::{resolve_tts_transport, validate_model_id, DEFAULT_MODEL_ID};

pub struct FishAudioTts {
    client: FishAudioClient,
    model: String,
    voice: Option<String>,
    /// Resolved customer transport: `Http` = chunked HTTP with the documented WS fallback,
    /// `Websocket` = socket only, no fallback.
    transport: SpeechTransport,
}

impl FishAudioTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_ID.to_string());
        // Fish silently falls back to `s2.1-pro` for an unknown model, so never send one.
        validate_model_id(&model).map_err(SpeechError::Config)?;
        let transport = resolve_tts_transport(config.transport)?;
        Ok(Self {
            client: FishAudioClient::new(
                config
                    .api_key
                    .clone()
                    .or_else(|| std::env::var("FISH_API_KEY").ok()),
                config.endpoint.clone(),
            )?,
            model,
            voice: config.voice.clone(),
            transport,
        })
    }

    fn reference_id(&self) -> Option<String> {
        resolve_reference_id(
            self.voice.as_deref(),
            std::env::var("FISH_VOICE_ID").ok().as_deref(),
        )
    }
}

/// `None`, `""` and `"default"` mean "no explicit voice": use `FISH_VOICE_ID` when set,
/// otherwise omit `reference_id` so Fish picks the model's default voice.
fn resolve_reference_id(voice: Option<&str>, env_voice: Option<&str>) -> Option<String> {
    match voice {
        Some(voice) if !voice.is_empty() && voice != "default" => Some(voice.to_string()),
        _ => env_voice
            .filter(|voice| !voice.is_empty())
            .map(str::to_string),
    }
}

fn into_chunks(outcome: StreamOutcome) -> Vec<TtsAudioChunk> {
    if outcome.mono_48k_stereo.is_empty() {
        return Vec::new();
    }
    let duration_ms =
        duration_ms_from_mono_s16le(outcome.mono_48k_stereo.len() / 2, WEBRTC_PCM_SAMPLE_RATE);
    vec![TtsAudioChunk {
        pcm: outcome.mono_48k_stereo.into(),
        duration_ms,
    }]
}

/// Keeps the primary transport's error text and appends why the other transport was tried
/// first and abandoned, e.g. `(after http failure: ...)`. Only vendor messages are combined,
/// so request text never ends up in the result.
#[cfg(feature = "live")]
fn with_context(primary: SpeechError, other_label: &str, other: &SpeechError) -> SpeechError {
    match (primary, other) {
        (
            SpeechError::Vendor { vendor, message },
            SpeechError::Vendor {
                message: other_message,
                ..
            },
        ) => SpeechError::Vendor {
            vendor,
            message: format!("{message} (after {other_label} failure: {other_message})"),
        },
        (primary, _) => primary,
    }
}

#[async_trait]
impl TtsProvider for FishAudioTts {
    fn vendor_name(&self) -> &'static str {
        "fishaudio"
    }

    fn last_transport(&self) -> Option<String> {
        self.client.last_transport()
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let reference_id = self.reference_id();
        let reference_id = reference_id.as_deref();

        if self.transport == SpeechTransport::Websocket {
            // Explicit `websocket`: the socket only. No fallback to HTTP, so a failure is the
            // WebSocket's own error and nothing is retried on the other transport.
            let outcome = self
                .client
                .synthesize_ws(text, reference_id, &self.model, sink.as_ref())
                .await
                .map_err(|failure| failure.error)?;
            return Ok(into_chunks(outcome));
        }

        // Chunked HTTP is the default: the agent hands whole sentences, so the WebSocket's
        // text-streaming input buys nothing, and HTTP reached the first audio chunk sooner
        // in live measurements (291-658 ms vs 963-1035 ms, 2026-10-16).
        #[cfg(feature = "live")]
        let outcome = match self
            .client
            .synthesize_http(text, reference_id, &self.model, sink.as_ref())
            .await
        {
            Ok(outcome) => outcome,
            // Documented fallback, only while no audio reached the sink (a retry after audio
            // started would play the start of the utterance twice) and only for failures the
            // other transport can fix: not for a 4xx about the request, key or credit.
            Err(http) if !http.emitted_any && http.allows_ws_fallback() => {
                match self
                    .client
                    .synthesize_ws(text, reference_id, &self.model, sink.as_ref())
                    .await
                {
                    Ok(outcome) => outcome,
                    Err(ws) => return Err(with_context(ws.error, "http", &http.error)),
                }
            }
            Err(http) => return Err(http.error),
        };

        #[cfg(not(feature = "live"))]
        let outcome = self
            .client
            .synthesize_http(text, reference_id, &self.model, sink.as_ref())
            .await
            .map_err(|failure| failure.error)?;

        // The sink already received every chunk once, as it arrived. The return value is the
        // whole utterance for the phrase cache and is never re-sent.
        Ok(into_chunks(outcome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_voice_wins_over_env() {
        assert_eq!(
            resolve_reference_id(Some("v1"), Some("env")).as_deref(),
            Some("v1")
        );
    }

    #[test]
    fn unset_empty_and_default_use_env_then_omit() {
        for voice in [None, Some(""), Some("default")] {
            assert_eq!(
                resolve_reference_id(voice, Some("env")).as_deref(),
                Some("env")
            );
            assert_eq!(resolve_reference_id(voice, Some("")), None);
            assert_eq!(resolve_reference_id(voice, None), None);
        }
    }
}
