use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::mono_s16le_bytes_to_f32;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use sherpa_onnx::OnlineStream;
use tokio::sync::Mutex;

use crate::pool::{SharedSttRecognizer, SherpaModelPool};

pub(crate) const SAMPLE_RATE: i32 = 16_000;

static SHERPA_PUSH_COUNT: AtomicU64 = AtomicU64::new(0);

fn voice_debug_enabled() -> bool {
    matches!(
        std::env::var("VOICE_DEBUG").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn voice_debug(message: impl AsRef<str>) {
    if voice_debug_enabled() {
        eprintln!("[voice-debug] {}", message.as_ref());
    }
}

struct SttSessionState {
    shared: Arc<SharedSttRecognizer>,
    stream: OnlineStream,
    last_emitted_text: String,
    pending: VecDeque<SttTranscript>,
}

struct SherpaSttState {
    running: bool,
    session: Option<SttSessionState>,
}

pub struct SherpaStt {
    config: SttConfig,
    pool: Arc<crate::pool::SherpaModelPool>,
    state: Arc<Mutex<SherpaSttState>>,
}

impl SherpaStt {
    pub fn new(config: &SttConfig) -> Self {
        Self {
            config: config.clone(),
            pool: SherpaModelPool::global(),
            state: Arc::new(Mutex::new(SherpaSttState {
                running: false,
                session: None,
            })),
        }
    }

    fn open_session(config: &SttConfig, pool: &crate::pool::SherpaModelPool) -> SpeechResult<SttSessionState> {
        let shared = pool.get_or_create_stt(config)?;
        let stream = shared.create_stream();
        shared.session_started();
        voice_debug("sherpa OnlineRecognizer ready (pooled)");
        Ok(SttSessionState {
            shared,
            stream,
            last_emitted_text: String::new(),
            pending: VecDeque::new(),
        })
    }
}

#[async_trait]
impl SttProvider for SherpaStt {
    fn vendor_name(&self) -> &'static str {
        "local-sherpa"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        let config = self.config.clone();
        let pool = Arc::clone(&self.pool);
        let state = Arc::clone(&self.state);

        tokio::task::spawn_blocking(move || -> SpeechResult<()> {
            let session = SherpaStt::open_session(&config, &pool)?;
            let mut guard = state.blocking_lock();
            guard.running = true;
            guard.session = Some(session);
            Ok(())
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))??;

        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let state = Arc::clone(&self.state);

        tokio::task::spawn_blocking(move || {
            let mut guard = state.blocking_lock();
            guard.running = false;
            if let Some(session) = guard.session.as_mut() {
                session.shared.with_recognizer(|recognizer| {
                    recognizer.reset(&session.stream);
                });
                session.last_emitted_text.clear();
                session.pending.clear();
                session.shared.session_ended();
            }
            guard.session = None;
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))?;

        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let samples = mono_s16le_bytes_to_f32(pcm.as_ref());
        if samples.is_empty() {
            return Ok(());
        }

        let state = Arc::clone(&self.state);
        let decode_semaphore = self.pool.decode_semaphore();
        let _permit = decode_semaphore
            .acquire()
            .await
            .map_err(|_| SpeechError::Internal("sherpa decode semaphore closed".into()))?;

        tokio::task::spawn_blocking(move || -> SpeechResult<()> {
            let mut guard = state.blocking_lock();
            if !guard.running {
                return Ok(());
            }
            let Some(session) = guard.session.as_mut() else {
                return Ok(());
            };

            session.stream.accept_waveform(SAMPLE_RATE, &samples);
            session.shared.with_recognizer(|recognizer| {
                let mut decode_steps = 0u32;
                while recognizer.is_ready(&session.stream) {
                    recognizer.decode(&session.stream);
                    decode_steps = decode_steps.saturating_add(1);
                    if decode_steps >= 64 {
                        voice_debug(
                            "sherpa decode loop capped at 64 steps (possible is_ready stuck)",
                        );
                        break;
                    }
                }
            });

            let push = SHERPA_PUSH_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if push == 1 || push % 50 == 0 {
                voice_debug(format!("sherpa push_audio samples={}", samples.len()));
            }

            Ok(())
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))??;

        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let state = Arc::clone(&self.state);
        let decode_semaphore = self.pool.decode_semaphore();
        let _permit = decode_semaphore
            .acquire()
            .await
            .map_err(|_| SpeechError::Internal("sherpa decode semaphore closed".into()))?;

        tokio::task::spawn_blocking(move || -> SpeechResult<Option<SttTranscript>> {
            let mut guard = state.blocking_lock();
            if !guard.running {
                return Ok(None);
            }
            let Some(session) = guard.session.as_mut() else {
                return Ok(None);
            };

            if let Some(pending) = session.pending.pop_front() {
                return Ok(Some(pending));
            }

            let (text, endpoint) = session.shared.with_recognizer(|recognizer| {
                let result = recognizer.get_result(&session.stream);
                let text = result
                    .as_ref()
                    .map(|value| value.text.trim())
                    .unwrap_or("")
                    .to_string();
                let endpoint = recognizer.is_endpoint(&session.stream);
                (text, endpoint)
            });

            if text.is_empty() {
                return Ok(None);
            }

            voice_debug(format!("sherpa hypothesis={text:?} endpoint={endpoint}"));

            if endpoint {
                session.shared.with_recognizer(|recognizer| {
                    recognizer.reset(&session.stream);
                });
                session.last_emitted_text.clear();
                return Ok(Some(SttTranscript::Final(text)));
            }

            if text == session.last_emitted_text {
                return Ok(None);
            }

            session.last_emitted_text = text.clone();
            Ok(Some(SttTranscript::Partial(text)))
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))?
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let state = Arc::clone(&self.state);
        let decode_semaphore = self.pool.decode_semaphore();
        let _permit = decode_semaphore
            .acquire()
            .await
            .map_err(|_| SpeechError::Internal("sherpa decode semaphore closed".into()))?;

        tokio::task::spawn_blocking(move || -> SpeechResult<()> {
            let mut guard = state.blocking_lock();
            if !guard.running {
                return Ok(());
            }
            let Some(session) = guard.session.as_mut() else {
                return Ok(());
            };

            session.stream.input_finished();
            session.shared.with_recognizer(|recognizer| {
                let mut decode_steps = 0u32;
                while recognizer.is_ready(&session.stream) {
                    recognizer.decode(&session.stream);
                    decode_steps = decode_steps.saturating_add(1);
                    if decode_steps >= 64 {
                        break;
                    }
                }

                let result = recognizer.get_result(&session.stream);
                let text = result
                    .as_ref()
                    .map(|value| value.text.trim())
                    .unwrap_or("")
                    .to_string();

                voice_debug(format!("sherpa finalize_utterance text={text:?}"));

                if !text.is_empty() {
                    session.pending.push_back(SttTranscript::Final(text));
                }
                recognizer.reset(&session.stream);
            });
            session.last_emitted_text.clear();
            Ok(())
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))??;

        Ok(())
    }
}
