use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[cfg(feature = "live")]
use async_openai::types::{AudioInput, CreateTranscriptionRequestArgs, InputSource};
#[cfg(feature = "live")]
use async_openai::Client;
use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
#[cfg(feature = "live")]
use node_webrtc_rust_speech::pcm::mono16_le_to_wav;
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, STT_MIN_BATCH_BYTES, STT_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};

#[cfg(feature = "live")]
use crate::factory::api_key_from;
use crate::factory::{OpenAiSttState, SharedSttState};

pub struct OpenAiStt {
    api_key: Option<String>,
    model: String,
    language: Option<String>,
    state: SharedSttState,
    /// PCM accepted and not yet returned from [`SttProvider::poll_transcript`].
    /// Kept high through the batch HTTP call so C1 does not abort a long utterance.
    backlog_ms: AtomicU32,
    transcribe_in_flight: AtomicBool,
}

impl OpenAiStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        let api_key = config
            .api_key
            .clone()
            .or_else(|| std::env::var("OPENAI_API_KEY").ok());
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
            backlog_ms: AtomicU32::new(0),
            transcribe_in_flight: AtomicBool::new(false),
        })
    }

    fn store_backlog_for_buffered_len(&self, len: usize) {
        self.backlog_ms
            .store(backlog_ms_for_pcm_len(len), Ordering::Relaxed);
    }

    fn clear_inflight_backlog(&self) {
        self.transcribe_in_flight.store(false, Ordering::Relaxed);
        self.backlog_ms.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    async fn buffered_len(&self) -> usize {
        self.state.lock().await.buffered.len()
    }
}

/// Whole utterance for one batch request. `None` when the buffer is too short
/// (those bytes are dropped so they do not prefix the next utterance).
pub(crate) fn take_final_utterance(state: &mut OpenAiSttState) -> Option<Bytes> {
    if !state.running {
        return None;
    }
    if state.buffered.len() < STT_MIN_BATCH_BYTES {
        state.buffered.clear();
        return None;
    }
    Some(Bytes::from(std::mem::take(&mut state.buffered)))
}

fn backlog_ms_for_pcm_len(len: usize) -> u32 {
    if len == 0 {
        return 0;
    }
    duration_ms_from_mono_s16le(len, STT_PCM_SAMPLE_RATE)
}

