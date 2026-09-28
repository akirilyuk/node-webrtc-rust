use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

use crate::matrix::voice_supports_streaming_synthesize;

#[cfg(feature = "live")]
use crate::tts_live;

pub struct GoogleTts {
    voice: String,
    language: String,
}

impl GoogleTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        let voice = config
            .voice
            .clone()
            .or_else(|| config.model.clone())
            .unwrap_or_else(|| "en-US-Neural2-A".to_string());
        let language = voice.split('-').take(2).collect::<Vec<_>>().join("-");
        Ok(Self {
            voice,
            language: if language.is_empty() {
                "en-US".to_string()
            } else {
                language
            },
        })
    }
}

#[async_trait]
impl TtsProvider for GoogleTts {
    fn vendor_name(&self) -> &'static str {
        "google"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            if voice_supports_streaming_synthesize(&self.voice) {
                return tts_live::streaming_synthesize(&self.voice, &self.language, text, sink)
                    .await;
            }
            let mono = tts_live::synthesize_linear16(text, &self.voice, &self.language).await?;
            let chunks = vec![pcm_chunk_from_mono(&mono)];
            if let Some(s) = sink {
                for chunk in &chunks {
                    if !s.send(chunk.clone()) {
                        break;
                    }
                }
            }
            return Ok(chunks);
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = (text, sink, &self.voice);
            Err(SpeechError::Vendor {
                vendor: "google".into(),
                message: "live Google TTS requires `--features live` on vendor-google".into(),
            })
        }
    }
}

fn pcm_chunk_from_mono(mono: &[u8]) -> TtsAudioChunk {
    let duration_ms = duration_ms_from_mono_s16le(mono.len(), WEBRTC_PCM_SAMPLE_RATE);
    let pcm = mono_s16le_to_stereo(mono);
    TtsAudioChunk { pcm, duration_ms }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::TtsVendor;

    #[test]
    fn derives_language_from_voice_name() {
        let tts = GoogleTts::new(&TtsConfig {
            provider: TtsVendor::Google,
            model: None,
            model_path: None,
            voice: Some("en-US-Neural2-A".into()),
            api_key: None,
            endpoint: None,
        })
        .unwrap();
        assert_eq!(tts.language, "en-US");
        assert_eq!(tts.voice, "en-US-Neural2-A");
    }

    #[test]
    fn neural2_not_streaming_voice() {
        assert!(!voice_supports_streaming_synthesize("en-US-Neural2-A"));
    }
}
