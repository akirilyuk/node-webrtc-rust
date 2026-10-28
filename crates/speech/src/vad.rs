//! Voice activity detection with Silero (optional) and energy fallback.
//!
//! [`VadEngine`] wraps the configured provider and accumulates speech/silence duration to emit
//! [`VadTransition::SpeechStart`] and [`VadTransition::SpeechEnd`]. The voice agent uses those
//! transitions for `user_speaking_*` events, STT gating, and barge-in ([`handle_barge_in`]).

use crate::config::{AdaptiveVadConfig, BargeInConfig, VadConfig};
use crate::error::SpeechResult;
use crate::events::SpeechEvent;
use crate::pcm::{pcm_rms_i16, stereo_48k_to_mono_16k, WEBRTC_PCM_SAMPLE_RATE};
use crate::tts_buffer::TtsBuffer;

/// VAD transition emitted to the voice agent loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadTransition {
    SpeechStart,
    SpeechEnd,
}

/// Trait for frame-level voice activity detectors.
pub trait VoiceActivityDetector: Send {
    fn reset(&mut self);
    fn process_mono_frame(&mut self, mono_i16: &[i16], sample_rate: u32) -> SpeechResult<bool>;
}

/// Energy-threshold VAD used by default and in CI tests.
pub struct EnergyVad {
    threshold: f32,
}

impl EnergyVad {
    pub fn new(threshold: f32) -> Self {
        Self { threshold }
    }
}

impl VoiceActivityDetector for EnergyVad {
    fn reset(&mut self) {}

    fn process_mono_frame(&mut self, mono_i16: &[i16], _sample_rate: u32) -> SpeechResult<bool> {
        Ok(pcm_rms_i16(mono_i16) >= self.threshold)
    }
}

/// Read-only snapshot of the adaptive VAD's levels, in dBFS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdaptiveVadState {
    pub threshold_db: f32,
    pub noise_db: f32,
    pub speech_db: f32,
}

/// Energy VAD whose threshold follows the noise floor and the caller's level.
///
/// Runs per mono 16 kHz frame. See `VOICE-VAD-AND-BARGE-IN.md` ("Adaptive energy VAD").
pub struct AdaptiveEnergyVad {
    cfg: AdaptiveVadConfig,
    warmup_threshold: f32,
    /// Ring of noise-frame levels (dB).
    ring: Vec<f32>,
    ring_pos: usize,
    scratch: Vec<f32>,
    elapsed_ms: u32,
    active: bool,
    /// Time since the engine's SpeechStart; 0 while no utterance is open.
    utterance_ms: u32,
    utterance_open: bool,
    stuck_logged: bool,
    speech_db: Option<f32>,
    last_threshold_db: f32,
    last_noise_db: f32,
}

impl AdaptiveEnergyVad {
    pub fn new(warmup_threshold: f32, cfg: AdaptiveVadConfig) -> Self {
        Self {
            cfg,
            warmup_threshold,
            ring: Vec::new(),
            ring_pos: 0,
            scratch: Vec::new(),
            elapsed_ms: 0,
            active: false,
            utterance_ms: 0,
            utterance_open: false,
            stuck_logged: false,
            speech_db: None,
            last_threshold_db: f32::NAN,
            last_noise_db: f32::NAN,
        }
    }

    /// The engine declared SpeechStart: the stuck guard now times the open utterance.
    pub fn on_speech_start(&mut self) {
        self.utterance_open = true;
        self.utterance_ms = 0;
    }

    /// The engine declared SpeechEnd: the utterance is over, the guard is disarmed.
    pub fn on_speech_end(&mut self) {
        self.utterance_open = false;
        self.utterance_ms = 0;
        self.stuck_logged = false;
    }

    pub fn state(&self) -> AdaptiveVadState {
        AdaptiveVadState {
            threshold_db: self.last_threshold_db,
            noise_db: self.last_noise_db,
            speech_db: self.speech_db.unwrap_or(f32::NAN),
        }
    }

