//! Voice agent orchestration (attach, start/stop, TTS injection).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use tokio::sync::{broadcast, Mutex};

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
    /// Set when poll_transcript already emitted user_speech_final for this utterance.
    stt_final_emitted_this_utterance: bool,
    /// True while agent TTS is synthesizing or playing outbound audio.
    agent_speaking: bool,
    agent_speaking_since: Option<Instant>,
    pcm_writer: Option<PcmWriter>,
    pcm_reader: Option<PcmReader>,
}

/// One voice agent session bound to a single peer connection.
pub struct VoiceAgent {
    event_bus: SpeechEventBus,
    tts_buffer: TtsBuffer,
    #[allow(dead_code)]
    registry: Arc<VendorRegistry>,
    inner: Mutex<AgentInner>,
    stt: Mutex<Option<Box<dyn SttProvider>>>,
    tts: Mutex<Option<Box<dyn TtsProvider>>>,
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
            inner: Mutex::new(AgentInner {
                config,
                attached: false,
                running: false,
                vad,
                stt_pre_roll,
                stt_gate_hold_ms: 0,
                stt_finalize_pending: false,
                stt_final_emitted_this_utterance: false,
                agent_speaking: false,
                agent_speaking_since: None,
                pcm_writer: None,
                pcm_reader: None,
            }),
            stt: Mutex::new(stt),
            tts: Mutex::new(tts),
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
        Ok(())
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
        {
            let mut inner = self.inner.lock().await;
            inner.agent_speaking = true;
            inner.agent_speaking_since = Some(Instant::now());
        }
        voice_debug("agent_speaking=true (TTS starting)");

        let chunks = {
            let tts = self.tts.lock().await;
            let tts = tts
                .as_ref()
                .ok_or_else(|| SpeechError::Config("TTS not configured".into()))?;
            tts.synthesize(text).await?
        };

        self.emit(SpeechEvent::agent_speaking_start());
        self.tts_buffer.enqueue(chunks).await;
        self.drain_tts_buffer().await?;
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
        let mut inner = self.inner.lock().await;
        inner.agent_speaking = false;
        inner.agent_speaking_since = None;
        if arm_stt_hold_after_playback {
            Self::arm_stt_hold_if_idle(&mut inner);
        }
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
            pre_roll_flush,
            stt_gate_open,
            should_finalize_utterance,
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

            let mut pre_roll_flush = None;

