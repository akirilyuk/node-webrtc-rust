use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, WEBRTC_PCM_SAMPLE_RATE};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

use crate::client::MistralClient;
use crate::matrix::{
    resolve_tts_transport, validate_tts_model, Timeouts, DEFAULT_TTS_MODEL, DEFAULT_TTS_VOICE,
};

pub struct MistralTts {
    client: MistralClient,
    model: String,
    voice: String,
}

impl MistralTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        Self::with_timeouts(config, Timeouts::default())
    }

    /// Same as [`MistralTts::new`] with explicit bounds (tests shorten them).
    pub fn with_timeouts(config: &TtsConfig, timeouts: Timeouts) -> SpeechResult<Self> {
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_TTS_MODEL.to_string());
        // The only transport is HTTP SSE; any other value fails the config here.
        resolve_tts_transport(config.transport)?;
        // An unknown id is a config error: nothing is sent, nothing falls back to another model.
        validate_tts_model(&model)?;
        Ok(Self {
            client: MistralClient::new(
                config
                    .api_key
                    .clone()
                    .or_else(|| std::env::var("MISTRAL_API_KEY").ok()),
                config.endpoint.clone(),
                timeouts,
            )?,
            model,
            voice: resolve_voice(config.voice.as_deref()),
        })
    }
}

/// Mistral requires a voice. `None`, `""` and `"default"` mean "not chosen": use the catalog
/// default preset.
fn resolve_voice(voice: Option<&str>) -> String {
    match voice {
        Some(voice) if !voice.is_empty() && voice != "default" => voice.to_string(),
        _ => (*DEFAULT_TTS_VOICE).to_string(),
    }
}

#[async_trait]
impl TtsProvider for MistralTts {
    fn vendor_name(&self) -> &'static str {
        "mistral"
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
        let pcm = self
            .client
            .synthesize(text, &self.voice, &self.model, sink.as_ref())
            .await?;
        // The sink already received every byte once, as it arrived. The return value is the
        // whole utterance for the phrase cache and is never re-sent.
        if pcm.is_empty() {
            return Ok(Vec::new());
        }
        let duration_ms = duration_ms_from_mono_s16le(pcm.len() / 2, WEBRTC_PCM_SAMPLE_RATE);
        Ok(vec![TtsAudioChunk {
            pcm: pcm.into(),
            duration_ms,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_empty_and_default_use_the_catalog_voice() {
        for voice in [None, Some(""), Some("default")] {
            assert_eq!(resolve_voice(voice), *DEFAULT_TTS_VOICE);
        }
        assert_eq!(resolve_voice(Some("gb_jane_neutral")), "gb_jane_neutral");
    }
}
