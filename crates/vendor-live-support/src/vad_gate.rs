//! VAD gating for STT tests: only speech (plus the production pre-roll) is pushed to the vendor, never
//! the whole clip. Cloud vendors bill STT for the audio streamed, and the production
//! recommendation is `vad.gateStt: true` (`VOICE_AGENT_VAD_PRESET`), which this helper mirrors
//! with the same engine and default thresholds the voice agent uses.

use std::collections::VecDeque;

use bytes::Bytes;
use node_webrtc_rust_speech::config::VadConfig;
use node_webrtc_rust_speech::pcm::{i16_samples_to_bytes, stereo_48k_to_mono_16k};
use node_webrtc_rust_speech::stt_pre_roll::stt_pre_roll_capacity_ms;
use node_webrtc_rust_speech::vad::{VadEngine, VadTransition};

/// 20 ms of 48 kHz stereo s16le.
pub const FRAME_BYTES: usize = 3840;
/// 48 kHz stereo s16le: 4 bytes per frame, 48 frames per millisecond.
pub const BYTES_PER_MS: usize = 192;
/// `speechPadMs` of `VOICE_AGENT_VAD_PRESET` (`packages/sdk/src/voice/defaults.ts`), the
/// recommended production preset. The Rust default is 300.
const PRESET_SPEECH_PAD_MS: u32 = 500;

/// The VAD config the voice agent runs with `VOICE_AGENT_VAD_PRESET`: library defaults (energy
/// engine, threshold 0.15, 250 ms minimum speech, 1300 ms minimum silence) and a 500 ms pad.
pub fn preset_vad_config() -> VadConfig {
    VadConfig {
        enabled: true,
        gate_stt: true,
        speech_pad_ms: PRESET_SPEECH_PAD_MS,
        ..VadConfig::default()
    }
}

/// Past audio emitted with `SpeechStart`: the production STT pre-roll ring (`speech_pad_ms` +
/// `min_speech_duration_ms`, 750 ms with the preset). The speech accumulates its minimum duration
/// over frames that may be separated by quiet word gaps, so a shorter ring clips the first words.
fn pre_roll_frames() -> usize {
    (stt_pre_roll_capacity_ms(&preset_vad_config()) / 20) as usize
}

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
    let mut vad = VadEngine::new(preset_vad_config()).expect("energy VAD");
    let pre_roll = pre_roll_frames();
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
            if ring.len() > pre_roll {
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

/// Peak level the STT clip is brought to before gating: -1 dBFS, a loud but unclipped microphone.
pub const CLIP_PEAK_DBFS: f64 = -1.0;

/// Scales 48 kHz stereo s16le so its peak sits at `peak_dbfs`. Vendor TTS output is not level
/// normalised: Deepgram Aura speech has frame RMS ~0.09 of full scale, below the default energy
/// VAD threshold (0.15), so the production gate would never open on it. A real microphone chain is
/// gain-controlled; this stands in for it and leaves the production VAD threshold untouched.
/// Digital silence stays silence, and a clip already louder than the target is left as it is.
pub fn normalize_peak_stereo_48k(pcm: &[u8], peak_dbfs: f64) -> Vec<u8> {
    let peak = pcm
        .as_chunks::<2>()
        .0
        .iter()
        .map(|s| i32::from(i16::from_le_bytes(*s)).unsigned_abs())
        .max()
        .unwrap_or(0);
    if peak == 0 {
        return pcm.to_vec();
    }
    let target = 10_f64.powf(peak_dbfs / 20.0) * f64::from(i16::MAX);
    let gain = target / f64::from(peak);
    if gain <= 1.0 {
        return pcm.to_vec();
    }
    pcm.as_chunks::<2>()
        .0
        .iter()
        .flat_map(|s| {
            let v = (f64::from(i16::from_le_bytes(*s)) * gain)
                .round()
                .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
            v.to_le_bytes()
        })
        .collect()
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

    /// Frame-RMS shaped like Deepgram Aura speech (~0.1 of full scale): the default energy VAD
    /// (threshold 0.15) never opens on it as delivered.
    fn quiet_speech() -> Vec<u8> {
        [
            silence_stereo_48k(1000),
            tone_stereo_48k(1500, -14.0),
            silence_stereo_48k(2000),
        ]
        .concat()
    }

    #[test]
    fn quiet_vendor_speech_is_not_gated_in_as_delivered() {
        assert_eq!(vad_gated_frames(&quiet_speech()).starts, 0);
    }

    #[test]
    fn normalised_quiet_speech_opens_the_gate_and_ends_it() {
        let gated = vad_gated_frames(&normalize_peak_stereo_48k(&quiet_speech(), CLIP_PEAK_DBFS));
        assert_eq!((gated.starts, gated.ends), (1, 1));
        assert!(gated.pushed_ms < gated.total_ms - 1000);
    }

    #[test]
    fn normalise_sets_the_peak_keeps_silence_and_never_attenuates() {
        let out = normalize_peak_stereo_48k(&quiet_speech(), -1.0);
        assert_eq!(out.len(), quiet_speech().len());
        let peak = out
            .as_chunks::<2>()
            .0
            .iter()
            .map(|s| i32::from(i16::from_le_bytes(*s)).abs())
            .max()
            .unwrap();
        assert!((29_000..=29_300).contains(&peak), "{peak}");
        assert!(out[..1000 * BYTES_PER_MS].iter().all(|b| *b == 0));
        let loud = tone_stereo_48k(100, 0.0);
        assert_eq!(normalize_peak_stereo_48k(&loud, -1.0), loud);
        assert!(normalize_peak_stereo_48k(&silence_stereo_48k(100), -1.0)
            .iter()
            .all(|b| *b == 0));
    }

    /// A short first word, a 400 ms gap, then the rest of the phrase: the VAD reaches its 250 ms
    /// minimum speech only inside the second burst, so the first word is in the pre-roll ring
    /// only if the ring is as long as production's (750 ms), not 300 ms.
    #[test]
    fn pre_roll_reaches_back_over_a_word_gap_to_the_first_word() {
        let clip = [
            silence_stereo_48k(1000),
            tone_stereo_48k(100, -3.0),
            silence_stereo_48k(400),
            tone_stereo_48k(600, -3.0),
            silence_stereo_48k(2000),
        ]
        .concat();
        let gated = vad_gated_frames(&clip);
        assert_eq!((gated.starts, gated.ends), (1, 1));
        let voiced = gated
            .frames_16k
            .iter()
            .filter(|f| {
                f.as_chunks::<2>()
                    .0
                    .iter()
                    .any(|s| i16::from_le_bytes(*s) != 0)
            })
            .count();
        // 5 frames of the first word + 30 of the second.
        assert!(voiced >= 35, "first word clipped: {voiced} voiced frames");
        assert_eq!(pre_roll_frames(), 37);
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