            if transitions.contains(&VadTransition::SpeechStart) {
                inner.stt_gate_hold_ms = 0;
                inner.stt_finalize_pending = false;
                inner.stt_final_emitted_this_utterance = false;
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
                            pre_roll_flush = Some(buffered);
                        }
                    }
                }
            } else if transitions.contains(&VadTransition::SpeechEnd) {
                if gate_stt {
                    inner.stt_pre_roll.as_mut().map(SttPreRollBuffer::clear);
                }
                // TTS word gaps can trigger SpeechEnd; defer finalize until agent finishes speaking.
                if !inner.agent_speaking {
                    inner.stt_gate_hold_ms = inner.config.vad.stt_gate_hold_ms;
                    inner.stt_finalize_pending = true;
                    voice_debug(format!(
                        "STT gate hold: {} ms after speech end",
                        inner.stt_gate_hold_ms
                    ));
                }
            } else if inner.stt_gate_hold_ms > 0 {
                // Only confirmed speech cancels hold — not VAD "pending" (pre-start) blips.
                let user_voice_active = frame_active || vad_speaking;
                if user_voice_active {
                    inner.stt_gate_hold_ms = 0;
                    inner.stt_finalize_pending = false;
                    voice_debug(
                        "STT gate hold cancelled: voice active again before hold expired",
                    );
                } else {
                    inner.stt_gate_hold_ms = inner
                        .stt_gate_hold_ms
                        .saturating_sub(duration_ms);
                }
            }

            let pending_gate = inner.config.vad.gate_stt_open_on_pending && vad_pending;
            let finalize_armed =
                inner.stt_finalize_pending && !inner.stt_final_emitted_this_utterance;
            let stt_gate_open = if gate_stt {
                // Do not feed agent TTS echo from speakers into STT while the agent is playing.
                if inner.agent_speaking {
                    false
                } else {
                    vad_speaking
                        || inner.stt_gate_hold_ms > 0
                        || pending_gate
                        || finalize_armed
                }
            } else {
                true
            };

            // Hold drained (or retry on a later silent frame after a skipped expiry).
            let should_finalize_utterance = gate_stt
                && finalize_armed
                && inner.stt_gate_hold_ms == 0
                && !vad_speaking
                && !frame_active;

            (
                transitions,
                gate_stt,
                pre_roll_flush,
                stt_gate_open,
                should_finalize_utterance,
            )
        };

        let pre_roll_was_empty = pre_roll_flush.as_ref().is_none_or(|b| b.is_empty());
        if let Some(buffered) = pre_roll_flush {
            self.push_stt_audio_bytes(buffered).await?;
        }

        for transition in &transitions {
            voice_debug(format!("VAD {transition:?}"));
            match transition {
                VadTransition::SpeechStart => {
                    let barge_in = {
                        let inner = self.inner.lock().await;
                        inner.config.vad.barge_in.clone()
                    };
                    let guard_active = {
                        let inner = self.inner.lock().await;
                        Self::agent_playback_guard_active(&inner)
                    };
                    if barge_in.enabled && barge_in.use_vad {
                        if guard_active {
                            voice_debug(format!(
                                "barge-in suppressed: agent playback guard {} ms",
                                barge_in.agent_playback_guard_ms
                            ));
                        } else {
                            handle_barge_in(&barge_in, &self.tts_buffer, |event| self.emit(event))
                                .await;
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
                        }
                    }
                    self.emit(SpeechEvent::user_speaking_start());
                }
                VadTransition::SpeechEnd => {
                    let (defer_speaking_end, agent_speaking) = {
                        let inner = self.inner.lock().await;
                        (
                            inner.config.vad.gate_stt && !inner.agent_speaking,
                            inner.agent_speaking,
                        )
                    };
                    if agent_speaking {
                        voice_debug(
                            "user_speaking_end suppressed (VAD SpeechEnd during agent TTS)",
                        );
                    } else if defer_speaking_end {
                        voice_debug(
                            "user_speaking_end deferred until STT gate hold expires (gate_stt)",
                        );
                    } else {
                        self.emit(SpeechEvent::user_speaking_end());
                    }
                }
            }
        }

        // Hold expiry must still run endpoint tail + finalize even though the gate closes on
        // the same frame (hold_ms reaches 0). Skipping here caused user_speech_final to fire
        // only after the next SpeechStart opened the gate again.
        if gate_stt && !stt_gate_open && !should_finalize_utterance {
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

        if gate_stt {
            let speech_start = transitions.contains(&VadTransition::SpeechStart);
            // Current frame is usually in the pre-roll flush on SpeechStart; push it when flush was empty.
            if !speech_start || pre_roll_was_empty {
                self.push_stt_audio_bytes(mono_bytes).await?;
            }
        } else {
            self.push_stt_audio_bytes(mono_bytes).await?;
        }
        self.poll_stt_transcripts().await?;

        if should_finalize_utterance {
            self.emit(SpeechEvent::user_speaking_end());
            let tail_ms = {
                let inner = self.inner.lock().await;
                inner.config.vad.min_silence_duration_ms.max(800)
            };
            {
                let mut inner = self.inner.lock().await;
                inner.stt_finalize_pending = false;
            }
            self.push_stt_endpoint_tail(tail_ms).await?;
            self.finalize_stt_utterance().await?;
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
                    self.emit(SpeechEvent::user_speech_partial(text));
                }
                SttTranscript::Final(text) => {
                    voice_debug(format!("STT final: {text}"));
                    {
                        let mut inner = self.inner.lock().await;
                        inner.stt_final_emitted_this_utterance = true;
                        inner.stt_finalize_pending = false;
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

    async fn drain_tts_buffer(&self) -> SpeechResult<()> {
        let writer = {
            let inner = self.inner.lock().await;
            inner
                .pcm_writer
                .clone()
                .ok_or(SpeechError::NotAttached)?
        };

        let drain_generation = self.tts_buffer.current_generation().await;

        while let Some(chunk) = self.tts_buffer.pop_chunk().await {
            for (frame, duration_ms) in split_stereo_pcm_frames(&chunk.pcm, chunk.duration_ms) {
                if self.tts_buffer.current_generation().await != drain_generation {
                    voice_debug("TTS drain stopped (barge-in flush)");
                    let still_speaking = {
                        let inner = self.inner.lock().await;
                        inner.agent_speaking
                    };
                    if still_speaking {
                        self.end_agent_speaking(false).await;
                        self.emit(SpeechEvent::agent_speaking_end());
                    }
                    return Ok(());
                }
                writer(frame, duration_ms)?;
                // Pace outbound frames so playback aligns with wall time and barge-in guard is meaningful.
                tokio::time::sleep(std::time::Duration::from_millis(duration_ms as u64)).await;
            }
        }
        self.end_agent_speaking(true).await;
        voice_debug("agent_speaking=false (TTS drained)");
        self.emit(SpeechEvent::agent_speaking_end());
        Ok(())
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
