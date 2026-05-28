use async_trait::async_trait;
#[cfg(feature = "live")]
use async_openai::types::{AudioInput, CreateTranscriptionRequestArgs, InputSource};
#[cfg(feature = "live")]
use async_openai::Client;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
#[cfg(feature = "live")]
use node_webrtc_rust_speech::pcm::{mono16_le_to_wav, STT_MIN_BATCH_BYTES, STT_PREFERRED_BATCH_BYTES};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};

use crate::factory::{api_key_from, OpenAiSttState, SharedSttState};

pub struct OpenAiStt {
    api_key: Option<String>,
    model: String,
    language: Option<String>,
    state: SharedSttState,
}

impl OpenAiStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        let api_key = config.api_key.clone().or_else(|| std::env::var("OPENAI_API_KEY").ok());
        Ok(Self {
            api_key,
            model: config
                .model
                .clone()
                .unwrap_or_else(|| "whisper-1".to_string()),
            language: config.language.clone(),
            state: std::sync::Arc::new(tokio::sync::Mutex::new(OpenAiSttState {
                buffered: Vec::new(),
                running: false,
                pending: None,
            })),
        })
    }
}

#[async_trait]
impl SttProvider for OpenAiStt {
    fn vendor_name(&self) -> &'static str {
        "openai"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        let mut state = self.state.lock().await;
        state.running = true;
        state.buffered.clear();
        state.pending = None;
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let mut state = self.state.lock().await;
        state.running = false;
        state.buffered.clear();
        state.pending = None;
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let mut state = self.state.lock().await;
        if state.running {
            state.buffered.extend_from_slice(pcm.as_ref());
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        #[cfg(feature = "live")]
        {
            if let Some(pending) = self.state.lock().await.pending.take() {
                return Ok(Some(pending));
            }

            let (pcm, api_key, model, language) = {
                let mut state = self.state.lock().await;
                if !state.running || state.buffered.len() < STT_MIN_BATCH_BYTES {
                    return Ok(None);
                }
                if state.buffered.len() < STT_PREFERRED_BATCH_BYTES {
                    return Ok(None);
                }
                let pcm = Bytes::from(std::mem::take(&mut state.buffered));
                let api_key = api_key_from(&self.api_key, "OPENAI_API_KEY")?;
                (pcm, api_key, self.model.clone(), self.language.clone())
            };

            let transcript = live_transcribe(pcm, &api_key, &model, language.as_deref()).await?;
            return Ok(Some(transcript));
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = api_key_from(&self.api_key, "OPENAI_API_KEY");
            Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: "live OpenAI STT requires `--features live` on vendor-openai".into(),
            })
        }
    }
}

#[cfg(feature = "live")]
async fn live_transcribe(
    pcm: Bytes,
    api_key: &str,
    model: &str,
    language: Option<&str>,
) -> SpeechResult<SttTranscript> {
    let wav = mono16_le_to_wav(pcm.as_ref());
    let mut builder = CreateTranscriptionRequestArgs::default();
    builder.model(model);
    if let Some(language) = language {
        builder.language(language);
    }
    let request = builder
        .file(AudioInput {
            source: InputSource::Bytes {
                filename: "audio.wav".into(),
                bytes: wav.into(),
            },
        })
        .build()
        .map_err(|err| SpeechError::Vendor {
            vendor: "openai".into(),
            message: err.to_string(),
        })?;

    let client = Client::with_config(async_openai::config::OpenAIConfig::new().with_api_key(api_key));
    let response = client.audio().transcribe(request).await.map_err(|err| SpeechError::Vendor {
        vendor: "openai".into(),
        message: err.to_string(),
    })?;

    let text = response.text.trim();
    if text.is_empty() {
        return Ok(SttTranscript::Final(String::new()));
    }
    Ok(SttTranscript::Final(text.to_string()))
}
