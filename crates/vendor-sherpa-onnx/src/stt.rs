use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::otel;
use node_webrtc_rust_speech::pcm::mono_s16le_bytes_to_f32;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use sherpa_onnx::OnlineStream;
use tokio::sync::Mutex;

use crate::loader::voice_debug;
use crate::pool::{ActiveSessionGuard, SharedSttRecognizer, SherpaModelPool};

pub(crate) const SAMPLE_RATE: i32 = 16_000;

static SHERPA_PUSH_COUNT: AtomicU64 = AtomicU64::new(0);
static SHERPA_GET_RESULT_COUNT: AtomicU64 = AtomicU64::new(0);
static SHERPA_POLL_BLOCKING_HOPS: AtomicU64 = AtomicU64::new(0);

/// Number of `poll_transcript` calls that took a decode permit and a `spawn_blocking` hop
/// (perf probe / tests). Calls answered by the `unread` fast path do not count.
pub fn sherpa_poll_blocking_hops() -> u64 {
    SHERPA_POLL_BLOCKING_HOPS.load(Ordering::SeqCst)
}

pub fn reset_sherpa_poll_blocking_hops() {
    SHERPA_POLL_BLOCKING_HOPS.store(0, Ordering::SeqCst);
}

/// Number of `get_result` reads this process has made on STT streams (perf probe / tests).
pub fn sherpa_get_result_count() -> u64 {
    SHERPA_GET_RESULT_COUNT.load(Ordering::SeqCst)
}

pub fn reset_sherpa_get_result_count() {
    SHERPA_GET_RESULT_COUNT.store(0, Ordering::SeqCst);
}

struct SttSessionState {
    shared: Arc<SharedSttRecognizer>,
    /// Decrements pool active_sessions exactly once on drop (stop / error / panic).
    _active: ActiveSessionGuard,
    stream: OnlineStream,
    last_emitted_text: String,
    pending: VecDeque<SttTranscript>,
    /// True when at least one decode step ran since the result was last read. `get_result` and
    /// `is_endpoint` only change after a decode, so the read is skipped while this is false.
    decoded_since_result: bool,
    /// Fast-path mirror of "something is unread" (`decoded_since_result` or `pending` not empty),
    /// shared with `SherpaStt` so `poll_transcript` can skip the blocking hop. Only written under
    /// the session lock; `decoded_since_result` stays the authority.
    unread: Arc<AtomicBool>,
}

impl SttSessionState {
    /// Decode every ready chunk (capped at 64 steps). Sets `decoded_since_result` when one ran.
    fn decode_ready(&mut self, cap_message: &'static str) {
        let steps = self.shared.with_recognizer(|recognizer| {
            let mut decode_steps = 0u32;
            while recognizer.is_ready(&self.stream) {
                recognizer.decode(&self.stream);
                decode_steps = decode_steps.saturating_add(1);
                if decode_steps >= 64 {
                    voice_debug(cap_message);
                    break;
                }
            }
            decode_steps
        });
        if steps > 0 {
            self.decoded_since_result = true;
            self.unread.store(true, Ordering::Release);
        }
    }

    /// Read the current result and apply the partial / final / endpoint rules. Resets
    /// `decoded_since_result`.
    fn read_transcript(&mut self) -> Option<SttTranscript> {
        self.decoded_since_result = false;
        self.unread
            .store(!self.pending.is_empty(), Ordering::Release);
        SHERPA_GET_RESULT_COUNT.fetch_add(1, Ordering::SeqCst);
        let (text, endpoint) = self.shared.with_recognizer(|recognizer| {
            let result = recognizer.get_result(&self.stream);
            let text = result
                .as_ref()
                .map(|value| value.text.trim())
                .unwrap_or("")
                .to_string();
            let endpoint = recognizer.is_endpoint(&self.stream);
            (text, endpoint)
        });

        if text.is_empty() {
            return None;
        }

        if endpoint {
            self.shared.with_recognizer(|recognizer| {
                recognizer.reset(&self.stream);
            });
            self.last_emitted_text.clear();
            return Some(SttTranscript::Final(text));
        }

        if text == self.last_emitted_text {
            return None;
        }

        self.last_emitted_text = text.clone();
        Some(SttTranscript::Partial(text))
    }
}

