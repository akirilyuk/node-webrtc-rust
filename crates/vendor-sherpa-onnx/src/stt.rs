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
use sherpa_onnx::{OnlineRecognizer, OnlineStream};
use tokio::sync::Mutex;

use crate::loader::voice_debug;
use crate::native_calls::NativeCallGuard;
use crate::pool::{ActiveSessionGuard, SharedSttRecognizer, SherpaModelPool};
use crate::stt_batch::{AfterDecode, DecodeRequest};

pub(crate) const SAMPLE_RATE: i32 = 16_000;

/// Zeros fed to the stream before `InputFinished` in `finalize_utterance`. The streaming
/// zipformer decodes whole chunks only, and its last word needs look-ahead frames after it.
/// Without padding the frames after the last full chunk are never decoded, so the last word of
/// an utterance was dropped or cut ("four five six" -> "four five" / "four five si") depending
/// on how the utterance length fell on the chunk grid. 100 ms fixed the captured CI turn; 640 ms
/// covers every 20 ms alignment in `tests/stt_finalize_last_word_test.rs` with margin.
/// The `stt_replay` / `phrase_sweep` examples (`examples/support/mod.rs` `recognize`) feed the
/// same padding before `InputFinished`, so offline replays decode like the product.
pub const FINALIZE_TAIL_PADDING_MS: usize = 640;

static SHERPA_PUSH_COUNT: AtomicU64 = AtomicU64::new(0);
static SHERPA_GET_RESULT_COUNT: AtomicU64 = AtomicU64::new(0);
static SHERPA_POLL_BLOCKING_HOPS: AtomicU64 = AtomicU64::new(0);
static SHERPA_PUSH_BLOCKING_HOPS: AtomicU64 = AtomicU64::new(0);

/// Process-wide default for the inline push path: env `SHERPA_STT_INLINE_PUSH`, off only for
/// `0|false|no`.
fn inline_push_default() -> bool {
    static DEFAULT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEFAULT.get_or_init(|| {
        !matches!(
            std::env::var("SHERPA_STT_INLINE_PUSH")
                .ok()
                .map(|v| v.trim().to_ascii_lowercase())
                .as_deref(),
            Some("0" | "false" | "no")
        )
    })
}

static STT_BATCH_DECODE: std::sync::OnceLock<AtomicBool> = std::sync::OnceLock::new();

fn batch_decode_flag() -> &'static AtomicBool {
    STT_BATCH_DECODE.get_or_init(|| {
        AtomicBool::new(matches!(
            std::env::var("SHERPA_STT_BATCH_DECODE")
                .ok()
                .map(|v| v.trim().to_ascii_lowercase())
                .as_deref(),
            Some("1" | "true" | "yes")
        ))
    })
}

/// Process-wide switch for the batched decode thread; read by `SherpaStt::new`. Env default
/// `SHERPA_STT_BATCH_DECODE` (on only for `1|true|yes`, default off). For tests and probes.
pub fn set_stt_batch_decode(enabled: bool) {
    batch_decode_flag().store(enabled, Ordering::SeqCst);
}

/// Number of `push_audio` / `push_and_poll` calls that took a decode permit and a
/// `spawn_blocking` hop (perf probe / tests).
pub fn sherpa_push_blocking_hops() -> u64 {
    SHERPA_PUSH_BLOCKING_HOPS.load(Ordering::SeqCst)
}

pub fn reset_sherpa_push_blocking_hops() {
    SHERPA_PUSH_BLOCKING_HOPS.store(0, Ordering::SeqCst);
}

enum InlinePush {
    /// Audio is in the stream and no chunk is ready: nothing else to do.
    Done,
    /// Audio is in the stream; a decode or a read is due (slow path, do not accept again).
    AcceptedNeedsDecode,
    /// Nothing was done (lock busy, unread results, switch off): slow path does everything.
    NotTaken,
}

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

