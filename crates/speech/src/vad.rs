//! Voice activity detection with Silero (optional) and energy fallback.

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
    inner: silero_vad_rust::SileroVad,
    threshold: f32,
}

#[cfg(feature = "silero-vad")]
impl SileroVad {
    pub fn new(threshold: f32, sample_rate: crate::config::VadSampleRate) -> SpeechResult<Self> {
        let inner = silero_vad_rust::SileroVad::new(sample_rate.as_u32())
            .map_err(|e| crate::error::SpeechError::Vad(e.to_string()))?;
        Ok(Self { inner, threshold })
    }
}

#[cfg(feature = "silero-vad")]
impl VoiceActivityDetector for SileroVad {
    fn reset(&mut self) {
        self.inner.reset();
    }

    fn process_mono_frame(&mut self, mono_i16: &[i16], sample_rate: u32) -> SpeechResult<bool> {
        let prob = self
            .inner
            .predict(mono_i16, sample_rate)
            .map_err(|e| crate::error::SpeechError::Vad(e.to_string()))?;
        Ok(prob >= self.threshold)
    }
}

enum VadBackend {
    Energy(EnergyVad),
    #[cfg(feature = "silero-vad")]
    Silero(SileroVad),
}

/// Stateful VAD with min speech/silence timing and barge-in handling.
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
                VadBackend::Energy(EnergyVad::new(config.threshold))
            }
        } else {
            VadBackend::Energy(EnergyVad::new(config.threshold))
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
            if !self.speaking
                && self.speech_ms >= self.config.min_speech_duration_ms.saturating_sub(self.config.speech_pad_ms)
            {
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

/// Apply barge-in policy when user speech starts.
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