#[async_trait]
impl SttProvider for OpenAiStt {
    fn vendor_name(&self) -> &'static str {
        "openai"
    }

    fn decode_backlog_ms(&self) -> u32 {
        self.backlog_ms.load(Ordering::Relaxed)
    }

    async fn start(&mut self) -> SpeechResult<()> {
        let mut state = self.state.lock().await;
        state.running = true;
        state.buffered.clear();
        state.pending = None;
        self.clear_inflight_backlog();
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let mut state = self.state.lock().await;
        state.running = false;
        state.buffered.clear();
        state.pending = None;
        self.clear_inflight_backlog();
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let mut state = self.state.lock().await;
        if state.running {
            state.buffered.extend_from_slice(pcm.as_ref());
            let ms = backlog_ms_for_pcm_len(state.buffered.len());
            if self.transcribe_in_flight.load(Ordering::Relaxed) {
                self.backlog_ms.fetch_max(ms, Ordering::Relaxed);
            } else {
                self.backlog_ms.store(ms, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        // Do not transcribe here. `STT_PREFERRED_BATCH_BYTES` (~1 s) used to
        // emit `user_speech_final` while the caller was still speaking, so a
        // counting phrase came back as "one" / "One, two."
        let pending = {
            let mut state = self.state.lock().await;
            state.pending.take()
        };
        if pending.is_some() {
            self.transcribe_in_flight.store(false, Ordering::Relaxed);
            let rest = self.state.lock().await.buffered.len();
            self.store_backlog_for_buffered_len(rest);
        }
        Ok(pending)
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let pcm = {
            let mut state = self.state.lock().await;
            take_final_utterance(&mut state)
        };
        let Some(pcm) = pcm else {
            self.clear_inflight_backlog();
            return Ok(());
        };
        // Leave `backlog_ms` at the utterance duration until `poll_transcript`
        // returns the final. Clearing it here lets C1 fire during the HTTP call.
        self.transcribe_in_flight.store(true, Ordering::Relaxed);

        #[cfg(feature = "live")]
        {
            let api_key = match api_key_from(&self.api_key, "OPENAI_API_KEY") {
                Ok(key) => key,
                Err(err) => {
                    self.clear_inflight_backlog();
                    return Err(err);
                }
            };
            match live_transcribe(pcm, &api_key, &self.model, self.language.as_deref()).await {
                Ok(transcript) => {
                    self.state.lock().await.pending = Some(transcript);
                    Ok(())
                }
                Err(err) => {
                    self.clear_inflight_backlog();
                    Err(err)
                }
            }
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = pcm;
            self.clear_inflight_backlog();
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

    let client =
        Client::with_config(async_openai::config::OpenAIConfig::new().with_api_key(api_key));
    let response = client
        .audio()
        .transcribe(request)
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "openai".into(),
            message: err.to_string(),
        })?;

    let text = response.text.trim();
    if text.is_empty() {
        return Ok(SttTranscript::Final(String::new()));
    }
    Ok(SttTranscript::Final(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::SttVendor;
    use node_webrtc_rust_speech::pcm::STT_PREFERRED_BATCH_BYTES;

    fn test_config() -> SttConfig {
        SttConfig {
            provider: SttVendor::Openai,
            model: Some("gpt-4o-mini-transcribe".into()),
            model_path: None,
            language: Some("en".into()),
            api_key: Some("test-key".into()),
            endpoint: None,
        }
    }

    #[test]
    fn finalize_takes_the_whole_utterance_not_a_one_second_slice() {
        let total = STT_PREFERRED_BATCH_BYTES + 5_000;
        let mut state = OpenAiSttState {
            buffered: vec![1u8; total],
            running: true,
            pending: None,
        };
        let pcm = take_final_utterance(&mut state).expect("utterance");
        assert_eq!(pcm.len(), total);
        assert!(state.buffered.is_empty());
        assert!(take_final_utterance(&mut state).is_none());
    }

    #[test]
    fn short_buffer_is_not_transcribed() {
        let mut state = OpenAiSttState {
            buffered: vec![1u8; STT_MIN_BATCH_BYTES - 1],
            running: true,
            pending: None,
        };
        assert!(take_final_utterance(&mut state).is_none());
        assert!(state.buffered.is_empty());
    }

    #[tokio::test]
    async fn poll_holds_audio_past_one_second_until_finalize() {
        let mut stt = OpenAiStt::new(&test_config()).expect("stt");
        stt.start().await.expect("start");
        let total = STT_PREFERRED_BATCH_BYTES + 8_000;
        stt.push_audio(Bytes::from(vec![0u8; total]))
            .await
            .expect("push");

        assert!(stt.poll_transcript().await.expect("poll").is_none());
        assert_eq!(stt.buffered_len().await, total);
        assert!(
            stt.decode_backlog_ms() > 1_000,
            "backlog {} ms",
            stt.decode_backlog_ms()
        );

        assert!(stt.poll_transcript().await.expect("poll again").is_none());
        assert_eq!(stt.buffered_len().await, total);

        // The live feature would call OpenAI. Assert the compile-time gate only
        // when that feature is off so this test never sends audio.
        #[cfg(not(feature = "live"))]
        {
            let err = stt.finalize_utterance().await.expect_err("no live feature");
            let SpeechError::Vendor { message, .. } = err else {
                panic!("expected vendor error");
            };
            assert!(message.contains("live"));
            assert_eq!(stt.buffered_len().await, 0);
            assert_eq!(stt.decode_backlog_ms(), 0);
        }
    }
}