pub(crate) struct SttSessionState {
    pub(crate) shared: Arc<SharedSttRecognizer>,
    /// Decrements pool active_sessions exactly once on drop (stop / error / panic).
    _active: ActiveSessionGuard,
    pub(crate) stream: OnlineStream,
    last_emitted_text: String,
    pub(crate) pending: VecDeque<SttTranscript>,
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
        let shared = Arc::clone(&self.shared);
        shared.with_recognizer(|recognizer| self.read_transcript_with(recognizer))
    }

    /// `read_transcript` for a caller that already holds the recognizer.
    fn read_transcript_with(&mut self, recognizer: &OnlineRecognizer) -> Option<SttTranscript> {
        self.decoded_since_result = false;
        self.unread
            .store(!self.pending.is_empty(), Ordering::Release);
        SHERPA_GET_RESULT_COUNT.fetch_add(1, Ordering::SeqCst);
        let result = recognizer.get_result(&self.stream);
        let text = result
            .as_ref()
            .map(|value| value.text.trim())
            .unwrap_or("")
            .to_string();
        let endpoint = recognizer.is_endpoint(&self.stream);

        if text.is_empty() {
            return None;
        }

        if endpoint {
            recognizer.reset(&self.stream);
            self.last_emitted_text.clear();
            return Some(SttTranscript::Final(text));
        }

        if text == self.last_emitted_text {
            return None;
        }

        self.last_emitted_text = text.clone();
        Some(SttTranscript::Partial(text))
    }

    /// The part of `finalize_utterance` after the last decode step: read the result, queue a
    /// non-empty text as `Final`, and start the next utterance on a fresh stream. A finalized
    /// stream got InputFinished and Reset keeps the encoder state and the last tokens as context,
    /// so reusing it can repeat the previous utterance's first word ("okay okay").
    fn finish_finalize_with(&mut self, recognizer: &OnlineRecognizer) {
        SHERPA_GET_RESULT_COUNT.fetch_add(1, Ordering::SeqCst);
        let result = recognizer.get_result(&self.stream);
        let text = result
            .as_ref()
            .map(|value| value.text.trim())
            .unwrap_or("")
            .to_string();

        voice_debug(format!("sherpa finalize_utterance text={text:?}"));

        if !text.is_empty() {
            self.pending.push_back(SttTranscript::Final(text));
        }
        self.stream = recognizer.create_stream();
        self.last_emitted_text.clear();
        self.decoded_since_result = false;
        self.unread
            .store(!self.pending.is_empty(), Ordering::Release);
    }

    /// What the decode thread does once a stream has nothing ready left (`steps` decode steps ran).
    pub(crate) fn finish_after(
        &mut self,
        recognizer: &OnlineRecognizer,
        after: &AfterDecode,
        steps: u32,
    ) -> Vec<SttTranscript> {
        if steps > 0 {
            self.decoded_since_result = true;
            self.unread.store(true, Ordering::Release);
        }
        let mut ready = Vec::new();
        match after {
            AfterDecode::MarkOnly => {}
            AfterDecode::ReadTranscripts => {
                while let Some(pending) = self.pending.pop_front() {
                    ready.push(pending);
                }
                if self.decoded_since_result {
                    if let Some(transcript) = self.read_transcript_with(recognizer) {
                        ready.push(transcript);
                    }
                }
                self.unread.store(false, Ordering::Release);
            }
            AfterDecode::Finalize => self.finish_finalize_with(recognizer),
        }
        ready
    }
}

