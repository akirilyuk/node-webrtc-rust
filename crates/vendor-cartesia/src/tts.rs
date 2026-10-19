use async_trait::async_trait;
use node_webrtc_rust_speech::config::{SpeechTransport, SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

use crate::client::CartesiaClient;
use crate::matrix::{resolve_tts_transport, TtsDeliveryPlan, DEFAULT_MODEL_ID};

pub struct CartesiaTts {
    client: CartesiaClient,
    model: String,
    voice: Option<String>,
    /// Resolved customer transport: `Websocket` = WS with the no-audio REST fallback,
    /// `Http` = REST only.
    transport: SpeechTransport,
}

impl CartesiaTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        let transport = resolve_tts_transport(config.transport)?;
        Ok(Self {
            client: CartesiaClient::with_endpoint(
                config
                    .api_key
                    .clone()
                    .or_else(|| std::env::var("CARTESIA_API_KEY").ok()),
                config.endpoint.clone(),
            )?,
            model: config
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL_ID.to_string()),
            voice: config.voice.clone(),
            transport,
        })
    }

    /// `tts.voice`, else `CARTESIA_VOICE_ID`. Cartesia has no default voice.
    fn voice_id(&self) -> SpeechResult<String> {
        resolve_voice(
            self.voice.as_deref(),
            std::env::var("CARTESIA_VOICE_ID").ok(),
        )
    }
}

/// The voice id from the config, else the environment value. Neither set (or empty) is a config
/// error: Cartesia documents no default voice.
fn resolve_voice(configured: Option<&str>, env: Option<String>) -> SpeechResult<String> {
    configured
        .map(str::to_string)
        .filter(|voice| !voice.is_empty())
        .or_else(|| env.filter(|voice| !voice.is_empty()))
        .ok_or_else(|| {
            SpeechError::Config("set tts.voice or CARTESIA_VOICE_ID to a Cartesia voice id".into())
        })
}

pub fn unsupported_stt(
    _config: &SttConfig,
) -> SpeechResult<Box<dyn node_webrtc_rust_speech::pipeline::SttProvider>> {
    Err(SpeechError::Config("Cartesia does not provide STT".into()))
}

#[async_trait]
impl TtsProvider for CartesiaTts {
    fn vendor_name(&self) -> &'static str {
        "cartesia"
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
        let voice = self.voice_id()?;
        if self.transport == SpeechTransport::Websocket
            && crate::matrix::tts_delivery_plan(&self.model) == TtsDeliveryPlan::WebSocketContexts
        {
            #[cfg(feature = "live")]
            match self
                .client
                .synthesize_websocket(text, &voice, &self.model, sink.clone())
                .await
            {
                // WS already sent every chunk to the sink; never re-send.
                Ok(outcome) => return Ok(vec![chunk_from_mono(&outcome.mono)]),
                // Audio already played: falling back would play the utterance again.
                Err(failure) if failure.emitted_any => return Err(failure.error),
                // Nothing reached the sink: the streamed REST body below.
                Err(_) => {}
            }
        }
        // The REST body goes to the sink as it arrives, each chunk once.
        let mono = self
            .client
            .synthesize_text(text, &voice, &self.model, sink.as_ref())
            .await?;
        Ok(vec![chunk_from_mono(&mono)])
    }
}

fn chunk_from_mono(mono: &[u8]) -> TtsAudioChunk {
    TtsAudioChunk {
        pcm: mono_s16le_to_stereo(mono),
        duration_ms: duration_ms_from_mono_s16le(mono.len(), WEBRTC_PCM_SAMPLE_RATE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_comes_from_config_then_env() {
        assert_eq!(resolve_voice(Some("a"), Some("b".into())).unwrap(), "a");
        assert_eq!(resolve_voice(None, Some("b".into())).unwrap(), "b");
        assert_eq!(resolve_voice(Some(""), Some("b".into())).unwrap(), "b");
    }

    #[test]
    fn no_voice_anywhere_is_a_config_error_naming_the_variable() {
        for (configured, env) in [(None, None), (Some(""), Some(String::new()))] {
            match resolve_voice(configured, env) {
                Err(SpeechError::Config(message)) => {
                    assert!(message.contains("CARTESIA_VOICE_ID"), "{message}");
                }
                other => panic!("{other:?}"),
            }
        }
    }
}
