//! Voice agent orchestration (attach, start/stop, TTS injection, inbound PCM).
//!
//! [`VoiceAgent`] is the main entry point: one instance per WebRTC session. Inbound audio
//! is processed in [`VoiceAgent::process_inbound_pcm`]; the TypeScript SDK calls that from
//! `RemoteAudioTrack.readSample()` in a loop after [`VoiceAgent::start`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use tokio::sync::{broadcast, Mutex, Notify};

use crate::config::{EventDeliveryMode, SendTextToTtsOptions, VadConfig, VoiceAgentConfig};
use crate::error::{SpeechError, SpeechResult};
use crate::events::{SpeechEvent, SpeechEventBus};
use crate::pcm::i16_samples_to_bytes;
use crate::pipeline::{SttProvider, SttTranscript, TtsProvider};
use crate::registry::VendorRegistry;
use crate::stt_pre_roll::SttPreRollBuffer;
use crate::tts_buffer::TtsBuffer;
use crate::vad::{handle_barge_in, VadEngine, VadTransition};

/// Callback invoked when PCM should be written to the outbound track.
pub type PcmWriter = Arc<dyn Fn(Bytes, u32) -> SpeechResult<()> + Send + Sync>;

/// Callback invoked to read inbound PCM from the attached remote track.
pub type PcmReader = Arc<dyn Fn() -> SpeechResult<Option<(Bytes, u32)>> + Send + Sync>;

static INBOUND_PCM_FRAMES: AtomicU64 = AtomicU64::new(0);

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

/// Synthetic silence fed to STT before finalize — aligned with Sherpa roundtrip harness.
fn stt_endpoint_tail_ms(vad: &VadConfig) -> u32 {
    vad.min_silence_duration_ms.max(400).min(600)
}

/// True when most of the post–speech-end gate hold has elapsed (90%), i.e. resume is a new phrase not a digit gap.
fn gate_hold_long_pause_elapsed(hold_total: u32, hold_elapsed: u32) -> bool {
    hold_total > 0 && hold_elapsed.saturating_mul(10) > hold_total.saturating_mul(9)
}

struct AgentInner {
    config: VoiceAgentConfig,
    attached: bool,
    running: bool,
    vad: Option<VadEngine>,
    stt_pre_roll: Option<SttPreRollBuffer>,
    /// Milliseconds of inbound audio still forwarded to STT after VAD speech end.
    stt_gate_hold_ms: u32,
    /// When true, finalize STT once gate hold drains (after trailing speech is relayed).
    stt_finalize_pending: bool,
    /// Endpoint tail + `finalize_utterance` already started for the current pending close.
    stt_endpoint_closing_started: bool,
    /// Set when poll_transcript already emitted user_speech_final for this utterance.
    stt_final_emitted_this_utterance: bool,
    /// `user_speaking_end` paired with the next `user_speech_final` for this utterance.
    stt_speaking_end_emitted_this_utterance: bool,
    /// `user_speaking_start` already emitted for the current STT utterance.
    stt_speaking_start_emitted_this_utterance: bool,
    /// True while agent TTS is synthesizing or playing outbound audio.
    agent_speaking: bool,
    agent_speaking_since: Option<Instant>,
    /// STT vendor PCM feed open for the current VAD-triggered utterance.
    stt_stream_open: bool,
    /// User STT session open (`user_stt_start` … `user_stt_end` / `user_stt_not_found`).
    user_stt_session_open: bool,
    /// Set on each VAD `SpeechStart` when `vad.enabled` (barge prerequisite).
    vad_triggered_this_utterance: bool,
    /// C1: ms remaining until `user_stt_not_found` when no partial after `vad_triggered`.
    stt_listen_deadline_ms: u32,
    /// C2: ms remaining until forced `user_speech_final` after last partial or `SpeechEnd`.
    utterance_finalize_deadline_ms: u32,
    /// Start C2 only after gate hold drains when `SpeechEnd` preceded partials.
    defer_utterance_finalize_until_hold: bool,
    /// Last partial text for C2 forced final fallback.
    last_partial_text: Option<String>,
    /// At least one `user_speech_partial` this utterance.
    partials_emitted_this_utterance: bool,
    /// VAD saw speech during agent TTS; defer immediate flush until STT partial (if required).
    barge_awaiting_stt_partial: bool,
    /// Semantic barge already fired for the current agent playback generation.
    stt_barge_fired_this_agent_playback: bool,
    pcm_writer: Option<PcmWriter>,
    pcm_reader: Option<PcmReader>,
}

struct TtsSynthesisJob {
    text: String,
    done: Option<tokio::sync::oneshot::Sender<SpeechResult<()>>>,
}

/// One voice agent session bound to a single peer connection.
///
/// Holds VAD (optional), STT/TTS providers, TTS outbound buffer, and utterance state
/// (`stt_gate_hold_ms`, finalize pending, barge-in flags). Thread-safe via internal `Mutex` / `Mutex`es.
pub struct VoiceAgent {
    event_bus: SpeechEventBus,
    tts_buffer: TtsBuffer,
    #[allow(dead_code)]
    registry: Arc<VendorRegistry>,
    inner: Arc<Mutex<AgentInner>>,
    stt: Mutex<Option<Box<dyn SttProvider>>>,
    tts: Arc<Mutex<Option<Box<dyn TtsProvider>>>>,
    tts_drain_wake: Arc<Notify>,
    tts_drain_worker: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    tts_synthesis_queue: Arc<Mutex<VecDeque<TtsSynthesisJob>>>,
    tts_synthesis_wake: Arc<Notify>,
    tts_synthesis_worker: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    tts_synthesis_busy: Arc<AtomicBool>,
    /// Incremented on barge/flush/cancel so in-flight ONNX synthesis can drop late PCM.
    tts_synthesis_epoch: Arc<AtomicU64>,
}

impl VoiceAgent {
    /// Builds agents with STT/TTS from `registry` and VAD/pre-roll from `config.vad`.
    pub fn new(config: VoiceAgentConfig, registry: Arc<VendorRegistry>) -> SpeechResult<Self> {
        let mut stt = None;
        let mut tts = None;

        if let Some(stt_cfg) = &config.stt {
            stt = Some(registry.create_stt(stt_cfg)?);
        }
        if let Some(tts_cfg) = &config.tts {
            tts = Some(registry.create_tts(tts_cfg)?);
        }

        let vad = if config.vad.enabled {
            Some(VadEngine::new(config.vad.clone())?)
        } else {
            None
        };
        let stt_pre_roll = if config.vad.enabled && config.vad.gate_stt {
            Some(SttPreRollBuffer::from_vad_config(&config.vad))
        } else {
            None
        };

        Ok(Self {
            event_bus: SpeechEventBus::new(),
            tts_buffer: TtsBuffer::new(),
            registry,
            inner: Arc::new(Mutex::new(AgentInner {
                config,
                attached: false,
                running: false,
                vad,
                stt_pre_roll,
                stt_gate_hold_ms: 0,
                stt_finalize_pending: false,
                stt_endpoint_closing_started: false,
                stt_final_emitted_this_utterance: false,
                stt_speaking_end_emitted_this_utterance: false,
                stt_speaking_start_emitted_this_utterance: false,
                agent_speaking: false,
                agent_speaking_since: None,
                stt_stream_open: false,
                user_stt_session_open: false,
                vad_triggered_this_utterance: false,
                stt_listen_deadline_ms: 0,
                utterance_finalize_deadline_ms: 0,
                defer_utterance_finalize_until_hold: false,
                last_partial_text: None,
                partials_emitted_this_utterance: false,
                barge_awaiting_stt_partial: false,
                stt_barge_fired_this_agent_playback: false,
                pcm_writer: None,
                pcm_reader: None,
            })),
            stt: Mutex::new(stt),
            tts: Arc::new(Mutex::new(tts)),
            tts_drain_wake: Arc::new(Notify::new()),
            tts_drain_worker: Arc::new(Mutex::new(None)),
            tts_synthesis_queue: Arc::new(Mutex::new(VecDeque::new())),
            tts_synthesis_wake: Arc::new(Notify::new()),
            tts_synthesis_worker: Arc::new(Mutex::new(None)),
            tts_synthesis_busy: Arc::new(AtomicBool::new(false)),
            tts_synthesis_epoch: Arc::new(AtomicU64::new(0)),
        })
    }

