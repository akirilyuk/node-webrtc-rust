//! VAD gating for STT tests: only speech (plus a short pre-roll) is pushed to the vendor, never
//! the whole clip. Cloud vendors bill STT for the audio streamed, and the production
//! recommendation is `vad.gateStt: true` (`VOICE_AGENT_VAD_PRESET`), which this helper mirrors
//! with the same engine and default thresholds the voice agent uses.

use std::collections::VecDeque;

use bytes::Bytes;
use node_webrtc_rust_speech::config::VadConfig;
use node_webrtc_rust_speech::pcm::{i16_samples_to_bytes, stereo_48k_to_mono_16k};
use node_webrtc_rust_speech::vad::{VadEngine, VadTransition};

/// 20 ms of 48 kHz stereo s16le.
pub const FRAME_BYTES: usize = 3840;
/// 48 kHz stereo s16le: 4 bytes per frame, 48 frames per millisecond.
pub const BYTES_PER_MS: usize = 192;
/// Past audio emitted with `SpeechStart` so the first syllable is not clipped.
const PRE_ROLL_FRAMES: usize = 15; // 300 ms

/// Result of gating one clip.
pub struct GatedAudio {
    /// 16 kHz mono s16le, 20 ms per frame.
    pub frames_16k: Vec<Bytes>,
    pub pushed_ms: u32,
    pub total_ms: u32,
    pub starts: u32,
    pub ends: u32,
}

fn frame_ms(frame: &[u8]) -> u32 {
    (frame.len() / BYTES_PER_MS) as u32
}

fn to_16k(frame: &[u8]) -> Bytes {
    i16_samples_to_bytes(&stereo_48k_to_mono_16k(frame))
}

/// Runs the default energy VAD over 20 ms frames. On `SpeechStart` the 300 ms ring of past
/// frames is emitted, then every frame until `SpeechEnd` (inclusive). Frames outside a speech
/// segment are dropped.
pub fn vad_gated_frames(stereo_48k: &[u8]) -> GatedAudio {
    let mut vad = VadEngine::new(VadConfig {
        enabled: true,
        ..VadConfig::default()
    })
    .expect("energy VAD");
    let mut gated = GatedAudio {
        frames_16k: Vec::new(),
        pushed_ms: 0,
        total_ms: frame_ms(stereo_48k),
        starts: 0,
        ends: 0,
    };
    let mut ring: VecDeque<&[u8]> = VecDeque::new();
    let mut emitting = false;

    for frame in stereo_48k.chunks(FRAME_BYTES) {
        let (transitions, _) = vad
            .process_webrtc_pcm(frame, frame_ms(frame).max(1))
            .expect("vad frame");
        let mut emit_current = emitting;
        let mut end_after = false;
        for transition in transitions {
            match transition {
                VadTransition::SpeechStart => {
                    gated.starts += 1;
                    for past in ring.drain(..) {
                        gated.pushed_ms += frame_ms(past);
                        gated.frames_16k.push(to_16k(past));
                    }
                    emitting = true;
                    emit_current = true;
                }
                VadTransition::SpeechEnd => {
                    gated.ends += 1;
                    end_after = true;
                }
            }
        }
        if emit_current {
            gated.pushed_ms += frame_ms(frame);
            gated.frames_16k.push(to_16k(frame));
        } else {
            ring.push_back(frame);
            if ring.len() > PRE_ROLL_FRAMES {
                ring.pop_front();
            }
        }
        if end_after {
            emitting = false;
            ring.clear();
        }
    }
    gated
}

/// Digital silence, 48 kHz stereo s16le.
pub fn silence_stereo_48k(ms: usize) -> Vec<u8> {
    vec![0; ms * BYTES_PER_MS]
}

/// 440 Hz sine at the given peak level in dBFS, identical on both channels.
pub fn tone_stereo_48k(ms: usize, dbfs: f64) -> Vec<u8> {
    let amplitude = 10_f64.powf(dbfs / 20.0) * f64::from(i16::MAX);
    let frames = ms * 48;
    let mut out = Vec::with_capacity(frames * 4);
    for n in 0..frames {
        let phase = 2.0 * std::f64::consts::PI * 440.0 * (n as f64) / 48_000.0;
        let sample = (amplitude * phase.sin()) as i16;
        out.extend_from_slice(&sample.to_le_bytes());
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// 16 kHz mono s16le to 48 kHz stereo s16le: linear interpolation (x3), channel duplicated.
/// The last sample is held for its two interpolated slots.
pub fn mono_16k_to_stereo_48k(mono: &[u8]) -> Vec<u8> {
    let samples: Vec<i32> = mono
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i32::from(i16::from_le_bytes(*pair)))
        .collect();
    let mut out = Vec::with_capacity(samples.len() * 3 * 4);
    for (i, &a) in samples.iter().enumerate() {
        let b = samples.get(i + 1).copied().unwrap_or(a);
        for step in 0..3 {
            let value = (a + (b - a) * step / 3) as i16;
            let bytes = value.to_le_bytes();
            out.extend_from_slice(&bytes);
            out.extend_from_slice(&bytes);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    fn stereo_samples(bytes: &[u8]) -> Vec<(i16, i16)> {
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                (
                    i16::from_le_bytes([c[0], c[1]]),
                    i16::from_le_bytes([c[2], c[3]]),
                )
            })
            .collect()
    }

    #[test]
    fn upsample_triples_the_samples_and_duplicates_the_channel() {
        let out = mono_16k_to_stereo_48k(&mono(&[0, 300, -300]));
        assert_eq!(out.len(), 3 * 3 * 4);
        let pairs = stereo_samples(&out);
        assert!(pairs.iter().all(|(l, r)| l == r));
        let left: Vec<i16> = pairs.iter().map(|p| p.0).collect();
        // 0 -> 300 in thirds, 300 -> -300 in thirds, then the last sample held.
        assert_eq!(left, vec![0, 100, 200, 300, 100, -100, -300, -300, -300]);
    }

    #[test]
    fn upsample_then_downsample_keeps_a_dc_level() {
        let out = mono_16k_to_stereo_48k(&mono(&[1000; 160]));
        let back = stereo_48k_to_mono_16k(&out);
        assert_eq!(back.len(), 160);
        assert!(back.iter().all(|s| *s == 1000));
    }

    #[test]
    fn upsample_of_empty_is_empty() {
        assert!(mono_16k_to_stereo_48k(&[]).is_empty());
    }
}
