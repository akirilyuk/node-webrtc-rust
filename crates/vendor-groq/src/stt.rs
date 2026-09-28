//! Groq STT — file multipart transcription on VAD finalize (no documented live socket).
//!
//! <https://console.groq.com/docs/speech-to-text>

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, mono16_le_to_wav, STT_MIN_BATCH_BYTES, STT_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use tokio::sync::{mpsc, Mutex};

use crate::factory::api_key_from;
use crate::matrix::{default_stt_model, parse_transcription_json, validate_stt_model, DEFAULT_TRANSCRIPTIONS_URL};

pub struct GroqStt {
    api_key: Option<String>,
    transcriptions_url: String,
    model: String,
    language: Option<String>,
    inner: Mutex<GroqSttInner>,
    poll_rx: Mutex<Option<mpsc::UnboundedReceiver<SttTranscript>>>,
    poll_task: Mutex<Option<tokio::task::JoinHandle<SpeechResult<()>>>>,
    live_backlog_ms: AtomicU32,
    frozen_backlog_ms: AtomicU32,
    commit_waiting: AtomicBool,
}

struct GroqSttInner {
    running: bool,
    buffered: Vec<u8>,
}

impl GroqStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| default_stt_model().to_string());
        validate_stt_model(&model).map_err(SpeechError::Config)?;
        let transcriptions_url = config
            .endpoint
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_TRANSCRIPTIONS_URL.to_string());
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("GROQ_API_KEY").ok()),
            transcriptions_url,
            model,
            language: config.language.clone(),
            inner: Mutex::new(GroqSttInner {
                running: false,
                buffered: Vec::new(),
            }),
            poll_rx: Mutex::new(None),
            poll_task: Mutex::new(None),
            live_backlog_ms: AtomicU32::new(0),
            frozen_backlog_ms: AtomicU32::new(0),
            commit_waiting: AtomicBool::new(false),
        })
    }

    fn note_pushed_audio(&self, pcm_len: usize) {
        if pcm_len == 0 {
            return;
        }
        let ms = duration_ms_from_mono_s16le(pcm_len, STT_PCM_SAMPLE_RATE);
        self.live_backlog_ms.fetch_add(ms, Ordering::Relaxed);
    }

    fn freeze_backlog_for_commit(&self) {
        let utterance_ms = self.live_backlog_ms.swap(0, Ordering::Relaxed);
        self.frozen_backlog_ms
            .store(utterance_ms, Ordering::Relaxed);
        self.commit_waiting.store(true, Ordering::Relaxed);
    }

    fn clear_backlog(&self) {
        self.live_backlog_ms.store(0, Ordering::Relaxed);
        self.frozen_backlog_ms.store(0, Ordering::Relaxed);
        self.commit_waiting.store(false, Ordering::Relaxed);
    }

    fn on_final_delivered(&self) {
        self.commit_waiting.store(false, Ordering::Relaxed);
        self.frozen_backlog_ms.store(0, Ordering::Relaxed);
    }

    async fn clear_in_flight(&self) {
        if let Some(task) = self.poll_task.lock().await.take() {
            task.abort();
        }
        *self.poll_rx.lock().await = None;
    }
}

