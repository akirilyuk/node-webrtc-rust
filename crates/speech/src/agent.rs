//! Voice agent orchestration (attach, start/stop, TTS injection, inbound PCM).
//!
//! [`VoiceAgent`] is the main entry point: one instance per WebRTC session. Inbound audio
//! is processed in [`VoiceAgent::process_inbound_pcm`]; the TypeScript SDK calls that from
//! `RemoteAudioTrack.readSample()` in a loop after [`VoiceAgent::start`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::Instant;

use bytes::Bytes;
use tokio::sync::{broadcast, Mutex, Notify};

use crate::config::{
    resolved_post_utterance_silence_ms, EventDeliveryMode, NoiseSuppressionProvider,
    SendTextToTtsOptions, VadConfig, VoiceAgentConfig, VoiceSessionContext,
};
use crate::error::{SpeechError, SpeechResult};
use crate::events::{SpeechEvent, SpeechEventBus};
use crate::otel;
use crate::pcm::i16_samples_to_bytes;
use crate::pipeline::{
    tts_stream_chunks_enabled, SttProvider, SttTranscript, TtsProgressiveSink, TtsProvider,
};
use crate::registry::VendorRegistry;
use crate::stt_pre_roll::SttPreRollBuffer;
use crate::tts_buffer::TtsBuffer;
use crate::vad::{handle_barge_in, VadEngine, VadTransition};
use node_webrtc_rust_denoise::Stereo48kRnnoise;

/// Callback invoked when PCM should be written to the outbound track.
pub type PcmWriter = Arc<dyn Fn(Bytes, u32) -> SpeechResult<()> + Send + Sync>;

/// Callback invoked to read inbound PCM from the attached remote track.
pub type PcmReader = Arc<dyn Fn() -> SpeechResult<Option<(Bytes, u32)>> + Send + Sync>;

static INBOUND_PCM_FRAMES: AtomicU64 = AtomicU64::new(0);

/// Decrements `tts_worker_tasks_alive` on task exit (including abort).
struct TtsWorkerAliveGuard(Arc<AtomicUsize>);

impl TtsWorkerAliveGuard {
    fn enter(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(counter))
    }
}

impl Drop for TtsWorkerAliveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

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