    fn push_noise(&mut self, db: f32, cap: usize) {
        if self.ring.len() < cap {
            self.ring.push(db);
        } else {
            self.ring[self.ring_pos] = db;
            self.ring_pos = (self.ring_pos + 1) % cap;
        }
    }

    /// Nearest-rank percentile of the ring; `None` while the ring is empty.
    fn noise_level(&mut self) -> Option<f32> {
        let n = self.ring.len();
        if n == 0 {
            return None;
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.ring);
        let rank = ((self.cfg.noise_percentile as usize * n).div_ceil(100)).clamp(1, n) - 1;
        let (_, v, _) = self
            .scratch
            .select_nth_unstable_by(rank, |a, b| a.total_cmp(b));
        Some(*v)
    }
}

impl VoiceActivityDetector for AdaptiveEnergyVad {
    fn reset(&mut self) {
        self.ring.clear();
        self.ring_pos = 0;
        self.elapsed_ms = 0;
        self.active = false;
        self.utterance_ms = 0;
        self.utterance_open = false;
        self.stuck_logged = false;
        self.speech_db = None;
        self.last_threshold_db = f32::NAN;
        self.last_noise_db = f32::NAN;
    }

    fn process_mono_frame(&mut self, mono_i16: &[i16], sample_rate: u32) -> SpeechResult<bool> {
        let frame_ms =
            ((mono_i16.len() as u64 * 1000) / u64::from(sample_rate.max(1))).max(1) as u32;
        let cap = (self.cfg.noise_window_ms / frame_ms).max(1) as usize;
        let rms = pcm_rms_i16(mono_i16);
        let db = 20.0 * rms.max(1e-5).log10();
        // Until the noise window has a frame (a caller talking through the whole warm-up) the
        // plain energy rule keeps deciding.
        let warming = self.elapsed_ms < self.cfg.warmup_ms || self.ring.is_empty();
        self.elapsed_ms = self.elapsed_ms.saturating_add(frame_ms);

        let active = if warming {
            let a = rms >= self.warmup_threshold;
            if !a {
                self.push_noise(db, cap);
            }
            a
        } else {
            let noise = self.noise_level().expect("ring non-empty");
            let mut t = noise + self.cfg.noise_margin_db;
            if self.cfg.use_speech_level {
                if let Some(s) = self.speech_db {
                    if s > noise + 2.0 * self.cfg.noise_margin_db {
                        t = t.max((noise + s) / 2.0);
                    }
                }
            }
            let t = t.clamp(self.cfg.min_threshold_dbfs, self.cfg.max_threshold_dbfs);
            self.last_threshold_db = t;
            self.last_noise_db = noise;
            let a = if self.active {
                db >= t - self.cfg.hysteresis_db
            } else {
                db >= t
            };
            if a {
                self.speech_db = Some(match self.speech_db {
                    Some(s) => s + 0.05 * (db - s),
                    None => db,
                });
            } else {
                self.push_noise(db, cap);
            }
            a
        };

        // Stuck guard: an utterance open longer than `stuckActiveMs` feeds every frame into the
        // noise window (active ones too; inactive ones are already in) until SpeechEnd.
        if self.utterance_open {
            self.utterance_ms = self.utterance_ms.saturating_add(frame_ms);
            if self.utterance_ms > self.cfg.stuck_active_ms {
                if !self.stuck_logged {
                    self.stuck_logged = true;
                    crate::agent::voice_debug(
                        "adaptive vad: utterance open too long, feeding noise window",
                    );
                }
                if active {
                    self.push_noise(db, cap);
                }
            }
        }
        self.active = active;
        Ok(active)
    }
}

#[cfg(feature = "silero-vad")]
pub struct SileroVad {
    model: silero_vad_rust::silero_vad::model::OnnxModel,
    threshold: f32,
    sample_rate: u32,
    pending: Vec<f32>,
    last_prob: f32,
}

