use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, WEBRTC_PCM_SAMPLE_RATE};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

use crate::client::{FishAudioClient, StreamOutcome};
use crate::matrix::{validate_model_id, DEFAULT_MODEL_ID};

pub struct FishAudioTts {
    client: FishAudioClient,
    model: String,
    voice: Option<String>,
}

impl FishAudioTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_ID.to_string());
        // Fish silently falls back to `s2.1-pro` for an unknown model, so never send one.
        validate_model_id(&model).map_err(SpeechError::Config)?;
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

/// Keeps the HTTP error text and appends why the WebSocket attempt was abandoned.
#[cfg(feature = "live")]
fn with_ws_context(http: SpeechError, ws: &SpeechError) -> SpeechError {
    match (http, ws) {
        (
            SpeechError::Vendor { vendor, message },
            SpeechError::Vendor {
                message: ws_message,
                ..
            },
        ) => SpeechError::Vendor {
            vendor,
            message: format!("{message} (after ws failure: {ws_message})"),
        },
        (http, _) => http,
    }
}

#[async_trait]
impl TtsProvider for FishAudioTts {
    fn vendor_name(&self) -> &'static str {
        "fishaudio"
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

        #[cfg(feature = "live")]
        let outcome = match self
            .client
            .synthesize_ws(text, reference_id, &self.model, sink.as_ref())
            .await
        {
            Ok(outcome) => outcome,
            // Documented fallback, only while no audio reached the sink: a retry after audio
            // started would play the start of the utterance twice.
            Err(ws) if !ws.emitted_any => {
                match self
                    .client
                    .synthesize_http(text, reference_id, &self.model, sink.as_ref())
                    .await
                {
                    Ok(outcome) => outcome,
                    Err(http) => return Err(with_ws_context(http.error, &ws.error)),
                }
            }
            Err(ws) => return Err(ws.error),
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