    fn invalidate_inflight_tts_synthesis(&self) {
        self.tts_synthesis_epoch.fetch_add(1, Ordering::SeqCst);
    }

    pub fn event_bus(&self) -> &SpeechEventBus {
        &self.event_bus
    }

    pub fn tts_buffer(&self) -> &TtsBuffer {
        &self.tts_buffer
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<SpeechEvent> {
        self.event_bus.subscribe()
    }

    pub fn events_mode(&self) -> EventDeliveryMode {
        self.inner
            .try_lock()
            .map(|inner| inner.config.events.mode)
            .unwrap_or(EventDeliveryMode::Both)
    }

    /// Registers inbound (user) and outbound (agent TTS) PCM callbacks. Required before [`start`](Self::start).
    pub async fn attach(
        &self,
        pcm_reader: PcmReader,
        pcm_writer: PcmWriter,
    ) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        inner.pcm_reader = Some(pcm_reader);
        inner.pcm_writer = Some(pcm_writer);
        inner.attached = true;
        Ok(())
    }

    /// Starts STT vendor and TTS drain worker. Inbound PCM is driven by the host via [`process_inbound_pcm`](Self::process_inbound_pcm).
    pub async fn start(&self) -> SpeechResult<()> {
        {
            let mut inner = self.inner.lock().await;
            if !inner.attached {
                return Err(SpeechError::NotAttached);
            }
            if inner.running {
                return Err(SpeechError::AlreadyRunning);
            }
            inner.running = true;
        }

        voice_debug("VoiceAgent running=true");

        let mut stt = self.stt.lock().await;
        if let Some(stt) = stt.as_mut() {
            stt.start().await?;
            voice_debug(format!("STT started ({})", stt.vendor_name()));
        }
        self.ensure_tts_drain_worker().await;
        Ok(())
    }

    async fn ensure_tts_drain_worker(&self) {
        Self::ensure_tts_drain_worker_shared(
            &self.tts_drain_worker,
            &self.tts_drain_wake,
            &self.tts_buffer,
            &self.inner,
            &self.event_bus,
        )
        .await;
    }

