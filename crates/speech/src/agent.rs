//! Voice agent orchestration (attach, start/stop, TTS injection).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use tokio::sync::{broadcast, Mutex, Notify};

use crate::config::{EventDeliveryMode, VoiceAgentConfig};
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
    /// True while agent TTS is synthesizing or playing outbound audio.
    agent_speaking: bool,
    agent_speaking_since: Option<Instant>,
    /// VAD saw speech during agent TTS; defer immediate flush until STT partial (if required).
    barge_awaiting_stt_partial: bool,
    /// Semantic barge already fired for the current agent playback generation.
    stt_barge_fired_this_agent_playback: bool,
    pcm_writer: Option<PcmWriter>,
    pcm_reader: Option<PcmReader>,
}

/// One voice agent session bound to a single peer connection.
pub struct VoiceAgent {
    event_bus: SpeechEventBus,
    tts_buffer: TtsBuffer,
    #[allow(dead_code)]
    registry: Arc<VendorRegistry>,
    inner: Arc<Mutex<AgentInner>>,
    stt: Mutex<Option<Box<dyn SttProvider>>>,
    tts: Mutex<Option<Box<dyn TtsProvider>>>,
    tts_drain_wake: Arc<Notify>,
    tts_drain_worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl VoiceAgent {
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
                agent_speaking: false,
                agent_speaking_since: None,
                barge_awaiting_stt_partial: false,
                stt_barge_fired_this_agent_playback: false,
                pcm_writer: None,
                pcm_reader: None,
            })),
            stt: Mutex::new(stt),
            tts: Mutex::new(tts),
            tts_drain_wake: Arc::new(Notify::new()),
            tts_drain_worker: Mutex::new(None),
        })
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
        let mut slot = self.tts_drain_worker.lock().await;
        if slot.is_some() {
            return;
        }
        let wake = Arc::clone(&self.tts_drain_wake);
        let tts_buffer = self.tts_buffer.clone();
        let inner = Arc::clone(&self.inner);
        let event_bus = self.event_bus.clone();
        *slot = Some(tokio::spawn(async move {
            loop {
                wake.notified().await;
                if let Err(error) = Self::run_tts_drain(&tts_buffer, &inner, &event_bus).await {
                    voice_debug(format!("TTS drain error: {error}"));
                }
            }
        }));
    }

    /// Wait until outbound TTS playback finishes (for tests and explicit synchronization).
    pub async fn wait_tts_playback_idle(&self) -> SpeechResult<()> {
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let agent_speaking = self.inner.lock().await.agent_speaking;
            let queued = self.tts_buffer.is_speaking().await;
            if !agent_speaking && !queued {
                return Ok(());
            }
            if Instant::now() >= deadline {
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

    pub async fn send_text_to_tts(&self, text: &str) -> SpeechResult<()> {
        let chunks = {
            let tts = self.tts.lock().await;
            let tts = tts
                .as_ref()
                .ok_or_else(|| SpeechError::Config("TTS not configured".into()))?;
            tts.synthesize(text).await?
        };

        if chunks.is_empty() {
            return Ok(());
        }

        self.tts_buffer.enqueue(chunks).await;
        self.ensure_tts_drain_worker().await;
        self.tts_drain_wake.notify_one();
        Ok(())
    }

    pub async fn flush_tts(&self) -> SpeechResult<()> {
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
            if !barge_in_cfg.require_stt_partial || inner.config.stt.is_none() {
                return Ok(());
            }
            // Any qualifying partial while agent TTS is playing — STT gate already limits
            // inbound audio to user voice (not pending-only echo during semantic barge).
            if !inner.agent_speaking {
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
        Ok(())
    }

    async fn apply_vad_barge_in_on_speech_start(&self) -> SpeechResult<()> {
        let (barge_in, guard_active, agent_speaking, has_stt) = {
            let inner = self.inner.lock().await;
            (
                inner.config.vad.barge_in.clone(),
                Self::agent_playback_guard_active(&inner),
                inner.agent_speaking,
                inner.config.stt.is_some(),
            )
        };
        if !barge_in.enabled || !barge_in.use_vad {
            return Ok(());
        }
        if guard_active {
            voice_debug(format!(
                "barge-in suppressed: agent playback guard {} ms",
                barge_in.agent_playback_guard_ms
            ));
            return Ok(());
        }
        if barge_in.require_stt_partial && agent_speaking && has_stt {
            let mut inner = self.inner.lock().await;
            inner.barge_awaiting_stt_partial = true;
            voice_debug(
                "barge-in deferred until STT partial (require_stt_partial while agent TTS playing)",
            );
            return Ok(());
        }
        handle_barge_in(&barge_in, &self.tts_buffer, |event| self.emit(event)).await;
        if barge_in.flush_tts {
            let was_agent_speaking = {
                let inner = self.inner.lock().await;
                inner.agent_speaking
            };
            self.end_agent_speaking(false).await;
            voice_debug("agent_speaking=false (barge-in flush)");
            if was_agent_speaking {
                self.emit(SpeechEvent::agent_speaking_end());
            }
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
                inner.config.vad.min_silence_duration_ms.max(800),
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
    }

    /// Process one inbound PCM frame through VAD, barge-in, and optional STT gating.
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
            frame_active,
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
                if let Some(pre_roll) = inner.stt_pre_roll.as_mut() {
                    // Voice-only — silence must not fill the ring (see stt_pre_roll tests).
                    if !was_speaking && (frame_active || vad_pending) {
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
                    voice_debug(format!(
                        "STT gate hold: {} ms after speech end{}",
                        inner.stt_gate_hold_ms,
                        if inner.agent_speaking {
                            " (agent TTS playing — finalize after playback)"
                        } else {
                            ""
                        }
                    ));
                }
            } else if inner.stt_gate_hold_ms > 0 && !speech_start {
                // Brief gap (counting): cancel hold. Long pause then new speech: finish prior phrase.
                let user_voice_active = frame_active || vad_speaking;
                if user_voice_active && !inner.stt_endpoint_closing_started {
                    let hold_total = inner.config.vad.stt_gate_hold_ms;
                    let hold_elapsed = hold_total.saturating_sub(inner.stt_gate_hold_ms);
                    let long_pause_before_resume =
                        hold_total > 0 && hold_elapsed > hold_total / 2;
                    if inner.stt_finalize_pending
                        && !inner.stt_final_emitted_this_utterance
                        && long_pause_before_resume
                    {
                        complete_previous_utterance = true;
                        voice_debug(
                            "STT: voice resumed after long gate hold — completing previous utterance",
                        );
                    } else {
                        inner.stt_gate_hold_ms = 0;
                        inner.stt_finalize_pending = false;
                        voice_debug(
                            "STT gate hold cancelled: voice active again before hold expired",
                        );
                    }
                } else {
                    inner.stt_gate_hold_ms = inner
                        .stt_gate_hold_ms
                        .saturating_sub(duration_ms);
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

        let (stt_audio_open, stt_poll_open, should_finalize_utterance) = {
            let inner = self.inner.lock().await;
            let pending_gate = inner.config.vad.gate_stt_open_on_pending && vad_pending;
            let utterance_closing =
                inner.stt_finalize_pending && !inner.stt_final_emitted_this_utterance;
            let semantic_barge = inner.config.vad.barge_in.require_stt_partial
                && inner.config.stt.is_some();
            let stt_audio_open = if gate_stt {
                if inner.agent_speaking {
                    // While agent TTS plays, do not feed STT on VAD "pending" only — avoids
                    // burning a partial before `barge_awaiting_stt_partial` is armed on SpeechStart.
                    if semantic_barge {
                        vad_speaking || inner.stt_gate_hold_ms > 0
                    } else {
                        vad_speaking || inner.stt_gate_hold_ms > 0 || pending_gate
                    }
                } else {
                    vad_speaking
                        || inner.stt_gate_hold_ms > 0
                        || pending_gate
                        || utterance_closing
                }
            } else {
                true
            };
            let stt_poll_open = !gate_stt || stt_audio_open || utterance_closing;
            let should_finalize_utterance = gate_stt
                && utterance_closing
                && !inner.agent_speaking
                && !inner.stt_endpoint_closing_started
                && inner.stt_gate_hold_ms == 0
                && !vad_speaking
                && !frame_active;
            (stt_audio_open, stt_poll_open, should_finalize_utterance)
        };

        if complete_previous_utterance {
            self.complete_pending_utterance_if_any().await?;
        }

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
                self.push_stt_audio_bytes(buffered).await?;
            }
        }

        let mut speech_end_transition = false;
        for transition in &transitions {
            voice_debug(format!("VAD {transition:?}"));
            match transition {
                VadTransition::SpeechStart => {
                    self.apply_vad_barge_in_on_speech_start().await?;
                    self.emit(SpeechEvent::user_speaking_start());
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

        // Skip frame only when not streaming audio and not waiting on a pending STT final.
        if gate_stt && !stt_poll_open && !should_finalize_utterance {
            voice_debug(format!(
                "process_inbound_pcm call={call} skipped: gate_stt closed (not speaking, hold expired)"
            ));
            return Ok(());
        }

        if call == 1 || call % 50 == 0 {
            voice_debug(format!(
                "inbound PCM frame={call} bytes={} duration_ms={duration_ms}",
                pcm.len()
            ));
        }

        if !gate_stt || stt_audio_open {
            self.push_stt_audio_bytes(mono_bytes).await?;
        }
        if !gate_stt || stt_poll_open {
            self.poll_stt_transcripts().await?;
        }

        if speech_end_transition {
            let mut inner = self.inner.lock().await;
            // Defer clearing until after STT poll — same-frame partials must still trigger barge.
            // While agent TTS plays, brief VAD gaps must not cancel a pending semantic barge.
            if !inner.agent_speaking {
                inner.barge_awaiting_stt_partial = false;
            }
        }

        if should_finalize_utterance {
            {
                let mut inner = self.inner.lock().await;
                inner.stt_endpoint_closing_started = true;
            }
            let tail_ms = {
                let inner = self.inner.lock().await;
                inner.config.vad.min_silence_duration_ms.max(800)
            };
            voice_debug(format!(
                "STT utterance close: endpoint tail {tail_ms} ms then finalize (speaking_end with final)"
            ));
            self.push_stt_endpoint_tail(tail_ms).await?;
            self.finalize_stt_utterance().await?;
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
                    self.try_stt_gated_barge_in(&text).await?;
                    self.emit(SpeechEvent::user_speech_partial(text));
                }
                SttTranscript::Final(text) => {
                    voice_debug(format!("STT final: {text}"));
                    let emit_speaking_end = {
                        let mut inner = self.inner.lock().await;
                        inner.stt_final_emitted_this_utterance = true;
                        inner.stt_finalize_pending = false;
                        inner.stt_endpoint_closing_started = false;
                        let emit_end = !inner.stt_speaking_end_emitted_this_utterance;
                        if emit_end {
                            inner.stt_speaking_end_emitted_this_utterance = true;
                        }
                        emit_end
                    };
                    if emit_speaking_end {
                        self.emit(SpeechEvent::user_speaking_end());
                    }
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
