use bytes::Bytes;
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
};

/// Stereo 48 kHz s16le frame size for 20 ms (Opus-compatible).
pub const STEREO_FRAME_20MS_BYTES: usize = 3840;

const WEBRTC_PCM_CHANNELS: usize = 2;

/// Convert mono f32 PCM at `src_rate` Hz to stereo 48 kHz s16le for WebRTC outbound tracks.
/// Pads the result to a 20 ms frame boundary (legacy full-utterance path).
pub fn f32_mono_to_stereo_48k_s16le(samples: &[f32], src_rate: u32) -> (Bytes, u32) {
    let stereo = f32_mono_to_stereo_48k_s16le_raw(samples, src_rate);
    if stereo.is_empty() {
        return (Bytes::new(), 1);
    }
    align_stereo_pcm_to_20ms(stereo)
}

/// Convert without 20 ms padding — used for progressive TTS deltas (drain pads frames).
pub fn f32_mono_to_stereo_48k_s16le_raw(samples: &[f32], src_rate: u32) -> Bytes {
    let mut stream = StreamingStereo48kResampler::new(src_rate);
    let mut out = stream.push_f32(samples).to_vec();
    out.extend_from_slice(&stream.finish());
    Bytes::from(out)
}

/// Pad trailing silence so stereo PCM length is a multiple of 20 ms @ 48 kHz.
pub fn align_stereo_pcm_to_20ms(stereo: Bytes) -> (Bytes, u32) {
    if stereo.is_empty() {
        return (stereo, 1);
    }

    let remainder = stereo.len() % STEREO_FRAME_20MS_BYTES;
    let aligned = if remainder == 0 {
        stereo
    } else {
        let mut padded = stereo.to_vec();
        padded.resize(stereo.len() + (STEREO_FRAME_20MS_BYTES - remainder), 0);
        Bytes::from(padded)
    };

    let duration_ms = stereo_duration_ms(&aligned);
    (aligned, duration_ms)
}

fn stereo_duration_ms(stereo: &Bytes) -> u32 {
    duration_ms_from_mono_s16le(stereo.len() / WEBRTC_PCM_CHANNELS, WEBRTC_PCM_SAMPLE_RATE)
}

/// Continuous mono→stereo 48 kHz converter for progressive TTS deltas.
///
/// Chunked [`Self::push_f32`] + [`Self::finish`] matches one-shot
/// [`f32_mono_to_stereo_48k_s16le_raw`] on the concatenated source (same linear
/// resample phase across chunk boundaries — avoids STT dropouts from per-chunk resample).
pub struct StreamingStereo48kResampler {
    src_rate: u32,
    /// Source samples as i16 (same quantization as the one-shot path).
    src: Vec<i16>,
    /// Next 48 kHz mono output index in the continuous stream.
    next_out: u64,
}

impl StreamingStereo48kResampler {
    pub fn new(src_rate: u32) -> Self {
        Self {
            src_rate: src_rate.max(1),
            src: Vec::new(),
            next_out: 0,
        }
    }