pub(crate) struct SherpaSttState {
    pub(crate) running: bool,
    pub(crate) session: Option<SttSessionState>,
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
    /// Feed audio and ask `is_ready` on the caller's task; hop to the blocking pool only to decode.
    inline_push: bool,
    /// Decode on the shared decode thread (`decode_multiple_streams`) instead of per stream.
    batch_decode: bool,
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
            inline_push: inline_push_default(),
            batch_decode: batch_decode_flag().load(Ordering::SeqCst),
        }
    }

    /// A/B switch for tests and measurements.
    pub fn set_inline_push(&mut self, enabled: bool) {
        self.inline_push = enabled;
    }

    fn try_push_inline(&self, samples: &[f32], sample_ms: u32) -> InlinePush {
        if !self.inline_push {
            return InlinePush::NotTaken;
        }
        let Ok(mut guard) = self.state.try_lock() else {
            return InlinePush::NotTaken;
        };
        if !guard.running {
            return InlinePush::Done;
        }
        let Some(session) = guard.session.as_mut() else {
            return InlinePush::Done;
        };
        if !session.pending.is_empty() || session.decoded_since_result {
            return InlinePush::NotTaken;
        }
        let _native = NativeCallGuard::enter();
        if !samples.is_empty() {
            self.accepted_ms.fetch_add(sample_ms, Ordering::Relaxed);
            session.stream.accept_waveform(SAMPLE_RATE, samples);
            let push = SHERPA_PUSH_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if push == 1 || push.is_multiple_of(50) {
                voice_debug(format!("sherpa push_audio samples={}", samples.len()));
            }
        }
        match session.shared.try_is_ready(&session.stream) {
            Some(false) => InlinePush::Done,
            _ => InlinePush::AcceptedNeedsDecode,
        }
    }

    /// Submit a locked session to its recognizer's decode thread and await the result. The
    /// request owns the guard, so the stream cannot be touched by anyone else until the thread is
    /// done, also when the caller's future is dropped.
    async fn submit_to_batcher(
        guard: tokio::sync::OwnedMutexGuard<SherpaSttState>,
        after: AfterDecode,
    ) -> SpeechResult<Vec<SttTranscript>> {
        let Some(session) = guard.session.as_ref().filter(|_| guard.running) else {
            return Ok(Vec::new());
        };
        let shared = Arc::clone(&session.shared);
        let (done, rx) = tokio::sync::oneshot::channel();
        shared.batcher().submit(DecodeRequest {
            guard,
            after,
            submitted_at: std::time::Instant::now(),
            done,
        })?;
        rx.await
            .map_err(|_| SpeechError::Internal("sherpa decode thread stopped".into()))?
    }

    async fn decode_via_batcher(&self, after: AfterDecode) -> SpeechResult<Vec<SttTranscript>> {
        let guard = Arc::clone(&self.state).lock_owned().await;
        Self::submit_to_batcher(guard, after).await
    }

    /// Batched `push_audio` / `push_and_poll`: accept the samples (unless already accepted
    /// inline) under the owned session lock, then hand the same guard to the decode thread.
    async fn push_batched(
        &self,
        samples: &[f32],
        sample_ms: u32,
        already_accepted: bool,
        after: AfterDecode,
    ) -> SpeechResult<Vec<SttTranscript>> {
        if already_accepted {
            return self.decode_via_batcher(after).await;
        }
        let mut guard = Arc::clone(&self.state).lock_owned().await;
        if !guard.running {
            return Ok(Vec::new());
        }
        let Some(session) = guard.session.as_mut() else {
            return Ok(Vec::new());
        };
        if !samples.is_empty() {
            let _native = NativeCallGuard::enter();
            self.accepted_ms.fetch_add(sample_ms, Ordering::Relaxed);
            session.stream.accept_waveform(SAMPLE_RATE, samples);
            let push = SHERPA_PUSH_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if push == 1 || push.is_multiple_of(50) {
                voice_debug(format!("sherpa push_audio samples={}", samples.len()));
            }
        }
        Self::submit_to_batcher(guard, after).await
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
            let _native = NativeCallGuard::enter();
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
            let _native = NativeCallGuard::enter();
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

    fn discard_queued_transcripts(&mut self) -> usize {
        let Ok(mut guard) = self.state.try_lock() else {
            return 0;
        };
        let Some(session) = guard.session.as_mut() else {
            return 0;
        };
        let dropped = session.pending.len();
        session.pending.clear();
        if !session.decoded_since_result {
            session.unread.store(false, Ordering::Release);
        }
        dropped
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let samples = mono_s16le_bytes_to_f32(pcm.as_ref());
        if samples.is_empty() {
            return Ok(());
        }

        let sample_ms = (samples.len() as u32 * 1000) / SAMPLE_RATE as u32;
        let already_accepted = match self.try_push_inline(&samples, sample_ms) {
            InlinePush::Done => return Ok(()),
            InlinePush::AcceptedNeedsDecode => true,
            InlinePush::NotTaken => false,
        };

        if self.batch_decode {
            self.push_batched(&samples, sample_ms, already_accepted, AfterDecode::MarkOnly)
                .await?;
            return Ok(());
        }

        let state = Arc::clone(&self.state);
        let accepted_ms = Arc::clone(&self.accepted_ms);
        let decode_semaphore = self.pool.decode_semaphore();
        let _permit = otel::acquire_sherpa_permit(&decode_semaphore)
            .await
            .map_err(|_| SpeechError::Internal("sherpa decode semaphore closed".into()))?;

        tokio::task::spawn_blocking(move || -> SpeechResult<()> {
            let _native = NativeCallGuard::enter();
            SHERPA_PUSH_BLOCKING_HOPS.fetch_add(1, Ordering::SeqCst);
            let mut guard = state.blocking_lock();
            if !guard.running {
                return Ok(());
            }
            let Some(session) = guard.session.as_mut() else {
                return Ok(());
            };

            if !already_accepted {
                accepted_ms.fetch_add(sample_ms, Ordering::Relaxed);
                session.stream.accept_waveform(SAMPLE_RATE, &samples);
                let push = SHERPA_PUSH_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if push == 1 || push.is_multiple_of(50) {
                    voice_debug(format!("sherpa push_audio samples={}", samples.len()));
                }
            }
            session.decode_ready("sherpa decode loop capped at 64 steps (possible is_ready stuck)");

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
            let _native = NativeCallGuard::enter();
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

    async fn abandon_utterance(&mut self) -> SpeechResult<bool> {
        let state = Arc::clone(&self.state);
        self.accepted_ms.store(0, Ordering::Relaxed);
        tokio::task::spawn_blocking(move || -> SpeechResult<bool> {
            let _native = NativeCallGuard::enter();
            let mut guard = state.blocking_lock();
            if !guard.running {
                return Ok(true);
            }
            let Some(session) = guard.session.as_mut() else {
                return Ok(true);
            };
            session.stream = session.shared.create_stream();
            session.pending.clear();
            session.last_emitted_text.clear();
            session.decoded_since_result = false;
            session.unread.store(false, Ordering::Release);
            voice_debug("sherpa abandon_utterance");
            Ok(true)
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))?
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let state = Arc::clone(&self.state);
        self.accepted_ms.store(0, Ordering::Relaxed);
        if self.batch_decode {
            let mut guard = Arc::clone(&self.state).lock_owned().await;
            if !guard.running {
                return Ok(());
            }
            let Some(session) = guard.session.as_mut() else {
                return Ok(());
            };
            {
                let _native = NativeCallGuard::enter();
                let tail_padding =
                    vec![0.0_f32; SAMPLE_RATE as usize * FINALIZE_TAIL_PADDING_MS / 1000];
                session.stream.accept_waveform(SAMPLE_RATE, &tail_padding);
                session.stream.input_finished();
            }
            // The final sits in `pending`; the returned vector is empty for `Finalize`.
            Self::submit_to_batcher(guard, AfterDecode::Finalize).await?;
            self.accepted_ms.store(0, Ordering::Relaxed);
            return Ok(());
        }
        let decode_semaphore = self.pool.decode_semaphore();
        let _permit = otel::acquire_sherpa_permit(&decode_semaphore)
            .await
            .map_err(|_| SpeechError::Internal("sherpa decode semaphore closed".into()))?;

        tokio::task::spawn_blocking(move || -> SpeechResult<()> {
            let _native = NativeCallGuard::enter();
            let mut guard = state.blocking_lock();
            if !guard.running {
                return Ok(());
            }
            let Some(session) = guard.session.as_mut() else {
                return Ok(());
            };

            let tail_padding =
                vec![0.0_f32; SAMPLE_RATE as usize * FINALIZE_TAIL_PADDING_MS / 1000];
            session.stream.accept_waveform(SAMPLE_RATE, &tail_padding);
            session.stream.input_finished();
            let shared = Arc::clone(&session.shared);
            shared.with_recognizer(|recognizer| {
                let mut decode_steps = 0u32;
                while recognizer.is_ready(&session.stream) {
                    recognizer.decode(&session.stream);
                    decode_steps = decode_steps.saturating_add(1);
                    if decode_steps >= 64 {
                        break;
                    }
                }
                session.finish_finalize_with(recognizer);
            });
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
        let mut samples = mono_s16le_bytes_to_f32(pcm.as_ref());
        let mut sample_ms = (samples.len() as u32 * 1000) / SAMPLE_RATE as u32;
        match self.try_push_inline(&samples, sample_ms) {
            InlinePush::Done => {
                self.accepted_ms.store(0, Ordering::Relaxed);
                return Ok(());
            }
            InlinePush::AcceptedNeedsDecode => {
                samples = Vec::new();
                sample_ms = 0;
            }
            InlinePush::NotTaken => {}
        }

        if self.batch_decode {
            let already_accepted = samples.is_empty() && sample_ms == 0;
            let ready = self
                .push_batched(
                    &samples,
                    sample_ms,
                    already_accepted,
                    AfterDecode::ReadTranscripts,
                )
                .await?;
            self.accepted_ms.store(0, Ordering::Relaxed);
            out.extend(ready);
            return Ok(());
        }

        let state = Arc::clone(&self.state);
        let accepted_ms = Arc::clone(&self.accepted_ms);
        let decode_semaphore = self.pool.decode_semaphore();
        let _permit = otel::acquire_sherpa_permit(&decode_semaphore)
            .await
            .map_err(|_| SpeechError::Internal("sherpa decode semaphore closed".into()))?;

        let ready = tokio::task::spawn_blocking(move || -> SpeechResult<Vec<SttTranscript>> {
            let _native = NativeCallGuard::enter();
            SHERPA_PUSH_BLOCKING_HOPS.fetch_add(1, Ordering::SeqCst);
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
                if push == 1 || push.is_multiple_of(50) {
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