/// Prefer the inner `Internal` payload so callers see the original drain/writer text once.
fn playback_failure_message(error: &SpeechError) -> String {
    match error {
        SpeechError::Internal(message) => message.clone(),
        other => other.to_string(),
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
    otel: AgentOtelState,
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
    /// Wall-clock anchor for C2 when inbound PCM stops (see `c2_wall_clock_ticker`).
    utterance_finalize_armed_at: Option<Instant>,
    /// Last inbound frame wall time (real PCM received, including gate-closed skips).
    last_inbound_pcm_at: Option<Instant>,
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
    /// Bumped when TTS drain fails or playback wait times out so blocking jobs observe the error.
    tts_playback_failure_gen: u64,
    /// Original failure text for the latest playback abort (paired with `tts_playback_failure_gen`).
    tts_playback_last_error: Option<String>,
    pcm_writer: Option<PcmWriter>,
    pcm_reader: Option<PcmReader>,
    denoise: Option<Stereo48kRnnoise>,
}

/// Per-session OpenTelemetry state (public for the `otel` module).
pub struct AgentOtelState {
    pub session_context: VoiceSessionContext,
    #[cfg(feature = "otel")]
    pub session_span: Option<tracing::Span>,
    #[cfg(feature = "otel")]
    pub stt_vendor: Option<crate::config::SttVendor>,
    #[cfg(feature = "otel")]
    pub tts_vendor: Option<crate::config::TtsVendor>,
}

impl Default for AgentOtelState {
    fn default() -> Self {
        Self {
            session_context: VoiceSessionContext::default(),
            #[cfg(feature = "otel")]
            session_span: None,
            #[cfg(feature = "otel")]
            stt_vendor: None,
            #[cfg(feature = "otel")]
            tts_vendor: None,
        }
    }
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
    /// Set on barge/flush so progressive generators (Sherpa callback) can stop early.
    tts_generate_cancel: Arc<AtomicBool>,
    /// Set on stop/drop paths so synthesis/drain loops exit (no detached tasks).
    tts_workers_shutdown: Arc<AtomicBool>,
    /// Wakes workers blocked in `notified()` so stop cannot hang on idle wait.
    tts_workers_shutdown_wake: Arc<Notify>,
    /// True when a worker join timed out (blocked vendor) — host should recycle.
    tts_shutdown_unhealthy: Arc<AtomicBool>,
    /// Live synthesis + drain tokio tasks (tests / capacity diagnostics).
    tts_worker_tasks_alive: Arc<AtomicUsize>,
    /// In-flight async vendor `synthesize` calls (may outlive aborted Tokio workers).
    tts_vendor_calls_inflight: Arc<AtomicUsize>,
    weak_self: Weak<VoiceAgent>,
    c2_ticker_started: AtomicBool,
}

impl VoiceAgent {
    /// Builds agents with STT/TTS from `registry` and VAD/pre-roll from `config.vad`.
    pub fn new(config: VoiceAgentConfig, registry: Arc<VendorRegistry>) -> SpeechResult<Arc<Self>> {
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

        let denoise = match config.noise_suppression.provider {
            NoiseSuppressionProvider::Rnnoise => Some(Stereo48kRnnoise::new()),
            NoiseSuppressionProvider::None => None,
        };

        Ok(Arc::new_cyclic(|weak| Self {
            event_bus: SpeechEventBus::new(),
            tts_buffer: TtsBuffer::new(),
            registry,
            inner: Arc::new(Mutex::new(AgentInner {
                config,
                attached: false,
                running: false,
                otel: AgentOtelState::default(),
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
                utterance_finalize_armed_at: None,
                last_inbound_pcm_at: None,
                defer_utterance_finalize_until_hold: false,
                last_partial_text: None,
                partials_emitted_this_utterance: false,
                barge_awaiting_stt_partial: false,
                stt_barge_fired_this_agent_playback: false,
                tts_playback_failure_gen: 0,
                tts_playback_last_error: None,
                pcm_writer: None,
                pcm_reader: None,
                denoise,
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
            tts_generate_cancel: Arc::new(AtomicBool::new(false)),
            tts_workers_shutdown: Arc::new(AtomicBool::new(false)),
            tts_workers_shutdown_wake: Arc::new(Notify::new()),
            tts_shutdown_unhealthy: Arc::new(AtomicBool::new(false)),
            tts_worker_tasks_alive: Arc::new(AtomicUsize::new(0)),
            tts_vendor_calls_inflight: Arc::new(AtomicUsize::new(0)),
            weak_self: weak.clone(),
            c2_ticker_started: AtomicBool::new(false),
        }))
    }

    /// Number of live TTS synthesis/drain tokio tasks (0 when workers have exited).
    /// Does **not** include still-running `spawn_blocking` vendor work after abort.
    pub fn tts_worker_tasks_alive(&self) -> usize {
        self.tts_worker_tasks_alive.load(Ordering::SeqCst)
    }

    /// In-flight vendor `synthesize` calls observed by the agent (async layer).
    pub fn tts_vendor_calls_inflight(&self) -> usize {
        self.tts_vendor_calls_inflight.load(Ordering::SeqCst)
    }

    /// True when stop could not join a TTS worker within the bound (recycle signal).
    pub fn is_tts_shutdown_unhealthy(&self) -> bool {
        self.tts_shutdown_unhealthy.load(Ordering::SeqCst)
    }

    fn clear_utterance_finalize_timer(inner: &mut AgentInner) {
        inner.utterance_finalize_deadline_ms = 0;
        inner.utterance_finalize_armed_at = None;
        inner.defer_utterance_finalize_until_hold = false;
    }

    fn vad_is_speaking(inner: &AgentInner) -> bool {
        inner
            .vad
            .as_ref()
            .map(VadEngine::is_speaking)
            .unwrap_or(false)
    }

    /// C2 applies only after the user turn has ended: hold drained and VAD not in speech.
    fn c2_end_of_turn_ready(inner: &AgentInner) -> bool {
        if !inner.partials_emitted_this_utterance || inner.stt_final_emitted_this_utterance {
            return false;
        }
        if inner.defer_utterance_finalize_until_hold || inner.stt_gate_hold_ms > 0 {
            return false;
        }
        !Self::vad_is_speaking(inner)
    }

    fn disarm_utterance_finalize_timer(inner: &mut AgentInner) {
        inner.utterance_finalize_deadline_ms = 0;
        inner.utterance_finalize_armed_at = None;
    }

    fn arm_utterance_finalize_timer(inner: &mut AgentInner) {
        if !Self::c2_end_of_turn_ready(inner) {
            Self::disarm_utterance_finalize_timer(inner);
            return;
        }
        inner.utterance_finalize_deadline_ms = inner.config.vad.utterance_finalize_timeout_ms;
        inner.utterance_finalize_armed_at = Some(Instant::now());
        voice_debug(format!(
            "utterance finalize timer: {} ms (end-of-turn)",
            inner.utterance_finalize_deadline_ms
        ));
    }

    fn refresh_utterance_finalize_after_partial(inner: &mut AgentInner) {
        if Self::c2_end_of_turn_ready(inner) {
            Self::arm_utterance_finalize_timer(inner);
        } else {
            Self::disarm_utterance_finalize_timer(inner);
        }
    }

    fn ensure_c2_wall_clock_ticker(self: &Arc<Self>) {
        if self
            .c2_ticker_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let agent = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let Some(this) = agent.weak_self.upgrade() else {
                    break;
                };
                let running = {
                    let inner = this.inner.lock().await;
                    inner.running
                };
                if !running {
                    break;
                }
                if let Err(error) = this.c2_wall_clock_tick().await {
                    voice_debug(format!("C2 wall-clock tick error: {error}"));
                }
            }
        });
    }

    async fn c2_wall_clock_tick(&self) -> SpeechResult<()> {
        let should_force = {
            let inner = self.inner.lock().await;
            if !inner.running || inner.utterance_finalize_deadline_ms == 0 {
                return Ok(());
            }
            if inner.defer_utterance_finalize_until_hold || inner.stt_gate_hold_ms > 0 {
                return Ok(());
            }
            if Self::vad_is_speaking(&inner) {
                return Ok(());
            }
            let Some(armed_at) = inner.utterance_finalize_armed_at else {
                return Ok(());
            };
            let timeout_ms = inner.config.vad.utterance_finalize_timeout_ms as u64;
            if armed_at.elapsed() < std::time::Duration::from_millis(timeout_ms) {
                return Ok(());
            }
            // Safety net when RTP/inbound loop stops: only force if PCM actually stalled.
            let pcm_stalled = inner
                .last_inbound_pcm_at
                .map(|t| t.elapsed() >= std::time::Duration::from_millis(timeout_ms))
                .unwrap_or(true);
            pcm_stalled
        };
        if should_force {
            voice_debug("C2 wall-clock timeout (inbound PCM stalled after end-of-turn grace)");
            self.force_close_utterance().await?;
        }
        Ok(())
    }

    fn invalidate_inflight_tts_synthesis(&self) {
        self.tts_synthesis_epoch.fetch_add(1, Ordering::SeqCst);
        self.tts_generate_cancel.store(true, Ordering::SeqCst);
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
    pub async fn attach(&self, pcm_reader: PcmReader, pcm_writer: PcmWriter) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        inner.pcm_reader = Some(pcm_reader);
        inner.pcm_writer = Some(pcm_writer);
        inner.attached = true;
        Ok(())
    }

    /// Starts STT vendor and TTS drain worker. Inbound PCM is driven by the host via [`process_inbound_pcm`](Self::process_inbound_pcm).
    ///
    /// Optional `session_context` carries session labels and W3C `traceparent` for OpenTelemetry
    /// when the `otel` Cargo feature is enabled.
    pub async fn start(&self, session_context: Option<VoiceSessionContext>) -> SpeechResult<()> {
        {
            let mut inner = self.inner.lock().await;
            if !inner.attached {
                return Err(SpeechError::NotAttached);
            }
            if inner.running {
                return Err(SpeechError::AlreadyRunning);
            }
            inner.running = true;
            let stt_vendor = inner.config.stt.as_ref().map(|cfg| cfg.provider);
            let tts_vendor = inner.config.tts.as_ref().map(|cfg| cfg.provider);
            otel::begin_session(
                &mut inner.otel,
                session_context.unwrap_or_default(),
                stt_vendor,
                tts_vendor,
            );
        }

        voice_debug("VoiceAgent running=true");

        self.tts_workers_shutdown.store(false, Ordering::SeqCst);
        self.tts_shutdown_unhealthy.store(false, Ordering::SeqCst);

        let mut stt = self.stt.lock().await;
        if let Some(stt) = stt.as_mut() {
            stt.start().await?;
            voice_debug(format!("STT started ({})", stt.vendor_name()));
        }
        {
            let ctx = self.inner.lock().await.otel.session_context.clone();
            if let Some(tts) = self.tts.lock().await.as_ref() {
                tts.bind_session_context(&ctx);
            }
        }
        self.ensure_tts_drain_worker().await;
        if let Some(this) = self.weak_self.upgrade() {
            this.ensure_c2_wall_clock_ticker();
        }
        Ok(())
    }

    async fn ensure_tts_drain_worker(&self) {
        Self::ensure_tts_drain_worker_shared(
            &self.tts_drain_worker,
            &self.tts_drain_wake,
            &self.tts_buffer,
            &self.inner,
            &self.event_bus,
            &self.tts_workers_shutdown,
            &self.tts_workers_shutdown_wake,
            &self.tts_worker_tasks_alive,
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
        let was_running = {
            let mut inner = self.inner.lock().await;
            if !inner.running {
                // Idempotent diagnostic: prior unhealthy stop remains visible.
                if self.tts_shutdown_unhealthy.load(Ordering::SeqCst) {
                    return Err(SpeechError::TtsShutdownUnhealthy);
                }
                return Err(SpeechError::NotRunning);
            }
            inner.running = false;
            otel::end_session(&mut inner.otel);
            true
        };
        let _ = was_running;

        // Cancel producers, wake consumers, join workers within a bound (no detached tasks).
        self.tts_workers_shutdown.store(true, Ordering::SeqCst);
        self.cancel_pending_tts_synthesis().await;
        self.tts_buffer.flush().await;
        self.tts_synthesis_wake.notify_waiters();
        self.tts_drain_wake.notify_waiters();
        self.tts_workers_shutdown_wake.notify_waiters();

        let synth = self.tts_synthesis_worker.lock().await.take();
        let drain = self.tts_drain_worker.lock().await.take();
        // Join in parallel so drain pacing cannot stall synthesis join (and vice versa).
        let (synth_join, drain_join) = tokio::join!(
            Self::join_tts_worker_bounded(synth, "synthesis", &self.tts_shutdown_unhealthy),
            Self::join_tts_worker_bounded(drain, "drain", &self.tts_shutdown_unhealthy),
        );
        let _ = (synth_join, drain_join);

        let stt_result = {
            let mut stt = self.stt.lock().await;
            if let Some(stt) = stt.as_mut() {
                stt.stop().await
            } else {
                Ok(())
            }
        };

        voice_debug(format!(
            "VoiceAgent stopped workers_alive={} vendor_inflight={} unhealthy={}",
            self.tts_worker_tasks_alive(),
            self.tts_vendor_calls_inflight(),
            self.is_tts_shutdown_unhealthy()
        ));

        // Prefer recycle signal over STT stop errors so the JS host quarantines.
        if self.tts_shutdown_unhealthy.load(Ordering::SeqCst) {
            return Err(SpeechError::TtsShutdownUnhealthy);
        }
        stt_result
    }

    async fn join_tts_worker_bounded(
        handle: Option<tokio::task::JoinHandle<()>>,
        name: &str,
        unhealthy: &AtomicBool,
    ) {
        let Some(mut handle) = handle else {
            return;
        };
        const JOIN_BOUND: std::time::Duration = std::time::Duration::from_millis(2_000);
        tokio::select! {
            result = &mut handle => {
                if let Err(err) = result {
                    voice_debug(format!("TTS {name} worker join error: {err}"));
                }
            }
            _ = tokio::time::sleep(JOIN_BOUND) => {
                handle.abort();
                let _ = handle.await;
                unhealthy.store(true, Ordering::SeqCst);
                voice_debug(format!(
                    "TTS {name} worker join timed out after {}ms — marked shutdown unhealthy (recycle)",
                    JOIN_BOUND.as_millis()
                ));
            }
        }
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
        if self.tts_workers_shutdown.load(Ordering::SeqCst) {
            return;
        }
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
        let generate_cancel = Arc::clone(&self.tts_generate_cancel);
        let shutdown = Arc::clone(&self.tts_workers_shutdown);
        let shutdown_wake = Arc::clone(&self.tts_workers_shutdown_wake);
        let alive = Arc::clone(&self.tts_worker_tasks_alive);
        let vendor_inflight = Arc::clone(&self.tts_vendor_calls_inflight);
        let drain_shutdown = Arc::clone(&self.tts_workers_shutdown);
        let drain_alive = Arc::clone(&self.tts_worker_tasks_alive);
        *slot = Some(tokio::spawn(async move {
            let _alive_guard = TtsWorkerAliveGuard::enter(&alive);
            loop {
                // Level-triggered shutdown poll: notify_waiters is edge-triggered and
                // can be lost while the worker is inside a vendor call / drain pass.
                tokio::select! {
                    _ = wake.notified() => {}
                    _ = shutdown_wake.notified() => {}
                    _ = async {
                        while !shutdown.load(Ordering::SeqCst) {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                    } => {}
                }
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                loop {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }
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
                        &generate_cancel,
                        &drain_shutdown,
                        &shutdown_wake,
                        &drain_alive,
                        &vendor_inflight,
                    )
                    .await;
                    synthesis_busy.store(false, Ordering::SeqCst);

                    if let Some(done) = job.done {
                        let _ = done.send(result);
                    } else if let Err(error) = result {
                        voice_debug(format!("non-blocking TTS synthesis error: {error}"));
                    }
                }
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
            }
            // Cancel any leftovers so waiters are not stranded.
            let mut pending = queue.lock().await;
            for job in pending.drain(..) {
                if let Some(done) = job.done {
                    let _ = done.send(Err(SpeechError::Internal("TTS cancelled".into())));
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
        generate_cancel: &Arc<AtomicBool>,
        tts_workers_shutdown: &Arc<AtomicBool>,
        tts_workers_shutdown_wake: &Arc<Notify>,
        tts_worker_tasks_alive: &Arc<AtomicUsize>,
        tts_vendor_calls_inflight: &Arc<AtomicUsize>,
    ) -> SpeechResult<()> {
        if tts_stream_chunks_enabled() {
            Self::run_tts_synthesis_job_streaming(
                text,
                tts,
                tts_buffer,
                tts_drain_wake,
                tts_drain_worker,
                inner,
                event_bus,
                synthesis_epoch,
                generate_cancel,
                tts_workers_shutdown,
                tts_workers_shutdown_wake,
                tts_worker_tasks_alive,
                tts_vendor_calls_inflight,
            )
            .await
        } else {
            Self::run_tts_synthesis_job_buffered(
                text,
                tts,
                tts_buffer,
                tts_drain_wake,
                tts_drain_worker,
                inner,
                event_bus,
                synthesis_epoch,
                tts_workers_shutdown,
                tts_workers_shutdown_wake,
                tts_worker_tasks_alive,
                tts_vendor_calls_inflight,
            )
            .await
        }
    }

    /// Legacy path: fully synthesize, then enqueue all PCM, then drain.
    /// Enabled with `VOICE_TTS_STREAM_CHUNKS=0`.
    async fn run_tts_synthesis_job_buffered(
        text: &str,
        tts: &Arc<Mutex<Option<Box<dyn TtsProvider>>>>,
        tts_buffer: &TtsBuffer,
        tts_drain_wake: &Arc<Notify>,
        tts_drain_worker: &Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        synthesis_epoch: &Arc<AtomicU64>,
        tts_workers_shutdown: &Arc<AtomicBool>,
        tts_workers_shutdown_wake: &Arc<Notify>,
        tts_worker_tasks_alive: &Arc<AtomicUsize>,
        tts_vendor_calls_inflight: &Arc<AtomicUsize>,
    ) -> SpeechResult<()> {
        let epoch_at_start = synthesis_epoch.load(Ordering::SeqCst);
        let generation_at_start = tts_buffer.current_generation().await;
        let tts_started = Instant::now();
        let chunks = {
            let tts_guard = tts.lock().await;
            let provider = tts_guard
                .as_ref()
                .ok_or_else(|| SpeechError::Config("TTS not configured".into()))?;
            let (ctx, tts_vendor) = {
                let inner_guard = inner.lock().await;
                (
                    inner_guard.otel.session_context.clone(),
                    inner_guard.config.tts.as_ref().map(|cfg| cfg.provider),
                )
            };
            {
                let _span = otel::voice_span(
                    "voice.tts",
                    &ctx,
                    tts_vendor.map(crate::config::TtsVendor::as_str),
                );
            }
            tts_vendor_calls_inflight.fetch_add(1, Ordering::SeqCst);
            let synthesize_result = provider.synthesize(text).await;
            tts_vendor_calls_inflight.fetch_sub(1, Ordering::SeqCst);
            synthesize_result?
        };
        Self::record_tts_job_latency(inner, tts_started).await;

        if synthesis_epoch.load(Ordering::SeqCst) != epoch_at_start {
            voice_debug("TTS synthesis discarded (invalidated during synthesize)");
            return Ok(());
        }

        if chunks.is_empty() {
            return Ok(());
        }

        let failure_gen_at_start = {
            let guard = inner.lock().await;
            guard.tts_playback_failure_gen
        };

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
            tts_workers_shutdown,
            tts_workers_shutdown_wake,
            tts_worker_tasks_alive,
        )
        .await;
        tts_drain_wake.notify_one();
        Self::wait_job_playback_idle(tts_buffer, inner, event_bus, failure_gen_at_start).await
    }

    /// Default path: stream PCM chunks into the buffer while synthesis runs so
    /// drain can start before the full utterance is ready (`VOICE_TTS_STREAM_CHUNKS`).
    async fn run_tts_synthesis_job_streaming(
        text: &str,
        tts: &Arc<Mutex<Option<Box<dyn TtsProvider>>>>,
        tts_buffer: &TtsBuffer,
        tts_drain_wake: &Arc<Notify>,
        tts_drain_worker: &Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        synthesis_epoch: &Arc<AtomicU64>,
        generate_cancel: &Arc<AtomicBool>,
        tts_workers_shutdown: &Arc<AtomicBool>,
        tts_workers_shutdown_wake: &Arc<Notify>,
        tts_worker_tasks_alive: &Arc<AtomicUsize>,
        tts_vendor_calls_inflight: &Arc<AtomicUsize>,
    ) -> SpeechResult<()> {
        let epoch_at_start = synthesis_epoch.load(Ordering::SeqCst);
        let generation_at_start = tts_buffer.current_generation().await;
        // Capture before drain can abort this job's PCM (streaming starts drain early).
        let failure_gen_at_start = {
            let guard = inner.lock().await;
            guard.tts_playback_failure_gen
        };
        generate_cancel.store(false, Ordering::SeqCst);

        Self::ensure_tts_drain_worker_shared(
            tts_drain_worker,
            tts_drain_wake,
            tts_buffer,
            inner,
            event_bus,
            tts_workers_shutdown,
            tts_workers_shutdown_wake,
            tts_worker_tasks_alive,
        )
        .await;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TtsProgressiveSink {
            tx,
            cancel: Arc::clone(generate_cancel),
        };

        tts_buffer.set_producing(true).await;
        tts_drain_wake.notify_one();

        let enqueue_buffer = tts_buffer.clone();
        let enqueue_wake = Arc::clone(tts_drain_wake);
        let enqueue_task = tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                if !enqueue_buffer
                    .enqueue_if_generation(vec![chunk], Some(generation_at_start))
                    .await
                {
                    voice_debug("TTS progressive chunk discarded (buffer flushed)");
                    break;
                }
                enqueue_wake.notify_one();
            }
        });

        let tts_started = Instant::now();
        let synth_result = {
            let tts_guard = tts.lock().await;
            let provider = tts_guard
                .as_ref()
                .ok_or_else(|| SpeechError::Config("TTS not configured".into()))?;
            let (ctx, tts_vendor) = {
                let inner_guard = inner.lock().await;
                (
                    inner_guard.otel.session_context.clone(),
                    inner_guard.config.tts.as_ref().map(|cfg| cfg.provider),
                )
            };
            {
                let _span = otel::voice_span(
                    "voice.tts",
                    &ctx,
                    tts_vendor.map(crate::config::TtsVendor::as_str),
                );
            }
            tts_vendor_calls_inflight.fetch_add(1, Ordering::SeqCst);
            let result = provider.synthesize_progressive(text, Some(sink)).await;
            tts_vendor_calls_inflight.fetch_sub(1, Ordering::SeqCst);
            result
        };
        Self::record_tts_job_latency(inner, tts_started).await;

        // Dropping the last sender happens when synthesize_progressive returns
        // (sink moved into the call). Wait for the enqueue task to drain.
        let _ = enqueue_task.await;
        tts_buffer.set_producing(false).await;
        tts_drain_wake.notify_one();

        match synth_result {
            Ok(chunks) => {
                if synthesis_epoch.load(Ordering::SeqCst) != epoch_at_start {
                    voice_debug("TTS streaming synthesis discarded (invalidated)");
                    return Ok(());
                }
                if chunks.is_empty()
                    && !tts_buffer.is_speaking().await
                    && tts_buffer.pending_count().await == 0
                {
                    return Ok(());
                }
                Self::wait_job_playback_idle(tts_buffer, inner, event_bus, failure_gen_at_start)
                    .await
            }
            Err(error) => {
                tts_buffer.set_producing(false).await;
                tts_drain_wake.notify_one();
                Err(error)
            }
        }
    }

    async fn record_tts_job_latency(inner: &Arc<Mutex<AgentInner>>, started: Instant) {
        let tts_attrs = {
            let guard = inner.lock().await;
            let project_id = guard
                .otel
                .session_context
                .project_id
                .as_deref()
                .unwrap_or("");
            guard
                .config
                .tts
                .as_ref()
                .map(|cfg| otel::SherpaTtsMetricAttrs::from_tts_config(cfg, project_id))
                .unwrap_or_default()
        };
        otel::record_tts_latency_ms(started.elapsed().as_secs_f64() * 1000.0, &tts_attrs);
    }

    async fn ensure_tts_drain_worker_shared(
        slot: &Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
        wake: &Arc<Notify>,
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        shutdown: &Arc<AtomicBool>,
        shutdown_wake: &Arc<Notify>,
        alive: &Arc<AtomicUsize>,
    ) {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        let mut guard = slot.lock().await;
        if guard.is_some() {
            return;
        }
        let wake = Arc::clone(wake);
        let tts_buffer = tts_buffer.clone();
        let inner = Arc::clone(inner);
        let event_bus = event_bus.clone();
        let shutdown = Arc::clone(shutdown);
        let shutdown_wake = Arc::clone(shutdown_wake);
        let alive = Arc::clone(alive);
        *guard = Some(tokio::spawn(async move {
            let _alive_guard = TtsWorkerAliveGuard::enter(&alive);
            loop {
                tokio::select! {
                    _ = wake.notified() => {}
                    _ = shutdown_wake.notified() => {}
                    _ = async {
                        while !shutdown.load(Ordering::SeqCst) {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                    } => {}
                }
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                if let Err(error) = VoiceAgent::run_tts_drain(&tts_buffer, &inner, &event_bus).await
                {
                    voice_debug(format!("TTS drain error: {error}"));
                    let message = playback_failure_message(&error);
                    VoiceAgent::abort_tts_playback_failure(
                        &tts_buffer,
                        &inner,
                        &event_bus,
                        message,
                    )
                    .await;
                }
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
            }
        }));
    }

    /// Flush buffered TTS, reset speaking state, emit error (+ speaking_end when needed), and
    /// publish a failure generation so blocking waiters observe the original error.
    async fn abort_tts_playback_failure(
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        message: impl Into<String>,
    ) {
        let message = message.into();
        // Record failure before clearing idle flags so waiters never treat abort as success.
        let emit_speaking_end = {
            let mut guard = inner.lock().await;
            guard.tts_playback_failure_gen = guard.tts_playback_failure_gen.wrapping_add(1);
            guard.tts_playback_last_error = Some(message.clone());
            let was_speaking = guard.agent_speaking;
            guard.agent_speaking = false;
            guard.agent_speaking_since = None;
            guard.barge_awaiting_stt_partial = false;
            guard.stt_barge_fired_this_agent_playback = false;
            was_speaking
        };
        tts_buffer.flush().await;
        event_bus.emit(SpeechEvent::error(message.clone()));
        if emit_speaking_end {
            event_bus.emit(SpeechEvent::agent_speaking_end());
        }
        voice_debug(format!(
            "TTS playback aborted: {message} (speaking_end={emit_speaking_end})"
        ));
    }

    async fn wait_job_playback_idle(
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        failure_gen_at_start: u64,
    ) -> SpeechResult<()> {
        let deadline = Instant::now() + std::time::Duration::from_secs(45);
        loop {
            {
                let guard = inner.lock().await;
                // stop() clears running before joining workers — do not block shutdown.
                if !guard.running {
                    return Ok(());
                }
                if guard.tts_playback_failure_gen != failure_gen_at_start {
                    let message = guard
                        .tts_playback_last_error
                        .clone()
                        .unwrap_or_else(|| "TTS playback failed".into());
                    return Err(SpeechError::Internal(message));
                }
                let agent_speaking = guard.agent_speaking;
                drop(guard);
                let queued = tts_buffer.is_speaking().await;
                if !agent_speaking && !queued {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                let message = "timed out waiting for TTS job playback";
                Self::abort_tts_playback_failure(tts_buffer, inner, event_bus, message).await;
                return Err(SpeechError::Internal(message.into()));
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
        let min_tokens = inner.config.vad.barge_in.min_stt_partial_tokens.max(1) as usize;
        crate::config::stt_partial_token_count(text) >= min_tokens
    }

    async fn try_stt_gated_barge_in(&self, partial_text: &str) -> SpeechResult<()> {
        let (should_barge, barge_in) = {
            let inner = self.inner.lock().await;
            let barge_in_cfg = &inner.config.vad.barge_in;
            if !barge_in_cfg.enabled
                || !barge_in_cfg.require_stt_partial
                || inner.config.stt.is_none()
            {
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
            (true, barge_in_cfg.clone())
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
        {
            let ctx = self.inner.lock().await.otel.session_context.clone();
            otel::record_barge_in(&ctx);
        }
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
                Self::clear_utterance_finalize_timer(&mut inner);
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
                voice_debug("barge-in deferred until qualifying STT partial (require_stt_partial)");
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
            Self::clear_utterance_finalize_timer(&mut inner);
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
            Self::clear_utterance_finalize_timer(&mut inner);
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
            let needed = inner.stt_finalize_pending && !inner.stt_final_emitted_this_utterance;
            (
                needed,
                stt_endpoint_tail_ms(&inner.config.vad),
                inner.stt_endpoint_closing_started,
            )
        };
        if !needed {
            return Ok(());
        }
        voice_debug("STT: completing previous utterance (pending finalize before new speech)");
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
        Self::clear_utterance_finalize_timer(inner);
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
            voice_debug(format!(
                "process_inbound_pcm call={call} skipped: agent not running"
            ));
            return Ok(());
        }

        {
            let mut inner = self.inner.lock().await;
            inner.last_inbound_pcm_at = Some(Instant::now());
        }

        let pcm = {
            let mut inner = self.inner.lock().await;
            if let Some(denoiser) = inner.denoise.as_mut() {
                Bytes::from(denoiser.process_s16le_stereo(pcm.as_ref()))
            } else {
                pcm
            }
        };

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
            let vad_speaking = inner.vad.as_ref().map(|v| v.is_speaking()).unwrap_or(false);

            if gate_stt {
                let barge_listen = inner.agent_speaking && !inner.stt_stream_open;
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
                    let hold_ms = inner.config.vad.stt_gate_hold_ms;
                    inner.stt_gate_hold_ms = hold_ms;
                    let ctx = inner.otel.session_context.clone();
                    otel::record_gate_hold_start(&ctx, hold_ms);
                    inner.stt_finalize_pending = true;
                    if inner.partials_emitted_this_utterance
                        && !inner.stt_final_emitted_this_utterance
                    {
                        inner.defer_utterance_finalize_until_hold = true;
                        inner.utterance_finalize_deadline_ms = 0;
                        inner.utterance_finalize_armed_at = None;
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
                        inner.defer_utterance_finalize_until_hold = false;
                        Self::disarm_utterance_finalize_timer(&mut inner);
                        voice_debug(
                            "STT gate hold cancelled: voice active again before hold expired",
                        );
                    }
                } else {
                    let before = inner.stt_gate_hold_ms;
                    inner.stt_gate_hold_ms = before.saturating_sub(duration_ms);
                    let after = inner.stt_gate_hold_ms;
                    if before > 0 && after == 0 {
                        let ctx = inner.otel.session_context.clone();
                        otel::record_gate_hold_end(&ctx);
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
                    } else if before > 0 && after > 0 && (before / 500) != (after / 500) {
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
                    && !Self::vad_is_speaking(&inner)
                    && inner.stt_gate_hold_ms == 0
                {
                    inner.utterance_finalize_deadline_ms = inner
                        .utterance_finalize_deadline_ms
                        .saturating_sub(duration_ms);
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

        {
            let ctx = self.inner.lock().await.otel.session_context.clone();
            for transition in &transitions {
                otel::record_vad_transition(&ctx, transition);
            }
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
                if long_pause_new_phrase || inner.stt_final_emitted_this_utterance {
                    // A completed final always starts a new utterance — the brief-gap path is only
                    // for mid-phrase pauses (e.g. counting). Leaving `stt_final_emitted` set blocks
                    // C2 arming and `should_finalize_utterance` on turn 2+ (local multi-turn E2E).
                    Self::reset_utterance_state_for_new_speech(&mut inner);
                } else {
                    // Brief gap (e.g. counting): same utterance — clear hold only.
                    inner.stt_gate_hold_ms = 0;
                    inner.stt_finalize_pending = false;
                    inner.stt_endpoint_closing_started = false;
                    inner.defer_utterance_finalize_until_hold = false;
                    Self::disarm_utterance_finalize_timer(&mut inner);
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
                    vad_speaking || inner.stt_gate_hold_ms > 0 || pending_gate || utterance_closing
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
        let gate_closed_skip_stt = gate_stt && !stt_poll_open && !should_finalize_utterance;
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
                Self::clear_utterance_finalize_timer(&mut inner);
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
        let stt_started = Instant::now();
        let (ctx, stt_vendor, stt_attrs) = {
            let inner = self.inner.lock().await;
            let project_id = inner
                .otel
                .session_context
                .project_id
                .as_deref()
                .unwrap_or("");
            let stt_attrs = inner
                .config
                .stt
                .as_ref()
                .map(|cfg| otel::SttMetricAttrs::from_stt_config(cfg, project_id))
                .unwrap_or_default();
            (
                inner.otel.session_context.clone(),
                inner.config.stt.as_ref().map(|cfg| cfg.provider),
                stt_attrs,
            )
        };
        {
            let _span = otel::voice_span(
                "voice.stt",
                &ctx,
                stt_vendor.map(crate::config::SttVendor::as_str),
            );
        }
        {
            let mut stt = self.stt.lock().await;
            if let Some(stt) = stt.as_mut() {
                stt.finalize_utterance().await?;
            }
        }
        self.poll_stt_transcripts().await?;
        otel::record_stt_latency_ms(stt_started.elapsed().as_secs_f64() * 1000.0, &stt_attrs);
        Ok(())
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
                        Self::refresh_utterance_finalize_after_partial(&mut inner);
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
                        Self::clear_utterance_finalize_timer(&mut inner);
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
            guard.pcm_writer.clone().ok_or(SpeechError::NotAttached)?
        };

        let drain_generation = tts_buffer.current_generation().await;
        // Resume mid-utterance drain passes without re-emitting agent_speaking_start.
        let mut agent_start_emitted = {
            let guard = inner.lock().await;
            guard.agent_speaking
        };
        let mut played_any = false;
        // Carry incomplete frames across progressive chunks / drain wakeups.
        // Do not pad mid-utterance — that inserts silence and drops STT words.
        let mut frame_carry = tts_buffer.take_frame_carry().await;

        loop {
            let Some(chunk) = tts_buffer.pop_chunk().await else {
                // Progressive synth may still be producing — wait for the next wake
                // instead of ending the utterance early.
                if tts_buffer.is_producing().await
                    && tts_buffer.current_generation().await == drain_generation
                {
                    tts_buffer.store_frame_carry(frame_carry).await;
                    voice_debug("TTS drain paused (awaiting more progressive chunks)");
                    return Ok(());
                }
                break;
            };

            frame_carry.extend_from_slice(&chunk.pcm);
            let frames = take_complete_stereo_frames(&mut frame_carry);
            if frames.is_empty() {
                continue;
            }

            match Self::write_paced_tts_frames(
                &writer,
                tts_buffer,
                inner,
                event_bus,
                drain_generation,
                &mut agent_start_emitted,
                &mut played_any,
                frames,
            )
            .await?
            {
                TtsDrainWrite::Continued => {}
                TtsDrainWrite::Stopped => return Ok(()),
            }
        }

        // End of utterance — pad any trailing partial frame once (same as buffered path).
        if !frame_carry.is_empty()
            && !tts_buffer.is_producing().await
            && tts_buffer.current_generation().await == drain_generation
        {
            let (frame, duration_ms) = pad_stereo_frame_20ms(&frame_carry);
            frame_carry.clear();
            match Self::write_paced_tts_frames(
                &writer,
                tts_buffer,
                inner,
                event_bus,
                drain_generation,
                &mut agent_start_emitted,
                &mut played_any,
                vec![(frame, duration_ms)],
            )
            .await?
            {
                TtsDrainWrite::Continued => {}
                TtsDrainWrite::Stopped => return Ok(()),
            }
        }
        tts_buffer.store_frame_carry(frame_carry).await;

        if played_any && agent_start_emitted && !tts_buffer.is_producing().await {
            Self::end_agent_speaking_inner(inner, true).await;
            voice_debug("agent_speaking=false (TTS drained)");
            event_bus.emit(SpeechEvent::agent_speaking_end());

            let silence_ms = {
                let guard = inner.lock().await;
                if !guard.running {
                    return Ok(());
                }
                resolved_post_utterance_silence_ms(&guard.config)
            };
            if silence_ms > 0 {
                Self::stream_post_utterance_silence(
                    &writer,
                    tts_buffer,
                    inner,
                    drain_generation,
                    silence_ms,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Write paced 20 ms frames. Writer failures propagate; barge/stop returns [`TtsDrainWrite::Stopped`].
    async fn write_paced_tts_frames(
        writer: &PcmWriter,
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        drain_generation: u64,
        agent_start_emitted: &mut bool,
        played_any: &mut bool,
        frames: Vec<(Bytes, u32)>,
    ) -> SpeechResult<TtsDrainWrite> {
        for (frame, duration_ms) in frames {
            *played_any = true;
            if !*agent_start_emitted {
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
                *agent_start_emitted = true;
            }

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
                return Ok(TtsDrainWrite::Stopped);
            }
            writer(frame, duration_ms)?;
            if !Self::pace_tts_drain_frame_while_running(
                tts_buffer,
                inner,
                drain_generation,
                duration_ms,
            )
            .await
            {
                voice_debug("TTS drain stopped during frame pacing (barge-in flush / stop)");
                let still_speaking = {
                    let guard = inner.lock().await;
                    guard.agent_speaking
                };
                if still_speaking {
                    Self::end_agent_speaking_inner(inner, false).await;
                    event_bus.emit(SpeechEvent::agent_speaking_end());
                }
                return Ok(TtsDrainWrite::Stopped);
            }
        }
        Ok(TtsDrainWrite::Continued)
    }

    async fn stream_post_utterance_silence(
        writer: &PcmWriter,
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        drain_generation: u64,
        silence_ms: u32,
    ) -> SpeechResult<()> {
        let frame_count = silence_ms.div_ceil(20);
        let silent = Bytes::from(vec![0_u8; STEREO_FRAME_20MS_BYTES]);
        voice_debug(format!(
            "post-TTS outbound silence: {silence_ms} ms ({frame_count} frames)"
        ));
        for _ in 0..frame_count {
            if !inner.lock().await.running {
                voice_debug("post-TTS silence stopped (agent stop)");
                return Ok(());
            }
            if tts_buffer.current_generation().await != drain_generation {
                voice_debug("post-TTS silence stopped (barge-in flush)");
                return Ok(());
            }
            writer(silent.clone(), 20)?;
            if !Self::pace_tts_drain_frame_while_running(tts_buffer, inner, drain_generation, 20)
                .await
            {
                voice_debug("post-TTS silence stopped during pacing (barge-in flush / stop)");
                return Ok(());
            }
        }
        Ok(())
    }

    async fn end_agent_speaking_inner(
        inner: &Arc<Mutex<AgentInner>>,
        arm_stt_hold_after_playback: bool,
    ) {
        let mut guard = inner.lock().await;
        guard.agent_speaking = false;
        guard.agent_speaking_since = None;
        guard.barge_awaiting_stt_partial = false;
        guard.stt_barge_fired_this_agent_playback = false;
        if arm_stt_hold_after_playback {
            Self::arm_stt_hold_if_idle(&mut guard);
        }
    }

    /// Real-time pacing between PCM frames; returns false when flushed (barge-in) or stopped.
    async fn pace_tts_drain_frame_while_running(
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        drain_generation: u64,
        duration_ms: u32,
    ) -> bool {
        let mut remaining = duration_ms;
        while remaining > 0 {
            if !inner.lock().await.running {
                return false;
            }
            let slice = remaining.min(20);
            tokio::time::sleep(std::time::Duration::from_millis(slice as u64)).await;
            if tts_buffer.current_generation().await != drain_generation {
                return false;
            }
            remaining = remaining.saturating_sub(slice);
        }
        true
    }
}

const STEREO_FRAME_20MS_BYTES: usize = 3840;

enum TtsDrainWrite {
    Continued,
    Stopped,
}

/// Pad to a full 20 ms stereo frame so Opus always receives 960 samples/channel.
fn pad_stereo_frame_20ms(frame: &[u8]) -> (Bytes, u32) {
    if frame.len() == STEREO_FRAME_20MS_BYTES {
        return (Bytes::copy_from_slice(frame), 20);
    }
    let mut padded = vec![0_u8; STEREO_FRAME_20MS_BYTES];
    let copy_len = frame.len().min(STEREO_FRAME_20MS_BYTES);
    padded[..copy_len].copy_from_slice(&frame[..copy_len]);
    (Bytes::from(padded), 20)
}

/// Drain complete 20 ms frames from `carry`; leave a short remainder unpadded.
fn take_complete_stereo_frames(carry: &mut Vec<u8>) -> Vec<(Bytes, u32)> {
    let complete = (carry.len() / STEREO_FRAME_20MS_BYTES) * STEREO_FRAME_20MS_BYTES;
    if complete == 0 {
        return Vec::new();
    }
    let data: Vec<u8> = carry.drain(..complete).collect();
    data.chunks(STEREO_FRAME_20MS_BYTES)
        .map(|frame| (Bytes::copy_from_slice(frame), 20_u32))
        .collect()
}

/// Legacy helper: split and pad every partial chunk (unit tests only).
#[cfg(test)]
fn split_stereo_pcm_frames(pcm: &Bytes, _total_duration_ms: u32) -> Vec<(Bytes, u32)> {
    if pcm.is_empty() {
        return Vec::new();
    }
    let mut carry = pcm.to_vec();
    let mut frames = take_complete_stereo_frames(&mut carry);
    if !carry.is_empty() {
        frames.push(pad_stereo_frame_20ms(&carry));
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

    #[test]
    fn split_stereo_pcm_frames_pads_tail_to_opus_20ms() {
        let pcm = Bytes::from(vec![0_u8; STEREO_FRAME_20MS_BYTES + 2560]);
        let frames = split_stereo_pcm_frames(&pcm, 45);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].0.len(), STEREO_FRAME_20MS_BYTES);
        assert_eq!(frames[0].1, 20);
        assert_eq!(frames[1].0.len(), STEREO_FRAME_20MS_BYTES);
        assert_eq!(frames[1].1, 20);
    }

    #[test]
    fn take_complete_stereo_frames_does_not_pad_partial_midstream() {
        let mut carry = vec![1_u8; 1000];
        assert!(take_complete_stereo_frames(&mut carry).is_empty());
        assert_eq!(carry.len(), 1000);

        carry.extend(std::iter::repeat_n(2_u8, STEREO_FRAME_20MS_BYTES));
        let frames = take_complete_stereo_frames(&mut carry);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0.len(), STEREO_FRAME_20MS_BYTES);
        // Remainder kept for the next progressive chunk (no silence pad).
        assert_eq!(
            carry.len(),
            1000 + STEREO_FRAME_20MS_BYTES - STEREO_FRAME_20MS_BYTES
        );
        // 1000 bytes from the first partial remain after taking one full frame from the join.
        // carry was 1000+3840; drained 3840 → 1000 left.
        assert_eq!(carry.len(), 1000);
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
