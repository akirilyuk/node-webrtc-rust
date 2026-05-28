use async_trait::async_trait;
use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};

use crate::client::CartesiaClient;

const DEFAULT_MODEL_ID: &str = "sonic-english";

pub struct CartesiaTts {
    client: CartesiaClient,
    model: String,
    voice: Option<String>,
}

impl CartesiaTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        Ok(Self {
            client: CartesiaClient::new(
                config
                    .api_key
                    .clone()
                    .or_else(|| std::env::var("CARTESIA_API_KEY").ok()),
            ),
            model: config
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL_ID.to_string()),
            voice: config.voice.clone(),
        })
    }

    fn voice_id(&self) -> String {
        self.voice
            .clone()
            .filter(|voice| !voice.is_empty())
            .or_else(|| std::env::var("CARTESIA_VOICE_ID").ok())
            .unwrap_or_else(|| "default".to_string())
    }
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

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        let voice = self.voice_id();
        let mono = self.client.synthesize_text(text, &voice, &self.model).await?;
        let duration_ms = duration_ms_from_mono_s16le(mono.len(), WEBRTC_PCM_SAMPLE_RATE);
        let pcm = mono_s16le_to_stereo(&mono);
        Ok(vec![TtsAudioChunk { pcm, duration_ms }])
    }
}