    /// Wait until outbound TTS playback finishes (for tests and explicit synchronization).
    pub async fn wait_tts_playback_idle(&self) -> SpeechResult<()> {
        voice_debug(
            "wait_tts_playback_idle: waiting for synthesis queue, agent_speaking=false, TTS buffer drained",
        );
        let deadline = Instant::now() + std::time::Duration::from_secs(45);
        loop {
            let agent_speaking = self.inner.lock().await.agent_speaking;
            let queued = self.tts_buffer.is_speaking().await;
            let synth_pending = !self.tts_synthesis_queue.lock().await.is_empty()
                || self.tts_synthesis_busy.load(Ordering::SeqCst);
            if !agent_speaking && !queued && !synth_pending {
                voice_debug("wait_tts_playback_idle: playback idle");
                return Ok(());
            }
            if Instant::now() >= deadline {
                voice_debug(format!(
                    "wait_tts_playback_idle: TIMEOUT agent_speaking={agent_speaking} tts_queued={queued} synth_pending={synth_pending}"
                ));
                return Err(SpeechError::Internal(
                    "timed out waiting for TTS playback to finish".into(),
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    pub async fn stop(&self) -> SpeechResult<()> {
        {
            let mut inner = self.inner.lock().await;
            if !inner.running {
                return Err(SpeechError::NotRunning);
            }
            inner.running = false;
        }

        let mut stt = self.stt.lock().await;
        if let Some(stt) = stt.as_mut() {
            stt.stop().await?;
        }
        Ok(())
    }

    /// Synthesizes text and enqueues stereo 48 kHz PCM for real-time outbound drain.
    ///
    /// Default ([`SendTextToTtsOptions::default`]) waits until synthesis and playback for this
    /// utterance finish. Pass `non_blocking: true` to return once the job is queued.
    pub async fn send_text_to_tts(&self, text: &str) -> SpeechResult<()> {
        self.send_text_to_tts_with_options(text, SendTextToTtsOptions::default())
            .await
    }

    /// Like [`send_text_to_tts`](Self::send_text_to_tts) with explicit queue / wait behavior.
    pub async fn send_text_to_tts_with_options(
        &self,
        text: &str,
        options: SendTextToTtsOptions,
    ) -> SpeechResult<()> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        {
            let tts = self.tts.lock().await;
            if tts.is_none() {
                return Err(SpeechError::Config("TTS not configured".into()));
            }
        }

        let (done_tx, done_rx) = if options.non_blocking {
            (None, None)
        } else {
            let (tx, rx) = tokio::sync::oneshot::channel();
            (Some(tx), Some(rx))
        };

        {
            let mut queue = self.tts_synthesis_queue.lock().await;
            queue.push_back(TtsSynthesisJob {
                text: trimmed.to_string(),
                done: done_tx,
            });
        }

        self.ensure_tts_synthesis_worker().await;
        self.tts_synthesis_wake.notify_one();

        if let Some(rx) = done_rx {
            rx.await
                .map_err(|_| SpeechError::Internal("TTS job cancelled".into()))??;
        }

        Ok(())
    }

    async fn ensure_tts_synthesis_worker(&self) {
        let mut slot = self.tts_synthesis_worker.lock().await;
        if slot.is_some() {
            return;
        }
        let wake = Arc::clone(&self.tts_synthesis_wake);
        let queue = Arc::clone(&self.tts_synthesis_queue);
        let tts = Arc::clone(&self.tts);
        let tts_buffer = self.tts_buffer.clone();
        let tts_drain_wake = Arc::clone(&self.tts_drain_wake);
        let tts_drain_worker = Arc::clone(&self.tts_drain_worker);
        let inner = Arc::clone(&self.inner);
        let event_bus = self.event_bus.clone();
        let synthesis_busy = Arc::clone(&self.tts_synthesis_busy);
        let synthesis_epoch = Arc::clone(&self.tts_synthesis_epoch);
        *slot = Some(tokio::spawn(async move {
            loop {
                wake.notified().await;
                loop {
                    let job = {
                        let mut pending = queue.lock().await;
                        pending.pop_front()
                    };
                    let Some(job) = job else {
                        break;
                    };

                    synthesis_busy.store(true, Ordering::SeqCst);
                    let result = Self::run_tts_synthesis_job(
                        &job.text,
                        &tts,
                        &tts_buffer,
                        &tts_drain_wake,
                        &tts_drain_worker,
                        &inner,
                        &event_bus,
                        &synthesis_epoch,
                    )
                    .await;
                    synthesis_busy.store(false, Ordering::SeqCst);

                    if let Some(done) = job.done {
                        let _ = done.send(result);
                    } else if let Err(error) = result {
                        voice_debug(format!("non-blocking TTS synthesis error: {error}"));
                    }
                }
            }
        }));
    }

    async fn run_tts_synthesis_job(
        text: &str,
        tts: &Arc<Mutex<Option<Box<dyn TtsProvider>>>>,
        tts_buffer: &TtsBuffer,
        tts_drain_wake: &Arc<Notify>,
        tts_drain_worker: &Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        synthesis_epoch: &Arc<AtomicU64>,
    ) -> SpeechResult<()> {
        let epoch_at_start = synthesis_epoch.load(Ordering::SeqCst);
        let generation_at_start = tts_buffer.current_generation().await;
        let chunks = {
            let tts_guard = tts.lock().await;
            let provider = tts_guard
                .as_ref()
                .ok_or_else(|| SpeechError::Config("TTS not configured".into()))?;
            provider.synthesize(text).await?
        };

        if synthesis_epoch.load(Ordering::SeqCst) != epoch_at_start {
            voice_debug("TTS synthesis discarded (invalidated during synthesize)");
            return Ok(());
        }

        if chunks.is_empty() {
            return Ok(());
        }

        if !tts_buffer
            .enqueue_if_generation(chunks, Some(generation_at_start))
            .await
        {
            voice_debug("TTS synthesis discarded (buffer flushed during synthesize)");
            return Ok(());
        }
        Self::ensure_tts_drain_worker_shared(
            tts_drain_worker,
            tts_drain_wake,
            tts_buffer,
            inner,
            event_bus,
        )
        .await;
        tts_drain_wake.notify_one();
        Self::wait_job_playback_idle(tts_buffer, inner).await
    }

    async fn ensure_tts_drain_worker_shared(
        slot: &Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
        wake: &Arc<Notify>,
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
    ) {
        let mut guard = slot.lock().await;
        if guard.is_some() {
            return;
        }
        let wake = Arc::clone(wake);
        let tts_buffer = tts_buffer.clone();
        let inner = Arc::clone(inner);
        let event_bus = event_bus.clone();
        *guard = Some(tokio::spawn(async move {
            loop {
                wake.notified().await;
                if let Err(error) = VoiceAgent::run_tts_drain(&tts_buffer, &inner, &event_bus).await
                {
                    voice_debug(format!("TTS drain error: {error}"));
                }
            }
        }));
    }

    async fn wait_job_playback_idle(
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
    ) -> SpeechResult<()> {
        let deadline = Instant::now() + std::time::Duration::from_secs(45);
        loop {
            let agent_speaking = inner.lock().await.agent_speaking;
            let queued = tts_buffer.is_speaking().await;
            if !agent_speaking && !queued {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(SpeechError::Internal(
                    "timed out waiting for TTS job playback".into(),
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    async fn cancel_pending_tts_synthesis(&self) {
        self.invalidate_inflight_tts_synthesis();
        let mut queue = self.tts_synthesis_queue.lock().await;
        for job in queue.drain(..) {
            if let Some(done) = job.done {
                let _ = done.send(Err(SpeechError::Internal("TTS cancelled".into())));
            }
        }
    }

    /// Clears pending outbound TTS (manual cancel or barge-in when `flush_tts` is enabled).
    pub async fn flush_tts(&self) -> SpeechResult<()> {
        self.cancel_pending_tts_synthesis().await;
        let barge_in = {
            let inner = self.inner.lock().await;
            inner.config.vad.barge_in.clone()
        };
        if barge_in.enabled && !barge_in.use_vad {
            handle_barge_in(&barge_in, &self.tts_buffer, |event| self.emit(event)).await;
            self.end_agent_speaking(false).await;
            voice_debug("agent_speaking=false (manual barge-in flush)");
        } else {
            self.tts_buffer.flush().await;
            self.end_agent_speaking(true).await;
            voice_debug("agent_speaking=false (flush_tts)");
            self.emit(SpeechEvent::agent_speaking_end());
        }
        Ok(())
    }

    /// Non-blocking poll for stream-mode event delivery (NAPI / TS `speechEvents()`).
    pub async fn pull_speech_event(&self) -> Option<SpeechEvent> {
        None
    }

    /// Clears TTS playback state; optionally arms STT hold when playback ends and VAD is idle.
    async fn end_agent_speaking(&self, arm_stt_hold_after_playback: bool) {
        Self::end_agent_speaking_inner(&self.inner, arm_stt_hold_after_playback).await;
    }

    fn agent_playback_guard_active(inner: &AgentInner) -> bool {
        if !inner.agent_speaking {
            return false;
        }
        let Some(since) = inner.agent_speaking_since else {
            return false;
        };
        let guard_ms = inner.config.vad.barge_in.agent_playback_guard_ms;
        since.elapsed() < std::time::Duration::from_millis(guard_ms as u64)
    }

    fn stt_partial_qualifies_for_barge(inner: &AgentInner, text: &str) -> bool {
        let trimmed = text.trim();
        let min_chars = inner.config.vad.barge_in.min_stt_partial_chars.max(1) as usize;
        if trimmed.len() < min_chars {
            return false;
        }
        trimmed.chars().any(|c| c.is_alphanumeric())
    }

    async fn try_stt_gated_barge_in(&self, partial_text: &str) -> SpeechResult<()> {
        let (should_barge, barge_in) = {
            let inner = self.inner.lock().await;
            let barge_in_cfg = &inner.config.vad.barge_in;
            if !barge_in_cfg.enabled || !barge_in_cfg.require_stt_partial || inner.config.stt.is_none() {
                return Ok(());
            }
            if !inner.agent_speaking {
                return Ok(());
            }
            if !inner.vad_triggered_this_utterance {
                return Ok(());
            }
            if !Self::stt_partial_qualifies_for_barge(&inner, partial_text) {
                return Ok(());
            }
            if inner.stt_barge_fired_this_agent_playback {
                return Ok(());
            }
            (
                true,
                barge_in_cfg.clone(),
            )
        };
        if !should_barge {
            return Ok(());
        }
        {
            let mut inner = self.inner.lock().await;
            inner.barge_awaiting_stt_partial = false;
            inner.stt_barge_fired_this_agent_playback = true;
        }
        voice_debug(format!(
            "STT-gated barge-in: partial {:?}",
            partial_text.trim()
        ));
        handle_barge_in(&barge_in, &self.tts_buffer, |event| self.emit(event)).await;
        self.cancel_pending_tts_synthesis().await;
        if barge_in.flush_tts {
            let was_agent_speaking = {
                let inner = self.inner.lock().await;
                inner.agent_speaking
            };
            if was_agent_speaking {
                self.end_agent_speaking(false).await;
                self.emit(SpeechEvent::agent_speaking_end());
            }
        }
        Ok(())
    }

    async fn emit_stt_stream_start_if_needed(&self) {
        let emit = {
            let mut inner = self.inner.lock().await;
            if inner.stt_stream_open {
                false
            } else {
                inner.stt_stream_open = true;
                true
            }
        };
        if emit {
            voice_debug("emit stt_stream_start");
            self.emit(SpeechEvent::stt_stream_start());
        }
    }

    async fn emit_user_stt_start_if_needed(&self) {
        let emit = {
            let mut inner = self.inner.lock().await;
            if inner.user_stt_session_open {
                false
            } else {
                inner.user_stt_session_open = true;
                true
            }
        };
        if emit {
            voice_debug("emit user_stt_start");
            self.emit(SpeechEvent::user_stt_start());
        }
    }

    fn arm_utterance_finalize_timer(inner: &mut AgentInner) {
        if inner.partials_emitted_this_utterance && !inner.stt_final_emitted_this_utterance {
            inner.utterance_finalize_deadline_ms =
                inner.config.vad.utterance_finalize_timeout_ms;
            inner.defer_utterance_finalize_until_hold = false;
            voice_debug(format!(
                "utterance finalize timer: {} ms",
                inner.utterance_finalize_deadline_ms
            ));
        }
    }

    async fn on_vad_speech_start(&self) -> SpeechResult<()> {
        let (barge_in, guard_active, agent_speaking, has_stt, vad_enabled) = {
            let inner = self.inner.lock().await;
            (
                inner.config.vad.barge_in.clone(),
                Self::agent_playback_guard_active(&inner),
                inner.agent_speaking,
                inner.config.stt.is_some(),
                inner.config.vad.enabled,
            )
        };
        if !vad_enabled {
            return Ok(());
        }

        voice_debug("emit vad_triggered (VAD SpeechStart)");
        self.emit(SpeechEvent::vad_triggered());
        if has_stt {
            self.emit_user_stt_start_if_needed().await;
            self.emit_stt_stream_start_if_needed().await;
        }

        {
            let mut inner = self.inner.lock().await;
            inner.vad_triggered_this_utterance = true;
            if has_stt && !inner.partials_emitted_this_utterance {
                inner.stt_listen_deadline_ms = inner.config.vad.stt_listen_timeout_ms;
            }
            if !inner.partials_emitted_this_utterance {
                inner.utterance_finalize_deadline_ms = 0;
                inner.defer_utterance_finalize_until_hold = false;
            }
        }

        if barge_in.enabled && barge_in.use_vad && agent_speaking {
            if guard_active {
                voice_debug(format!(
                    "barge-in suppressed: agent playback guard {} ms",
                    barge_in.agent_playback_guard_ms
                ));
                return Ok(());
            }
            if barge_in.require_stt_partial && has_stt {
                let mut inner = self.inner.lock().await;
                inner.barge_awaiting_stt_partial = true;
                voice_debug(
                    "barge-in deferred until qualifying STT partial (require_stt_partial)",
                );
                return Ok(());
            }
            if !barge_in.require_stt_partial || !has_stt {
                voice_debug("immediate barge-in on VAD SpeechStart (require_stt_partial=false)");
                handle_barge_in(&barge_in, &self.tts_buffer, |event| self.emit(event)).await;
                self.cancel_pending_tts_synthesis().await;
                if barge_in.flush_tts {
                    let was_agent_speaking = {
                        let inner = self.inner.lock().await;
                        inner.agent_speaking
                    };
                    self.end_agent_speaking(false).await;
                    if was_agent_speaking {
                        self.emit(SpeechEvent::agent_speaking_end());
                    }
                }
            }
        }
        Ok(())
    }

    async fn close_stt_stream_not_found(&self) -> SpeechResult<()> {
        let (stream_was_open, session_open, vad_speaking) = {
            let mut inner = self.inner.lock().await;
            let stream_was_open = inner.stt_stream_open;
            let session_open = inner.user_stt_session_open;
            let vad_speaking = inner.vad.as_ref().map(|v| v.is_speaking()).unwrap_or(false);
            inner.stt_stream_open = false;
            inner.user_stt_session_open = false;
            inner.stt_listen_deadline_ms = 0;
            inner.utterance_finalize_deadline_ms = 0;
            inner.defer_utterance_finalize_until_hold = false;
            inner.vad_triggered_this_utterance = false;
            inner.barge_awaiting_stt_partial = false;
            (stream_was_open, session_open, vad_speaking)
        };
        if stream_was_open {
            voice_debug("emit stt_stream_end (C1: no STT partial)");
            self.emit(SpeechEvent::stt_stream_end());
        }
        if session_open {
            voice_debug("emit user_stt_not_found + user_stt_end (C1)");
            self.emit(SpeechEvent::user_stt_not_found());
            self.emit(SpeechEvent::user_stt_end());
        }
        if !vad_speaking {
            let emit_end = {
                let mut inner = self.inner.lock().await;
                if inner.stt_speaking_end_emitted_this_utterance {
                    false
                } else {
                    inner.stt_speaking_end_emitted_this_utterance = true;
                    true
                }
            };
            if emit_end {
                self.emit(SpeechEvent::user_speaking_end());
            }
        }
        Ok(())
    }

    async fn force_close_utterance(&self) -> SpeechResult<()> {
        voice_debug("force_close_utterance (C2 timeout or stall)");
        let last_partial = {
            let inner = self.inner.lock().await;
            inner.last_partial_text.clone()
        };

        let needs_finalize = {
            let inner = self.inner.lock().await;
            inner.stt_stream_open && !inner.stt_final_emitted_this_utterance
        };
        if needs_finalize {
            let tail_ms = {
                let inner = self.inner.lock().await;
                stt_endpoint_tail_ms(&inner.config.vad)
            };
            {
                let mut inner = self.inner.lock().await;
                inner.stt_endpoint_closing_started = true;
            }
            self.push_stt_endpoint_tail(tail_ms).await?;
            self.finalize_stt_utterance().await?;
        }

        let (emit_stream_end, emit_stt_end, already_final) = {
            let mut inner = self.inner.lock().await;
            let stream = inner.stt_stream_open;
            let session = inner.user_stt_session_open;
            if inner.stt_stream_open {
                inner.stt_stream_open = false;
            }
            if inner.user_stt_session_open {
                inner.user_stt_session_open = false;
            }
            inner.stt_listen_deadline_ms = 0;
            inner.utterance_finalize_deadline_ms = 0;
            inner.defer_utterance_finalize_until_hold = false;
            inner.vad_triggered_this_utterance = false;
            inner.barge_awaiting_stt_partial = false;
            let already_final = inner.stt_final_emitted_this_utterance;
            (stream, session, already_final)
        };
        if emit_stream_end {
            self.emit(SpeechEvent::stt_stream_end());
        }
        if emit_stt_end {
            self.emit(SpeechEvent::user_stt_end());
        }

        if !already_final {
            let emit_speaking_end = {
                let mut inner = self.inner.lock().await;
                let emit_end = !inner.stt_speaking_end_emitted_this_utterance;
                if emit_end {
                    inner.stt_speaking_end_emitted_this_utterance = true;
                }
                emit_end
            };
            if emit_speaking_end {
                voice_debug("emit user_speaking_end (forced utterance close)");
                self.emit(SpeechEvent::user_speaking_end());
            }
            let final_text = last_partial.unwrap_or_default();
            {
                let mut inner = self.inner.lock().await;
                inner.stt_final_emitted_this_utterance = true;
                inner.stt_finalize_pending = false;
                inner.stt_endpoint_closing_started = false;
            }
            voice_debug(format!(
                "emit user_speech_final (forced): {}",
                if final_text.len() > 80 {
                    format!("{}…", &final_text[..80])
                } else {
                    final_text.clone()
                }
            ));
            self.emit(SpeechEvent::user_speech_final(final_text));
        }
        Ok(())
    }

    fn arm_stt_hold_if_idle(inner: &mut AgentInner) {
        if !inner.config.vad.gate_stt || inner.stt_gate_hold_ms > 0 {
            return;
        }
        let still_speaking = inner.vad.as_ref().map(|v| v.is_speaking()).unwrap_or(false);
        if still_speaking {
            return;
        }
        inner.stt_gate_hold_ms = inner.config.vad.stt_gate_hold_ms;
        inner.stt_finalize_pending = true;
        voice_debug(format!(
            "STT gate hold: {} ms after TTS playback ended",
            inner.stt_gate_hold_ms
        ));
    }

    /// Finish STT for an utterance waiting on gate hold / finalize (e.g. before new `SpeechStart`).
    async fn complete_pending_utterance_if_any(&self) -> SpeechResult<()> {
        let (needed, tail_ms, closing_started) = {
            let inner = self.inner.lock().await;
            if !inner.config.vad.gate_stt || inner.config.stt.is_none() {
                return Ok(());
            }
            let needed =
                inner.stt_finalize_pending && !inner.stt_final_emitted_this_utterance;
            (
                needed,
                stt_endpoint_tail_ms(&inner.config.vad),
                inner.stt_endpoint_closing_started,
            )
        };
        if !needed {
            return Ok(());
        }
        voice_debug(
            "STT: completing previous utterance (pending finalize before new speech)",
        );
        if !closing_started {
            {
                let mut inner = self.inner.lock().await;
                inner.stt_endpoint_closing_started = true;
            }
            self.push_stt_endpoint_tail(tail_ms).await?;
        }
        self.finalize_stt_utterance().await
    }

    fn reset_utterance_state_for_new_speech(inner: &mut AgentInner) {
        inner.stt_gate_hold_ms = 0;
        inner.stt_finalize_pending = false;
        inner.stt_endpoint_closing_started = false;
        inner.stt_final_emitted_this_utterance = false;
        inner.stt_speaking_end_emitted_this_utterance = false;
        inner.stt_speaking_start_emitted_this_utterance = false;
        inner.stt_stream_open = false;
        inner.user_stt_session_open = false;
        inner.vad_triggered_this_utterance = false;
        inner.stt_listen_deadline_ms = 0;
        inner.utterance_finalize_deadline_ms = 0;
        inner.defer_utterance_finalize_until_hold = false;
        inner.last_partial_text = None;
        inner.partials_emitted_this_utterance = false;
    }

    async fn emit_user_speaking_start_if_needed(&self) {
        let emit = {
            let mut inner = self.inner.lock().await;
            if inner.stt_speaking_start_emitted_this_utterance {
                false
            } else {
                inner.stt_speaking_start_emitted_this_utterance = true;
                true
            }
        };
        if emit {
            voice_debug("emit user_speaking_start (before STT transcript)");
            self.emit(SpeechEvent::user_speaking_start());
        }
    }

    /// Processes one inbound WebRTC PCM frame (typically 20 ms stereo 48 kHz).
    ///
    /// Runs VAD transitions, barge-in, STT gate/hold/finalize, and STT poll. No-op when
    /// [`stop`](Self::stop) has run. Set `VOICE_DEBUG=1` for per-frame diagnostics.
    pub async fn process_inbound_pcm(&self, pcm: Bytes, duration_ms: u32) -> SpeechResult<()> {
        let call = INBOUND_PCM_FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
        if call == 1 || call % 50 == 0 {
            voice_debug(format!(
                "process_inbound_pcm call={call} bytes={} duration_ms={duration_ms}",
                pcm.len()
            ));
        }

        let running = {
            let inner = self.inner.lock().await;
            inner.running
        };
        if !running {
            voice_debug(format!("process_inbound_pcm call={call} skipped: agent not running"));
            return Ok(());
        }

        let mono = crate::pcm::stereo_48k_to_mono_16k(pcm.as_ref());
        let mono_bytes = i16_samples_to_bytes(&mono);

        let (
            transitions,
            gate_stt,
            speech_start,
            complete_previous_utterance,
            _frame_active,
            vad_pending,
            vad_speaking,
        ) = {
            let mut inner = self.inner.lock().await;
            let was_speaking = inner.vad.as_ref().map(|v| v.is_speaking()).unwrap_or(false);
            let gate_stt = inner.config.vad.gate_stt;

            let (transitions, frame_active) = match inner.vad.as_mut() {
                Some(vad) => vad.process_webrtc_pcm(pcm.as_ref(), duration_ms)?,
                None => (Vec::new(), false),
            };

            let vad_pending = inner
                .vad
                .as_ref()
                .map(VadEngine::is_pending_speech)
                .unwrap_or(false);
            let vad_speaking = inner
                .vad
                .as_ref()
                .map(|v| v.is_speaking())
                .unwrap_or(false);

            if gate_stt {
                let barge_listen =
                    inner.agent_speaking && !inner.stt_stream_open;
                if let Some(pre_roll) = inner.stt_pre_roll.as_mut() {
                    // During agent TTS the STT gate is closed until VAD SpeechStart. User speech
                    // often begins before VAD confirms (agent bleed / echo). Keep a continuous
                    // lookback ring so the flush at SpeechStart includes the first syllable.
                    if barge_listen {
                        pre_roll.push(&mono_bytes);
                    } else if !was_speaking && (frame_active || vad_pending) {
                        // Voice-only — silence must not fill the ring (see stt_pre_roll tests).
                        pre_roll.push(&mono_bytes);
                    }
                }
            }

            let speech_start = transitions.contains(&VadTransition::SpeechStart);
            let mut complete_previous_utterance = false;

            if transitions.contains(&VadTransition::SpeechEnd) {
                if gate_stt {
                    inner.stt_pre_roll.as_mut().map(SttPreRollBuffer::clear);
                    inner.stt_gate_hold_ms = inner.config.vad.stt_gate_hold_ms;
                    inner.stt_finalize_pending = true;
                    if inner.partials_emitted_this_utterance
                        && !inner.stt_final_emitted_this_utterance
                    {
                        inner.defer_utterance_finalize_until_hold = true;
                        inner.utterance_finalize_deadline_ms = 0;
                    }
                    voice_debug(format!(
                        "STT gate hold: {} ms after speech end{}",
                        inner.stt_gate_hold_ms,
                        if inner.agent_speaking {
                            " (agent TTS playing — finalize after playback)"
                        } else {
                            ""
                        }
                    ));
                } else if inner.partials_emitted_this_utterance
                    && !inner.stt_final_emitted_this_utterance
                {
                    Self::arm_utterance_finalize_timer(&mut inner);
                }
            } else if inner.stt_gate_hold_ms > 0 && !speech_start {
                // Brief gap (counting): cancel hold. Long pause then new speech: finish prior phrase.
                // Use declared VAD speech only — `frame_active` can flicker on TTS tail / echo during hold.
                let user_voice_active = vad_speaking;
                if user_voice_active && !inner.stt_endpoint_closing_started {
                    let hold_total = inner.config.vad.stt_gate_hold_ms;
                    let hold_elapsed = hold_total.saturating_sub(inner.stt_gate_hold_ms);
                    let long_pause_before_resume =
                        gate_hold_long_pause_elapsed(hold_total, hold_elapsed);
                    if inner.stt_finalize_pending
                        && !inner.stt_final_emitted_this_utterance
                        && long_pause_before_resume
                    {
                        complete_previous_utterance = true;
                        voice_debug(
                            "STT: voice resumed after long gate hold (≥90% elapsed) — completing previous utterance",
                        );
                    } else {
                        inner.stt_gate_hold_ms = 0;
                        inner.stt_finalize_pending = false;
                        voice_debug(
                            "STT gate hold cancelled: voice active again before hold expired",
                        );
                    }
                } else {
                    let before = inner.stt_gate_hold_ms;
                    inner.stt_gate_hold_ms = before.saturating_sub(duration_ms);
                    let after = inner.stt_gate_hold_ms;
                    if before > 0 && after == 0 {
                        voice_debug(
                            "STT gate hold expired — utterance may finalize on next inbound frame",
                        );
                        if inner.defer_utterance_finalize_until_hold
                            && inner.partials_emitted_this_utterance
                            && !inner.stt_final_emitted_this_utterance
                        {
                            inner.defer_utterance_finalize_until_hold = false;
                            Self::arm_utterance_finalize_timer(&mut inner);
                        }
                    } else if before > 0
                        && after > 0
                        && (before / 500) != (after / 500)
                    {
                        voice_debug(format!("STT gate hold: {after} ms remaining"));
                    }
                }
            }

            (
                transitions,
                gate_stt,
                speech_start,
                complete_previous_utterance,
                frame_active,
                vad_pending,
                vad_speaking,
            )
        };

        // C1 / C2 timeout ticks (only when VAD enabled and STT stream lifecycle active).
        let (c1_expired, c2_expired) = {
            let mut inner = self.inner.lock().await;
            if !inner.config.vad.enabled || inner.config.stt.is_none() {
                (false, false)
            } else {
                let mut c1 = false;
                let mut c2 = false;
                if inner.stt_stream_open
                    && !inner.partials_emitted_this_utterance
                    && inner.stt_listen_deadline_ms > 0
                {
                    inner.stt_listen_deadline_ms =
                        inner.stt_listen_deadline_ms.saturating_sub(duration_ms);
                    if inner.stt_listen_deadline_ms == 0 {
                        c1 = true;
                    }
                }
                if inner.utterance_finalize_deadline_ms > 0
                    && !inner.defer_utterance_finalize_until_hold
                {
                    inner.utterance_finalize_deadline_ms =
                        inner.utterance_finalize_deadline_ms.saturating_sub(duration_ms);
                    if inner.utterance_finalize_deadline_ms == 0 {
                        c2 = true;
                    }
                }
                (c1, c2)
            }
        };

        if c1_expired {
            self.close_stt_stream_not_found().await?;
        }
        if c2_expired {
            self.force_close_utterance().await?;
        }

        if complete_previous_utterance {
            self.complete_pending_utterance_if_any().await?;
        }

        let mut pre_roll_flushed_this_frame = false;
        if speech_start {
            let long_pause_new_phrase = {
                let inner = self.inner.lock().await;
                if !inner.config.vad.gate_stt {
                    true
                } else {
                    let hold_total = inner.config.vad.stt_gate_hold_ms;
                    let hold_elapsed = hold_total.saturating_sub(inner.stt_gate_hold_ms);
                    inner.stt_endpoint_closing_started
                        || (hold_total > 0 && hold_elapsed > hold_total / 2)
                }
            };
            if long_pause_new_phrase {
                self.complete_pending_utterance_if_any().await?;
            }
            let mut pre_roll_after_start = None;
            {
                let mut inner = self.inner.lock().await;
                if long_pause_new_phrase {
                    Self::reset_utterance_state_for_new_speech(&mut inner);
                } else {
                    // Brief gap (e.g. counting): same utterance — clear hold only.
                    inner.stt_gate_hold_ms = 0;
                    inner.stt_finalize_pending = false;
                    inner.stt_endpoint_closing_started = false;
                }
                if gate_stt {
                    if let Some(pre_roll) = inner.stt_pre_roll.as_mut() {
                        let buffered = pre_roll.drain();
                        if !buffered.is_empty() {
                            voice_debug(format!(
                                "STT pre-roll flush: {} bytes (~{} ms)",
                                buffered.len(),
                                crate::pcm::duration_ms_from_mono_s16le(
                                    buffered.len(),
                                    crate::pcm::STT_PCM_SAMPLE_RATE,
                                )
                            ));
                            pre_roll_after_start = Some(buffered);
                        }
                    }
                }
            }
            if let Some(buffered) = pre_roll_after_start {
                if !buffered.is_empty() {
                    self.push_stt_audio_bytes(buffered).await?;
                    pre_roll_flushed_this_frame = true;
                }
            }
        }

        let mut speech_end_transition = false;
        for transition in &transitions {
            voice_debug(format!("VAD {transition:?}"));
            match transition {
                VadTransition::SpeechStart => {
                    self.on_vad_speech_start().await?;
                    self.emit_user_speaking_start_if_needed().await;
                }
                VadTransition::SpeechEnd => {
                    speech_end_transition = true;
                    let (has_stt, defer_speaking_end, agent_speaking) = {
                        let inner = self.inner.lock().await;
                        (
                            inner.config.stt.is_some(),
                            inner.config.vad.gate_stt && !inner.agent_speaking,
                            inner.agent_speaking,
                        )
                    };
                    if agent_speaking {
                        voice_debug(
                            "user_speaking_end suppressed (VAD SpeechEnd during agent TTS)",
                        );
                    } else if has_stt {
                        voice_debug(
                            "user_speaking_end deferred until user_speech_final (STT utterance close)",
                        );
                    } else if defer_speaking_end {
                        voice_debug(
                            "user_speaking_end deferred until STT gate hold expires (gate_stt, no STT)",
                        );
                    } else {
                        self.emit(SpeechEvent::user_speaking_end());
                    }
                }
            }
        }

        // After VAD transitions (SpeechStart opens STT stream via `on_vad_speech_start`).
        let (stt_audio_open, stt_poll_open, should_finalize_utterance) = {
            let inner = self.inner.lock().await;
            let pending_gate = inner.config.vad.gate_stt_open_on_pending && vad_pending;
            let utterance_closing =
                inner.stt_finalize_pending && !inner.stt_final_emitted_this_utterance;
            let vad_enabled = inner.config.vad.enabled;
            let stt_audio_open = if !vad_enabled {
                true
            } else if gate_stt {
                if !inner.stt_stream_open {
                    false
                } else {
                    vad_speaking
                        || inner.stt_gate_hold_ms > 0
                        || pending_gate
                        || utterance_closing
                }
            } else if inner.stt_stream_open {
                true
            } else {
                true
            };
            let stt_poll_open = !gate_stt
                || stt_audio_open
                || utterance_closing
                || inner.stt_stream_open
                || inner.utterance_finalize_deadline_ms > 0;
            let should_finalize_utterance = gate_stt
                && utterance_closing
                && !inner.agent_speaking
                && !inner.stt_endpoint_closing_started
                && inner.stt_gate_hold_ms == 0
                && !vad_speaking;
            (stt_audio_open, stt_poll_open, should_finalize_utterance)
        };

        // When gate is closed: skip STT push/poll. During agent TTS we still run VAD every frame
        // (listening on the inbound track); only defer STT until VAD sees user voice.
        let gate_closed_skip_stt =
            gate_stt && !stt_poll_open && !should_finalize_utterance;
        if gate_closed_skip_stt {
            if call == 1 || call % 50 == 0 {
                let agent_speaking = self.inner.lock().await.agent_speaking;
                if !agent_speaking {
                    voice_debug(format!(
                        "process_inbound_pcm call={call} skipped: gate_stt closed (not speaking, hold expired)"
                    ));
                }
            }
            let agent_speaking = self.inner.lock().await.agent_speaking;
            if !agent_speaking {
                return Ok(());
            }
        }

        if call == 1 || call % 50 == 0 {
            voice_debug(format!(
                "inbound PCM frame={call} bytes={} duration_ms={duration_ms}",
                pcm.len()
            ));
        }

        if (!gate_stt || stt_audio_open) && !pre_roll_flushed_this_frame {
            self.push_stt_audio_bytes(mono_bytes).await?;
        }
        if !gate_stt || stt_poll_open {
            self.poll_stt_transcripts().await?;
        }

        if speech_end_transition {
            let mut inner = self.inner.lock().await;
            if !inner.agent_speaking {
                inner.barge_awaiting_stt_partial = false;
            }
        }

        if should_finalize_utterance {
            voice_debug(
                "STT should_finalize_utterance=true (gate hold done, agent idle, VAD not speaking)",
            );
            {
                let mut inner = self.inner.lock().await;
                inner.stt_endpoint_closing_started = true;
            }
            let tail_ms = {
                let inner = self.inner.lock().await;
                stt_endpoint_tail_ms(&inner.config.vad)
            };
            voice_debug(format!(
                "STT utterance close: endpoint tail {tail_ms} ms then finalize (speaking_end with final)"
            ));
            self.push_stt_endpoint_tail(tail_ms).await?;
            self.finalize_stt_utterance().await?;
            let (emit_stream_end, emit_stt_end, need_forced_final, forced_text) = {
                let mut inner = self.inner.lock().await;
                let stream = inner.stt_stream_open;
                let session = inner.user_stt_session_open;
                if inner.stt_stream_open {
                    inner.stt_stream_open = false;
                }
                if inner.user_stt_session_open {
                    inner.user_stt_session_open = false;
                }
                inner.stt_listen_deadline_ms = 0;
                inner.utterance_finalize_deadline_ms = 0;
                inner.defer_utterance_finalize_until_hold = false;
                inner.vad_triggered_this_utterance = false;
                let need_forced = !inner.stt_final_emitted_this_utterance
                    && inner.partials_emitted_this_utterance;
                let forced_text = inner.last_partial_text.clone().unwrap_or_default();
                (stream, session, need_forced, forced_text)
            };
            if emit_stream_end {
                self.emit(SpeechEvent::stt_stream_end());
            }
            if emit_stt_end {
                self.emit(SpeechEvent::user_stt_end());
            }
            if need_forced_final {
                let emit_speaking_end = {
                    let mut inner = self.inner.lock().await;
                    let emit_end = !inner.stt_speaking_end_emitted_this_utterance;
                    if emit_end {
                        inner.stt_speaking_end_emitted_this_utterance = true;
                    }
                    inner.stt_final_emitted_this_utterance = true;
                    inner.stt_finalize_pending = false;
                    inner.stt_endpoint_closing_started = false;
                    emit_end
                };
                if emit_speaking_end {
                    voice_debug("emit user_speaking_end (finalize without vendor final)");
                    self.emit(SpeechEvent::user_speaking_end());
                }
                voice_debug(format!(
                    "emit user_speech_final (last partial fallback): {}",
                    if forced_text.len() > 80 {
                        format!("{}…", &forced_text[..80])
                    } else {
                        forced_text.clone()
                    }
                ));
                self.emit(SpeechEvent::user_speech_final(forced_text));
            } else {
                let emit_speaking_end_without_final = {
                    let mut inner = self.inner.lock().await;
                    if inner.stt_speaking_end_emitted_this_utterance || inner.config.stt.is_some() {
                        false
                    } else {
                        inner.stt_speaking_end_emitted_this_utterance = true;
                        inner.stt_finalize_pending = false;
                        inner.stt_endpoint_closing_started = false;
                        true
                    }
                };
                if emit_speaking_end_without_final {
                    self.emit(SpeechEvent::user_speaking_end());
                }
            }
        }

        Ok(())
    }

    async fn push_stt_audio_bytes(&self, mono_bytes: Bytes) -> SpeechResult<()> {
        let mut stt = self.stt.lock().await;
        if let Some(stt) = stt.as_mut() {
            stt.push_audio(mono_bytes).await?;
        }
        Ok(())
    }

    /// Push trailing silence after VAD speech end so streaming STT vendors can detect endpoints.
    async fn push_stt_endpoint_tail(&self, tail_ms: u32) -> SpeechResult<()> {
        voice_debug(format!("STT endpoint tail: {tail_ms} ms silence"));
        const CHUNK_MS: u32 = 100;
        let chunks = tail_ms.div_ceil(CHUNK_MS);
        let chunk = crate::pcm::silence_mono_s16le_bytes(CHUNK_MS);
        for _ in 0..chunks {
            self.push_stt_audio_bytes(chunk.clone()).await?;
            self.poll_stt_transcripts().await?;
        }
        Ok(())
    }

    async fn finalize_stt_utterance(&self) -> SpeechResult<()> {
        voice_debug("STT finalize_utterance: vendor finalize + poll");
        {
            let mut stt = self.stt.lock().await;
            if let Some(stt) = stt.as_mut() {
                stt.finalize_utterance().await?;
            }
        }
        self.poll_stt_transcripts().await
    }

    async fn poll_stt_transcripts(&self) -> SpeechResult<()> {
        loop {
            let transcript = {
                let mut stt = self.stt.lock().await;
                let Some(stt) = stt.as_mut() else {
                    return Ok(());
                };
                stt.poll_transcript().await?
            };
            let Some(transcript) = transcript else {
                break;
            };
            match transcript {
                SttTranscript::Partial(text) => {
                    voice_debug(format!("STT partial: {text}"));
                    {
                        let mut inner = self.inner.lock().await;
                        inner.partials_emitted_this_utterance = true;
                        inner.last_partial_text = Some(text.clone());
                        inner.stt_listen_deadline_ms = 0;
                        Self::arm_utterance_finalize_timer(&mut inner);
                    }
                    self.emit_user_speaking_start_if_needed().await;
                    // Partial must precede barge_in in the event stream (semantic roundtrip E2E).
                    self.emit(SpeechEvent::user_speech_partial(text.clone()));
                    self.try_stt_gated_barge_in(&text).await?;
                }
                SttTranscript::Final(text) => {
                    voice_debug(format!("STT final: {text}"));
                    self.emit_user_speaking_start_if_needed().await;
                    self.try_stt_gated_barge_in(&text).await?;
                    let (emit_speaking_end, close_stream) = {
                        let mut inner = self.inner.lock().await;
                        inner.stt_final_emitted_this_utterance = true;
                        inner.stt_finalize_pending = false;
                        inner.stt_endpoint_closing_started = false;
                        inner.stt_listen_deadline_ms = 0;
                        inner.utterance_finalize_deadline_ms = 0;
                        inner.defer_utterance_finalize_until_hold = false;
                        inner.vad_triggered_this_utterance = false;
                        let emit_end = !inner.stt_speaking_end_emitted_this_utterance;
                        if emit_end {
                            inner.stt_speaking_end_emitted_this_utterance = true;
                        }
                        let close_stream = inner.stt_stream_open || inner.user_stt_session_open;
                        if inner.stt_stream_open {
                            inner.stt_stream_open = false;
                        }
                        if inner.user_stt_session_open {
                            inner.user_stt_session_open = false;
                        }
                        (emit_end, close_stream)
                    };
                    if close_stream {
                        self.emit(SpeechEvent::stt_stream_end());
                        self.emit(SpeechEvent::user_stt_end());
                    }
                    if emit_speaking_end {
                        voice_debug("emit user_speaking_end (paired with STT final)");
                        self.emit(SpeechEvent::user_speaking_end());
                    }
                    voice_debug(format!(
                        "emit user_speech_final: {}",
                        if text.len() > 80 {
                            format!("{}…", &text[..80])
                        } else {
                            text.clone()
                        }
                    ));
                    self.emit(SpeechEvent::user_speech_final(text));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn emit(&self, event: SpeechEvent) {
        let mode = self.events_mode();
        if matches!(
            mode,
            EventDeliveryMode::Callback | EventDeliveryMode::Stream | EventDeliveryMode::Both
        ) {
            self.event_bus.emit(event);
        }
    }

    async fn run_tts_drain(
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
    ) -> SpeechResult<()> {
        let writer = {
            let guard = inner.lock().await;
            guard
                .pcm_writer
                .clone()
                .ok_or(SpeechError::NotAttached)?
        };

        let drain_generation = tts_buffer.current_generation().await;
        let mut agent_start_emitted = false;

        while let Some(chunk) = tts_buffer.pop_chunk().await {
            if !agent_start_emitted {
                {
                    let mut guard = inner.lock().await;
                    guard.agent_speaking = true;
                    guard.agent_speaking_since = Some(Instant::now());
                    guard.stt_barge_fired_this_agent_playback = false;
                    guard.barge_awaiting_stt_partial = false;
                    // Drop any user-turn pre-roll so agent echo does not reach STT on barge.
                    guard.stt_pre_roll.as_mut().map(SttPreRollBuffer::clear);
                }
                event_bus.emit(SpeechEvent::agent_speaking_start());
                voice_debug("agent_speaking_start (first outbound PCM frame)");
                agent_start_emitted = true;
            }

            for (frame, duration_ms) in split_stereo_pcm_frames(&chunk.pcm, chunk.duration_ms) {
                if tts_buffer.current_generation().await != drain_generation {
                    voice_debug("TTS drain stopped (barge-in flush)");
                    let still_speaking = {
                        let guard = inner.lock().await;
                        guard.agent_speaking
                    };
                    if still_speaking {
                        Self::end_agent_speaking_inner(inner, false).await;
                        event_bus.emit(SpeechEvent::agent_speaking_end());
                    }
                    return Ok(());
                }
                writer(frame, duration_ms)?;
                tokio::time::sleep(std::time::Duration::from_millis(duration_ms as u64)).await;
            }
        }

        if agent_start_emitted {
            Self::end_agent_speaking_inner(inner, true).await;
            voice_debug("agent_speaking=false (TTS drained)");
            event_bus.emit(SpeechEvent::agent_speaking_end());
        }
        Ok(())
    }

    async fn end_agent_speaking_inner(inner: &Arc<Mutex<AgentInner>>, arm_stt_hold_after_playback: bool) {
        let mut guard = inner.lock().await;
        guard.agent_speaking = false;
        guard.agent_speaking_since = None;
        guard.barge_awaiting_stt_partial = false;
        guard.stt_barge_fired_this_agent_playback = false;
        if arm_stt_hold_after_playback {
            Self::arm_stt_hold_if_idle(&mut guard);
        }
    }
}

const STEREO_FRAME_20MS_BYTES: usize = 3840;

fn split_stereo_pcm_frames(pcm: &Bytes, total_duration_ms: u32) -> Vec<(Bytes, u32)> {
    if pcm.is_empty() {
        return Vec::new();
    }
    if pcm.len() <= STEREO_FRAME_20MS_BYTES {
        return vec![(pcm.clone(), total_duration_ms.max(1))];
    }

    let frame_count = pcm.len().div_ceil(STEREO_FRAME_20MS_BYTES);
    let mut frames = Vec::with_capacity(frame_count);
    for (index, frame) in pcm.chunks(STEREO_FRAME_20MS_BYTES).enumerate() {
        let duration_ms = if index + 1 == frame_count {
            total_duration_ms.saturating_sub(20 * index as u32).max(1)
        } else {
            20
        };
        frames.push((Bytes::copy_from_slice(frame), duration_ms));
    }
    frames
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
    use crate::pipeline::VendorFactory;

    struct NoopFactory;

    impl VendorFactory for NoopFactory {
        fn create_stt(&self, _config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
            Err(SpeechError::Config("noop".into()))
        }

        fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
            Err(SpeechError::Config("noop".into()))
        }
    }

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }

    #[tokio::test]
    async fn attach_requires_pcm_hooks() {
        let mut registry = VendorRegistry::new();
        registry.register_stt(SttVendor::Mock, Arc::new(NoopFactory));
        registry.register_tts(TtsVendor::Mock, Arc::new(NoopFactory));

        let config = VoiceAgentConfig {
            stt: None,
            tts: None,
            ..Default::default()
        };
        let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
        let reader: PcmReader = Arc::new(|| Ok(None));
        let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
        agent.attach(reader, writer).await.unwrap();
    }
}
