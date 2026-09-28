//! Azure STT — REST short audio only (final `DisplayText`; no partials per Microsoft docs).

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

use crate::factory::speech_key_from;
use crate::matrix::{
    default_stt_mode, parse_short_audio_response, short_audio_stt_url, validate_stt_mode,
};

pub struct AzureStt {
    speech_key: Option<String>,
    resource_host: String,
    mode: String,
    language: String,
    inner: Mutex<AzureSttInner>,
    poll_rx: Mutex<Option<mpsc::UnboundedReceiver<SttTranscript>>>,
    poll_task: Mutex<Option<tokio::task::JoinHandle<SpeechResult<()>>>>,
    live_backlog_ms: AtomicU32,
    frozen_backlog_ms: AtomicU32,
    commit_waiting: AtomicBool,
}

struct AzureSttInner {
    running: bool,
    buffered: Vec<u8>,
}

impl AzureStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        let mode = config
            .model
            .clone()
            .unwrap_or_else(|| default_stt_mode().to_string());
        validate_stt_mode(&mode).map_err(SpeechError::Config)?;
        let language = config
            .language
            .clone()
            .filter(|l| !l.is_empty())
            .or_else(|| std::env::var("AZURE_SPEECH_LANGUAGE").ok())
            .unwrap_or_else(|| "en-US".to_string());
        let resource_host = resolve_resource_host(config)?;
        Ok(Self {
            speech_key: config
                .api_key
                .clone()
                .or_else(speech_key_from_env),
            resource_host,
            mode,
            language,
            inner: Mutex::new(AzureSttInner {
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

    fn stt_url(&self) -> SpeechResult<String> {
        short_audio_stt_url(&self.resource_host, &self.mode, &self.language)
            .map_err(SpeechError::Config)
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

fn speech_key_from_env() -> Option<String> {
    ["AZURE_SPEECH_KEY", "SPEECH_KEY"]
        .into_iter()
        .find_map(|env| std::env::var(env).ok())
        .filter(|s| !s.is_empty())
}

fn resolve_resource_host(config: &SttConfig) -> SpeechResult<String> {
    if let Some(endpoint) = config.endpoint.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(normalize_host(endpoint));
    }
    if let Ok(host) = std::env::var("AZURE_SPEECH_RESOURCE") {
        if !host.is_empty() {
            return Ok(normalize_host(&host));
        }
    }
    if let Ok(region) = std::env::var("AZURE_SPEECH_REGION") {
        if !region.is_empty() {
            return Ok(format!("{region}.api.cognitive.microsoft.com"));
        }
    }
    Err(SpeechError::Config(
        "Azure STT requires config.endpoint, AZURE_SPEECH_RESOURCE, or AZURE_SPEECH_REGION"
            .into(),
    ))
}

fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

#[async_trait]
impl SttProvider for AzureStt {
    fn vendor_name(&self) -> &'static str {
        "azure"
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
                            vendor: "azure".into(),
                            message: "recognition task aborted".into(),
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
            let key = speech_key_from(&self.speech_key)?;
            let url = self.stt_url()?;
            let wav = mono16_le_to_wav(&pcm);
            let (tx, rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(async move {
                http_short_audio_recognize(wav, &key, &url, tx).await
            });
            *self.poll_rx.lock().await = Some(rx);
            *self.poll_task.lock().await = Some(handle);
            return Ok(());
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = pcm;
            Err(SpeechError::Vendor {
                vendor: "azure".into(),
                message: "live Azure STT requires `--features live` on vendor-azure".into(),
            })
        }
    }
}

#[cfg(feature = "live")]
async fn http_short_audio_recognize(
    wav: Vec<u8>,
    speech_key: &str,
    url: &str,
    tx: mpsc::UnboundedSender<SttTranscript>,
) -> SpeechResult<()> {
    let response = reqwest::Client::new()
        .post(url)
        .header("Ocp-Apim-Subscription-Key", speech_key)
        .header("Accept", "application/json")
        .header(
            "Content-Type",
            "audio/wav; codecs=audio/pcm; samplerate=16000",
        )
        .body(wav)
        .send()
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "azure".into(),
            message: err.to_string(),
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let message = response.text().await.unwrap_or_else(|_| status.to_string());
        return Err(SpeechError::Vendor {
            vendor: "azure".into(),
            message,
        });
    }
    let body = response.text().await.map_err(|err| SpeechError::Vendor {
        vendor: "azure".into(),
        message: err.to_string(),
    })?;
    if let Some(text) = parse_short_audio_response(&body) {
        let _ = tx.send(SttTranscript::Final(text));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::SttVendor;

    #[test]
    fn rejects_undocumented_mode() {
        assert!(matches!(
            AzureStt::new(&SttConfig {
                provider: SttVendor::Azure,
                model: Some("dictation".into()),
                model_path: None,
                language: Some("en-US".into()),
                api_key: Some("k".into()),
                endpoint: Some("res.cognitiveservices.azure.com".into()),
            }),
            Err(SpeechError::Config(_))
        ));
    }
}
