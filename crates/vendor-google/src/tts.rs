use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::SpeechResult;
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
    /// One REST client for the provider's life: connections (and TLS sessions) are reused.
    #[cfg(feature = "live")]
    http: reqwest::Client,
    /// The gRPC client (StreamingSynthesize), built on first use and kept.
    #[cfg(feature = "live")]
    grpc: tokio::sync::OnceCell<google_cloud_texttospeech_v1::client::TextToSpeech>,
    #[cfg(feature = "live")]
    rest_url_override: Option<String>,
    last_transport: std::sync::Mutex<Option<String>>,
}

impl GoogleTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        crate::matrix::check_tts_transport(config.transport)?;
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
            #[cfg(feature = "live")]
            http: crate::http::build_client(None)?,
            #[cfg(feature = "live")]
            grpc: tokio::sync::OnceCell::new(),
            #[cfg(feature = "live")]
            rest_url_override: None,
            last_transport: std::sync::Mutex::new(None),
        })
    }

    /// Test hook: REST `text:synthesize` goes to this URL (plain `http://` mock server) with a
    /// fixed bearer token instead of Google credentials.
    #[cfg(feature = "live")]
    #[doc(hidden)]
    pub fn __with_rest_url_override(mut self, url: impl Into<String>) -> Self {
        self.rest_url_override = Some(url.into());
        self
    }

    /// Test hook: replaces the pooled REST client with one that has this whole-request bound.
    #[cfg(feature = "live")]
    #[doc(hidden)]
    pub fn __with_total_timeout(mut self, total: std::time::Duration) -> Self {
        self.http = crate::http::build_client(Some(total)).expect("test http client");
        self
    }
}

#[async_trait]
impl TtsProvider for GoogleTts {
    fn vendor_name(&self) -> &'static str {
        "google"
    }

    fn last_transport(&self) -> Option<String> {
        self.last_transport.lock().ok().and_then(|g| g.clone())
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
                let client = self
                    .grpc
                    .get_or_try_init(tts_live::build_grpc_client)
                    .await?;
                return tts_live::streaming_synthesize(
                    client,
                    &self.last_transport,
                    &self.voice,
                    &self.language,
                    text,
                    sink,
                )
                .await;
            }
            let mono = tts_live::synthesize_linear16(
                &self.http,
                &self.last_transport,
                self.rest_url_override.as_deref(),
                text,
                &self.voice,
                &self.language,
            )
            .await?;
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
            Err(node_webrtc_rust_speech::error::SpeechError::Vendor {
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
            sentence_case: Default::default(),
            transport: None,
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