#[cfg(feature = "silero-vad")]
impl SileroVad {
    pub fn new(threshold: f32, sample_rate: crate::config::VadSampleRate) -> SpeechResult<Self> {
        let sample_rate = sample_rate.as_u32();
        let model = silero_vad_rust::load_silero_vad()
            .map_err(|e| crate::error::SpeechError::Vad(e.to_string()))?;
        Ok(Self {
            model,
            threshold,
            sample_rate,
            pending: Vec::new(),
            last_prob: 0.0,
        })
    }

    fn chunk_size(&self) -> usize {
        if self.sample_rate == 8_000 {
            256
        } else {
            512
        }
    }
}

#[cfg(feature = "silero-vad")]
impl VoiceActivityDetector for SileroVad {
    fn reset(&mut self) {
        self.model.reset_states();
        self.pending.clear();
        self.last_prob = 0.0;
    }

    fn process_mono_frame(&mut self, mono_i16: &[i16], sample_rate: u32) -> SpeechResult<bool> {
        const I16_SCALE: f32 = 1.0 / i16::MAX as f32;
        self.pending
            .extend(mono_i16.iter().map(|&s| f32::from(s) * I16_SCALE));

        let chunk = self.chunk_size();
        while self.pending.len() >= chunk {
            let frame: Vec<f32> = self.pending.drain(..chunk).collect();
            let output = self
                .model
                .forward_chunk(&frame, sample_rate)
                .map_err(|e| crate::error::SpeechError::Vad(e.to_string()))?;
            self.last_prob = output.get((0, 0)).copied().unwrap_or(0.0);
        }

        Ok(self.last_prob >= self.threshold)
    }
}

enum VadBackend {
    Energy(EnergyVad),
    Adaptive(AdaptiveEnergyVad),
    #[cfg(feature = "silero-vad")]
    Silero(SileroVad),
}

/// Stateful VAD with min speech/silence timing and barge-in handling.
/// Frame-level VAD with min speech/silence durations and optional pending-speech detection.
pub struct VadEngine {
    config: VadConfig,
    backend: VadBackend,
    speaking: bool,
    speech_ms: u32,
    silence_ms: u32,
    frame_ms: u32,
}

impl VadEngine {
    pub fn new(config: VadConfig) -> SpeechResult<Self> {
        let backend = if config.provider == "silero" {
            #[cfg(feature = "silero-vad")]
            {
                VadBackend::Silero(SileroVad::new(config.threshold, config.sample_rate)?)
            }
            #[cfg(not(feature = "silero-vad"))]
            {
                return Err(crate::error::SpeechError::Config(
                    "vad.provider is \"silero\" but this native build was compiled without \
                     the silero-vad feature (only energy VAD is available). Use \
                     vad.provider \"energy\" or rebuild with \
                     node-webrtc-rust-speech features = [\"silero-vad\"] on the bindings crate."
                        .into(),
                ));
            }
        } else if config.provider == "adaptive" {
            config
                .adaptive
                .validate()
                .map_err(crate::error::SpeechError::Config)?;
            VadBackend::Adaptive(AdaptiveEnergyVad::new(
                config.threshold,
                config.adaptive.clone(),
            ))
        } else if config.provider == "energy" || config.provider.is_empty() {
            VadBackend::Energy(EnergyVad::new(config.threshold))
        } else {
            return Err(crate::error::SpeechError::Config(format!(
                "unsupported vad.provider \"{}\" (use \"energy\", \"adaptive\" or \"silero\")",
                config.provider
            )));
        };

        Ok(Self {
            config,
            backend,
            speaking: false,
            speech_ms: 0,
            silence_ms: 0,
            frame_ms: 20,
        })
    }

    pub fn config(&self) -> &VadConfig {
        &self.config
    }