struct SherpaSttState {
    running: bool,
    session: Option<SttSessionState>,
}

pub struct SherpaStt {
    config: SttConfig,
    pool: Arc<crate::pool::SherpaModelPool>,
    state: Arc<Mutex<SherpaSttState>>,
    /// Something to read (a decode ran since the last read, or `pending` is not empty). When false,
    /// `poll_transcript` returns `None` without a decode permit or a blocking hop.
    unread: Arc<AtomicBool>,
    /// Audio accepted by `push_audio` not yet cleared by `poll_transcript` decode (sync read for C1).
    accepted_ms: Arc<AtomicU32>,
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
            unread: Arc::new(AtomicBool::new(false)),
            accepted_ms: Arc::new(AtomicU32::new(0)),
        }
    }

    fn open_session(
        config: &SttConfig,
        pool: &crate::pool::SherpaModelPool,
        unread: Arc<AtomicBool>,
    ) -> SpeechResult<SttSessionState> {
        let shared = pool.get_or_create_stt(config)?;
        // Acquire before create_stream so a panic/error during stream init still decrements.
        let active = shared.track_session();
        let stream = shared.create_stream();
        voice_debug("sherpa OnlineRecognizer ready (pooled)");
        Ok(SttSessionState {
            shared,
            _active: active,
            stream,
            last_emitted_text: String::new(),
            pending: VecDeque::new(),
            decoded_since_result: false,
            unread,
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
        let unread = Arc::clone(&self.unread);

        tokio::task::spawn_blocking(move || -> SpeechResult<()> {
            let session = SherpaStt::open_session(&config, &pool, unread)?;
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
        let unread = Arc::clone(&self.unread);
        self.accepted_ms.store(0, Ordering::Relaxed);

        tokio::task::spawn_blocking(move || {
            let mut guard = state.blocking_lock();
            guard.running = false;
            unread.store(false, Ordering::Release);
            if let Some(session) = guard.session.as_mut() {
                session.shared.with_recognizer(|recognizer| {
                    recognizer.reset(&session.stream);
                });
                session.last_emitted_text.clear();
                session.pending.clear();
                session.decoded_since_result = false;
            }
            // Dropping SttSessionState runs ActiveSessionGuard::drop (exactly once).
            guard.session = None;
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))?;

        Ok(())
    }

    fn decode_backlog_ms(&self) -> u32 {
        self.accepted_ms.load(Ordering::Relaxed)
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let samples = mono_s16le_bytes_to_f32(pcm.as_ref());
        if samples.is_empty() {
            return Ok(());
        }

        let state = Arc::clone(&self.state);
        let accepted_ms = Arc::clone(&self.accepted_ms);
        let sample_ms = (samples.len() as u32 * 1000) / SAMPLE_RATE as u32;
        let decode_semaphore = self.pool.decode_semaphore();
        let _permit = otel::acquire_sherpa_permit(&decode_semaphore)
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

            accepted_ms.fetch_add(sample_ms, Ordering::Relaxed);
            session.stream.accept_waveform(SAMPLE_RATE, &samples);
            session.decode_ready("sherpa decode loop capped at 64 steps (possible is_ready stuck)");

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
        if !self.unread.load(Ordering::Acquire) {
            // Nothing decoded since the last read and nothing queued: no permit, no blocking hop.
            self.accepted_ms.store(0, Ordering::Relaxed);
            return Ok(None);
        }
        let state = Arc::clone(&self.state);
        let accepted_ms = Arc::clone(&self.accepted_ms);
        let decode_semaphore = self.pool.decode_semaphore();
        let _permit = otel::acquire_sherpa_permit(&decode_semaphore)
            .await
            .map_err(|_| SpeechError::Internal("sherpa decode semaphore closed".into()))?;

        tokio::task::spawn_blocking(move || -> SpeechResult<Option<SttTranscript>> {
            SHERPA_POLL_BLOCKING_HOPS.fetch_add(1, Ordering::SeqCst);
            let mut guard = state.blocking_lock();
            if !guard.running {
                return Ok(None);
            }
            let Some(session) = guard.session.as_mut() else {
                return Ok(None);
            };

            if let Some(pending) = session.pending.pop_front() {
                accepted_ms.store(0, Ordering::Relaxed);
                if session.pending.is_empty() && !session.decoded_since_result {
                    session.unread.store(false, Ordering::Release);
                }
                return Ok(Some(pending));
            }

            session.decode_ready(
                "sherpa poll decode loop capped at 64 steps (possible is_ready stuck)",
            );
            accepted_ms.store(0, Ordering::Relaxed);

            if !session.decoded_since_result {
                // No decode since the last read: result and endpoint cannot have changed.
                // (A false positive of the `unread` fast path ends here.)
                session.unread.store(false, Ordering::Release);
                return Ok(None);
            }
            Ok(session.read_transcript())
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))?
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let state = Arc::clone(&self.state);
        self.accepted_ms.store(0, Ordering::Relaxed);
        let decode_semaphore = self.pool.decode_semaphore();
        let _permit = otel::acquire_sherpa_permit(&decode_semaphore)
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

                SHERPA_GET_RESULT_COUNT.fetch_add(1, Ordering::SeqCst);
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
            session.decoded_since_result = false;
            session
                .unread
                .store(!session.pending.is_empty(), Ordering::Release);
            Ok(())
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))??;

        Ok(())
    }

    async fn push_and_poll(
        &mut self,
        pcm: Bytes,
        out: &mut Vec<SttTranscript>,
    ) -> SpeechResult<()> {
        let samples = mono_s16le_bytes_to_f32(pcm.as_ref());

        let state = Arc::clone(&self.state);
        let accepted_ms = Arc::clone(&self.accepted_ms);
        let sample_ms = (samples.len() as u32 * 1000) / SAMPLE_RATE as u32;
        let decode_semaphore = self.pool.decode_semaphore();
        let _permit = otel::acquire_sherpa_permit(&decode_semaphore)
            .await
            .map_err(|_| SpeechError::Internal("sherpa decode semaphore closed".into()))?;

        let ready = tokio::task::spawn_blocking(move || -> SpeechResult<Vec<SttTranscript>> {
            let mut ready = Vec::new();
            let mut guard = state.blocking_lock();
            if !guard.running {
                return Ok(ready);
            }
            let Some(session) = guard.session.as_mut() else {
                return Ok(ready);
            };

            if !samples.is_empty() {
                accepted_ms.fetch_add(sample_ms, Ordering::Relaxed);
                session.stream.accept_waveform(SAMPLE_RATE, &samples);
                let push = SHERPA_PUSH_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if push == 1 || push % 50 == 0 {
                    voice_debug(format!("sherpa push_audio samples={}", samples.len()));
                }
            }
            session.decode_ready("sherpa decode loop capped at 64 steps (possible is_ready stuck)");
            accepted_ms.store(0, Ordering::Relaxed);

            while let Some(pending) = session.pending.pop_front() {
                ready.push(pending);
            }
            if session.decoded_since_result {
                if let Some(transcript) = session.read_transcript() {
                    ready.push(transcript);
                }
            }
            // `pending` is drained and the result read (or nothing was decoded): nothing unread.
            session.unread.store(false, Ordering::Release);
            Ok(ready)
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))??;

        out.extend(ready);
        Ok(())
    }
}