    /// Append source-rate mono f32 and emit any newly available stereo 48 kHz PCM.
    pub fn push_f32(&mut self, samples: &[f32]) -> Bytes {
        if samples.is_empty() {
            return Bytes::new();
        }
        self.src.reserve(samples.len());
        for &sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            self.src.push((clamped * i16::MAX as f32) as i16);
        }
        self.emit_available(false)
    }

    /// Emit remaining samples so the stream matches a one-shot convert of all pushed audio.
    pub fn finish(&mut self) -> Bytes {
        self.emit_available(true)
    }

    fn emit_available(&mut self, finished: bool) -> Bytes {
        if self.src.is_empty() {
            return Bytes::new();
        }

        let dst_rate = WEBRTC_PCM_SAMPLE_RATE;
        let mono = if self.src_rate == dst_rate {
            self.emit_passthrough(finished)
        } else {
            self.emit_linear(dst_rate, finished)
        };

        if mono.is_empty() {
            return Bytes::new();
        }

        let mono_bytes: Vec<u8> = mono
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        mono_s16le_to_stereo(&mono_bytes)
    }

    fn emit_passthrough(&mut self, finished: bool) -> Vec<i16> {
        let _ = finished;
        // Every source sample maps 1:1; emit everything not yet emitted.
        let already = self.next_out as usize;
        if already >= self.src.len() {
            return Vec::new();
        }
        let out = self.src[already..].to_vec();
        self.next_out = self.src.len() as u64;
        out
    }

    fn emit_linear(&mut self, dst_rate: u32, finished: bool) -> Vec<i16> {
        let total_src = self.src.len() as u64;
        let max_out = if finished {
            // Match `resample_linear_i16` batch length.
            ((total_src * u64::from(dst_rate)) / u64::from(self.src_rate)).max(1)
        } else {
            // Need `left + 1` available without clamping to the final sample.
            if total_src < 2 {
                return Vec::new();
            }
            ((total_src - 1) * u64::from(dst_rate)) / u64::from(self.src_rate)
        };

        if self.next_out >= max_out {
            return Vec::new();
        }

        let mut output = Vec::with_capacity((max_out - self.next_out) as usize);
        while self.next_out < max_out {
            let src_pos = self.next_out as f64 * f64::from(self.src_rate) / f64::from(dst_rate);
            let left = src_pos.floor() as usize;
            let right = (left + 1).min(self.src.len() - 1);
            let frac = (src_pos - left as f64) as f32;
            let sample =
                f32::from(self.src[left]) * (1.0 - frac) + f32::from(self.src[right]) * frac;
            output.push(sample as i16);
            self.next_out += 1;
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_f32(n: usize, freq_hz: f32, sample_rate: u32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (t * freq_hz * 2.0 * std::f32::consts::PI).sin() * 0.5
            })
            .collect()
    }

    #[test]
    fn resample_increases_sample_count_for_upsampling() {
        let input = sine_f32(4, 440.0, 22_050);
        let stereo = f32_mono_to_stereo_48k_s16le_raw(&input, 22_050);
        // stereo bytes = mono_i16_count * 4
        assert!(stereo.len() > input.len() * 4);
    }

    #[test]
    fn f32_to_stereo_produces_non_empty_pcm() {
        let samples = vec![0.0_f32, 0.5, -0.5, 0.25];
        let (pcm, duration_ms) = f32_mono_to_stereo_48k_s16le(&samples, 22_050);
        assert!(!pcm.is_empty());
        assert!(duration_ms >= 1);
        assert_eq!(pcm.len() % 4, 0);
        assert_eq!(pcm.len() % STEREO_FRAME_20MS_BYTES, 0);
    }

    #[test]
    fn align_stereo_pcm_pads_to_20ms_boundary() {
        let partial = Bytes::from(vec![0_u8; 1000]);
        let (aligned, duration_ms) = align_stereo_pcm_to_20ms(partial);
        assert_eq!(aligned.len() % STEREO_FRAME_20MS_BYTES, 0);
        assert!(aligned.len() > 1000);
        assert!(duration_ms >= 1);
    }

    #[test]
    fn streaming_chunked_matches_oneshot_raw() {
        let src_rate = 22_050_u32;
        let samples = sine_f32(8_000, 220.0, src_rate);
        let oneshot = f32_mono_to_stereo_48k_s16le_raw(&samples, src_rate);

        let mut stream = StreamingStereo48kResampler::new(src_rate);
        let mut chunked = Vec::new();
        for piece in samples.chunks(137) {
            chunked.extend_from_slice(&stream.push_f32(piece));
        }
        chunked.extend_from_slice(&stream.finish());

        assert_eq!(
            chunked.len(),
            oneshot.len(),
            "chunked len {} vs oneshot {}",
            chunked.len(),
            oneshot.len()
        );
        assert_eq!(
            Bytes::from(chunked),
            oneshot,
            "chunked progressive resample must match one-shot"
        );
    }

    #[test]
    fn streaming_uneven_chunks_match_oneshot() {
        let src_rate = 16_000_u32;
        let samples = sine_f32(3_333, 330.0, src_rate);
        let oneshot = f32_mono_to_stereo_48k_s16le_raw(&samples, src_rate);

        let mut stream = StreamingStereo48kResampler::new(src_rate);
        let mut chunked = Vec::new();
        let sizes = [1usize, 2, 7, 64, 255, 512, 1000];
        let mut offset = 0;
        for &size in &sizes {
            if offset >= samples.len() {
                break;
            }
            let end = (offset + size).min(samples.len());
            chunked.extend_from_slice(&stream.push_f32(&samples[offset..end]));
            offset = end;
        }
        if offset < samples.len() {
            chunked.extend_from_slice(&stream.push_f32(&samples[offset..]));
        }
        chunked.extend_from_slice(&stream.finish());

        assert_eq!(Bytes::from(chunked), oneshot);
    }

    #[test]
    fn streaming_passthrough_48k_matches_oneshot() {
        let samples = sine_f32(480, 440.0, WEBRTC_PCM_SAMPLE_RATE);
        let oneshot = f32_mono_to_stereo_48k_s16le_raw(&samples, WEBRTC_PCM_SAMPLE_RATE);
        let mut stream = StreamingStereo48kResampler::new(WEBRTC_PCM_SAMPLE_RATE);
        let mut chunked = Vec::new();
        for piece in samples.chunks(97) {
            chunked.extend_from_slice(&stream.push_f32(piece));
        }
        chunked.extend_from_slice(&stream.finish());
        assert_eq!(Bytes::from(chunked), oneshot);
    }
}