    /// Levels of the adaptive provider (`None` for other providers). For tests and the bench.
    pub fn adaptive_state(&self) -> Option<AdaptiveVadState> {
        match &self.backend {
            VadBackend::Adaptive(v) => Some(v.state()),
            _ => None,
        }
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    /// Process one inbound WebRTC PCM frame (48 kHz stereo).
    /// Returns detected transitions and whether the current frame is voice-active.
    pub fn process_webrtc_pcm(
        &mut self,
        pcm: &[u8],
        duration_ms: u32,
    ) -> SpeechResult<(Vec<VadTransition>, bool)> {
        if !self.config.enabled {
            return Ok((Vec::new(), false));
        }

        self.frame_ms = duration_ms.max(1);
        let mono = stereo_48k_to_mono_16k(pcm);
        if mono.is_empty() {
            return Ok((Vec::new(), false));
        }

        let active = match &mut self.backend {
            VadBackend::Energy(v) => {
                v.process_mono_frame(&mono, self.config.sample_rate.as_u32())?
            }
            VadBackend::Adaptive(v) => v.process_mono_frame(&mono, WEBRTC_PCM_SAMPLE_RATE_16K)?,
            #[cfg(feature = "silero-vad")]
            VadBackend::Silero(v) => {
                v.process_mono_frame(&mono, self.config.sample_rate.as_u32())?
            }
        };

        let transitions = self.update_state(active);
        if let VadBackend::Adaptive(v) = &mut self.backend {
            for t in &transitions {
                match t {
                    VadTransition::SpeechStart => v.on_speech_start(),
                    VadTransition::SpeechEnd => v.on_speech_end(),
                }
            }
        }
        if transitions.contains(&VadTransition::SpeechStart) {
            if let VadBackend::Adaptive(v) = &self.backend {
                let st = v.state();
                crate::agent::voice_debug(format!(
                    "adaptive vad: speech start T={:.1} N={:.1} S={:.1}",
                    st.threshold_db, st.noise_db, st.speech_db
                ));
            }
        }
        Ok((transitions, active))
    }

    /// True while the VAD is accumulating speech time but has not yet declared `SpeechStart`.
    pub fn is_pending_speech(&self) -> bool {
        !self.speaking && self.speech_ms > 0
    }

    fn update_state(&mut self, active: bool) -> Vec<VadTransition> {
        let mut transitions = Vec::new();

        if active {
            self.speech_ms = self.speech_ms.saturating_add(self.frame_ms);
            self.silence_ms = 0;
            // `speech_pad_ms` sizes the STT pre-roll ring only — do not subtract it here or
            // a large pad forces immediate SpeechStart with an almost empty pre-roll flush.
            if !self.speaking && self.speech_ms >= self.config.min_speech_duration_ms {
                self.speaking = true;
                transitions.push(VadTransition::SpeechStart);
            }
        } else {
            self.silence_ms = self.silence_ms.saturating_add(self.frame_ms);
            if self.speaking && self.silence_ms >= self.config.min_silence_duration_ms {
                self.speaking = false;
                self.speech_ms = 0;
                transitions.push(VadTransition::SpeechEnd);
            }
        }

        transitions
    }

    pub fn reset(&mut self) {
        self.speaking = false;
        self.speech_ms = 0;
        self.silence_ms = 0;
        match &mut self.backend {
            VadBackend::Energy(v) => v.reset(),
            VadBackend::Adaptive(v) => v.reset(),
            #[cfg(feature = "silero-vad")]
            VadBackend::Silero(v) => v.reset(),
        }
    }
}

/// Apply barge-in policy (flush + `barge_in` event) when `enabled` is true.
pub async fn handle_barge_in(
    barge_in: &BargeInConfig,
    tts_buffer: &TtsBuffer,
    emit: impl Fn(SpeechEvent),
) {
    if !barge_in.enabled {
        return;
    }

    if barge_in.flush_tts {
        tts_buffer.flush_with_reply_events(&emit).await;
    }
    emit(SpeechEvent::barge_in());
}

/// `stereo_48k_to_mono_16k` always yields 16 kHz mono.
const WEBRTC_PCM_SAMPLE_RATE_16K: u32 = 16_000;

pub fn webrtc_frame_sample_rate() -> u32 {
    WEBRTC_PCM_SAMPLE_RATE
}