#[async_trait]
impl SttProvider for GroqStt {
    fn vendor_name(&self) -> &'static str {
        "groq"
    }

    fn decode_backlog_ms(&self) -> u32 {
        if self.commit_waiting.load(Ordering::Relaxed) {
            self.frozen_backlog_ms.load(Ordering::Relaxed)
        } else {
            self.live_backlog_ms.load(Ordering::Relaxed)
        }
    }

    async fn start(&mut self) -> SpeechResult<()> {
        self.stop().await?;
        let mut inner = self.inner.lock().await;
        inner.running = true;
        inner.buffered.clear();
        self.clear_in_flight().await;
        self.clear_backlog();
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        inner.running = false;
        inner.buffered.clear();
        self.clear_in_flight().await;
        self.clear_backlog();
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        if !inner.running || pcm.is_empty() {
            return Ok(());
        }
        inner.buffered.extend_from_slice(pcm.as_ref());
        drop(inner);
        self.note_pushed_audio(pcm.len());
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let mut rx_guard = self.poll_rx.lock().await;
        let Some(rx) = rx_guard.as_mut() else {
            return Ok(None);
        };
        if let Ok(transcript) = rx.try_recv() {
            *rx_guard = None;
            self.poll_task.lock().await.take();
            self.on_final_delivered();
            return Ok(Some(transcript));
        }
        let task_finished = self
            .poll_task
            .lock()
            .await
            .as_ref()
            .is_none_or(|task| task.is_finished());
        if task_finished {
            *rx_guard = None;
            if let Some(task) = self.poll_task.lock().await.take() {
                match task.await {
                    Ok(inner) => inner?,
                    Err(_) => {
                        return Err(SpeechError::Vendor {
                            vendor: "groq".into(),
                            message: "transcription task aborted".into(),
                        });
                    }
                }
            }
            self.on_final_delivered();
            return Ok(None);
        }
        match rx.recv().await {
            Some(transcript) => {
                *rx_guard = None;
                self.poll_task.lock().await.take();
                self.on_final_delivered();
                Ok(Some(transcript))
            }
            None => {
                *rx_guard = None;
                self.poll_task.lock().await.take();
                self.on_final_delivered();
                Ok(None)
            }
        }
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        if self.poll_rx.lock().await.is_some() {
            return Ok(());
        }
        let pcm = {
            let mut inner = self.inner.lock().await;
            if !inner.running {
                return Ok(());
            }
            std::mem::take(&mut inner.buffered)
        };
        if pcm.len() < STT_MIN_BATCH_BYTES {
            self.clear_backlog();
            return Ok(());
        }
        self.freeze_backlog_for_commit();

        #[cfg(feature = "live")]
        {
            let api_key = api_key_from(&self.api_key, "GROQ_API_KEY")?;
            let url = self.transcriptions_url.clone();
            let model = self.model.clone();
            let language = self.language.clone();
            let pcm = Bytes::from(pcm);
            let (tx, rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(async move {
                http_transcribe(pcm, &api_key, &url, &model, language.as_deref(), tx).await
            });
            *self.poll_rx.lock().await = Some(rx);
            *self.poll_task.lock().await = Some(handle);
            return Ok(());
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = pcm;
            Err(SpeechError::Vendor {
                vendor: "groq".into(),
                message: "live Groq STT requires `--features live` on vendor-groq".into(),
            })
        }
    }
}

#[cfg(feature = "live")]
async fn http_transcribe(
    pcm: Bytes,
    api_key: &str,
    url: &str,
    model: &str,
    language: Option<&str>,
    tx: mpsc::UnboundedSender<SttTranscript>,
) -> SpeechResult<()> {
    let wav = mono16_le_to_wav(pcm.as_ref());
    let part = reqwest::multipart::Part::bytes(wav)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|err| SpeechError::Vendor {
            vendor: "groq".into(),
            message: err.to_string(),
        })?;
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", model.to_string())
        .text("response_format", "json");
    if let Some(language) = language.filter(|l| !l.is_empty()) {
        form = form.text("language", language.to_string());
    }

    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "groq".into(),
            message: err.to_string(),
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let message = response.text().await.unwrap_or_else(|_| status.to_string());
        return Err(SpeechError::Vendor {
            vendor: "groq".into(),
            message,
        });
    }

    let body = response.text().await.map_err(|err| SpeechError::Vendor {
        vendor: "groq".into(),
        message: err.to_string(),
    })?;

    if let Some(text) = parse_transcription_json(&body) {
        let _ = tx.send(SttTranscript::Final(text));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::SttVendor;

    #[test]
    fn rejects_undocumented_model() {
        assert!(matches!(
            GroqStt::new(&SttConfig {
                provider: SttVendor::Groq,
                model: Some("whisper-1".into()),
                model_path: None,
                language: None,
                api_key: Some("k".into()),
                endpoint: None,
            }),
            Err(SpeechError::Config(_))
        ));
    }
}
