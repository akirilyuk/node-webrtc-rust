//! Voice activity detection with Silero (optional) and energy fallback.
//!
//! [`VadEngine`] wraps the configured provider and accumulates speech/silence duration to emit
//! [`VadTransition::SpeechStart`] and [`VadTransition::SpeechEnd`]. The voice agent uses those
//! transitions for `user_speaking_*` events, STT gating, and barge-in ([`handle_barge_in`]).

use crate::config::{BargeInConfig, VadConfig};
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
        self.pending.extend(
            mono_i16
                .iter()
                .map(|&s| f32::from(s) * I16_SCALE),
        );

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
        } else if config.provider == "energy" || config.provider.is_empty() {
            VadBackend::Energy(EnergyVad::new(config.threshold))
        } else {
            return Err(crate::error::SpeechError::Config(format!(
                "unsupported vad.provider \"{}\" (use \"energy\" or \"silero\")",
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
            VadBackend::Energy(v) => v.process_mono_frame(&mono, self.config.sample_rate.as_u32())?,
            #[cfg(feature = "silero-vad")]
            VadBackend::Silero(v) => v.process_mono_frame(&mono, self.config.sample_rate.as_u32())?,
        };

        Ok((self.update_state(active), active))
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
        tts_buffer.flush().await;
    }
    emit(SpeechEvent::barge_in());
}

pub fn webrtc_frame_sample_rate() -> u32 {
    WEBRTC_PCM_SAMPLE_RATE
}
