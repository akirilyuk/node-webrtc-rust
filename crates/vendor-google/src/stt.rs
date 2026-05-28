use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{STT_MIN_BATCH_BYTES, STT_PREFERRED_BATCH_BYTES};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg(feature = "live")]
use crate::auth::{google_access_token, google_api_key};

pub struct GoogleStt {
    language: String,
    state: Arc<Mutex<GoogleSttState>>,
}

struct GoogleSttState {
    running: bool,
    buffered: Vec<u8>,
}

impl GoogleStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        Ok(Self {
            language: config
                .language
                .clone()
                .unwrap_or_else(|| "en-US".to_string()),
            state: Arc::new(Mutex::new(GoogleSttState {
                running: false,
                buffered: Vec::new(),
            })),
        })
    }
}

#[async_trait]
impl SttProvider for GoogleStt {
    fn vendor_name(&self) -> &'static str {
        "google"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        let mut state = self.state.lock().await;
        state.running = true;
        state.buffered.clear();
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let mut state = self.state.lock().await;
        state.running = false;
        state.buffered.clear();
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
            let (pcm, language) = {
                let mut state = self.state.lock().await;
                if !state.running || state.buffered.len() < STT_MIN_BATCH_BYTES {
                    return Ok(None);
                }
                if state.buffered.len() < STT_PREFERRED_BATCH_BYTES {
                    return Ok(None);
                }
                (
                    Bytes::from(std::mem::take(&mut state.buffered)),
                    self.language.clone(),
                )
            };
            let text = recognize_linear16(pcm, &language).await?;
            if text.trim().is_empty() {
                return Ok(None);
            }
            return Ok(Some(SttTranscript::Final(text)));
        }

        #[cfg(not(feature = "live"))]
        {
            Err(SpeechError::Vendor {
                vendor: "google".into(),
                message: "live Google STT requires `--features live` on vendor-google".into(),
            })
        }
    }
}

#[cfg(feature = "live")]
async fn recognize_linear16(pcm: Bytes, language: &str) -> SpeechResult<String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde_json::json;

    let client = reqwest::Client::new();
    let body = json!({
        "config": {
            "encoding": "LINEAR16",
            "sampleRateHertz": 16000,
            "languageCode": language,
            "enableAutomaticPunctuation": true
        },
        "audio": {
            "content": STANDARD.encode(pcm.as_ref())
        }
    });

    let request = if let Some(api_key) = google_api_key() {
        client.post(format!(
            "https://speech.googleapis.com/v1/speech:recognize?key={api_key}"
        ))
    } else {
        let token = google_access_token()
            .await?
            .ok_or_else(|| SpeechError::Vendor {
                vendor: "google".into(),
                message: "failed to obtain Google access token".into(),
            })?;
        client
            .post("https://speech.googleapis.com/v1/speech:recognize")
            .bearer_auth(token)
    };

    let response = request
        .json(&body)
        .send()
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;

    let status = response.status();
    let payload: serde_json::Value = response.json().await.map_err(|err| SpeechError::Vendor {
        vendor: "google".into(),
        message: err.to_string(),
    })?;

    if !status.is_success() {
        return Err(SpeechError::Vendor {
            vendor: "google".into(),
            message: payload.to_string(),
        });
    }

    let transcript = payload
        .pointer("/results/0/alternatives/0/transcript")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok(transcript)
}
