use bytes::{Bytes, BytesMut};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, WEBRTC_PCM_SAMPLE_RATE};

/// Stereo 48 kHz s16le frame size for 20 ms (Opus-compatible).
pub const STEREO_FRAME_20MS_BYTES: usize = 3840;

const WEBRTC_PCM_CHANNELS: usize = 2;

/// Upper bound for one progressive sink chunk: 1 s of stereo 48 kHz s16le
/// (a multiple of [`STEREO_FRAME_20MS_BYTES`]). Keeps every gRPC message small.
pub const SINK_SLICE_MAX_BYTES: usize = 192_000;

/// Split `pcm` into zero-copy pieces of at most [`SINK_SLICE_MAX_BYTES`].
pub fn slice_for_sink(pcm: &Bytes) -> impl Iterator<Item = Bytes> + '_ {
    (0..pcm.len())
        .step_by(SINK_SLICE_MAX_BYTES)
        .map(move |start| pcm.slice(start..(start + SINK_SLICE_MAX_BYTES).min(pcm.len())))
}

/// Convert mono f32 PCM at `src_rate` Hz to stereo 48 kHz s16le for WebRTC outbound tracks.
/// Pads the result to a 20 ms frame boundary (legacy full-utterance path).
///
/// TTS now streams through [`StreamingStereo48kResampler`]; this one-shot form stays for the
/// resample bench (`benches/resample.rs`) and the equivalence tests below.
#[allow(dead_code)]
pub fn f32_mono_to_stereo_48k_s16le(samples: &[f32], src_rate: u32) -> (Bytes, u32) {
    let stereo = f32_mono_to_stereo_48k_s16le_raw(samples, src_rate);
    if stereo.is_empty() {
        return (Bytes::new(), 1);
    }
    align_stereo_pcm_to_20ms(stereo)
}

/// Convert without 20 ms padding — used for progressive TTS deltas (drain pads frames).
#[allow(dead_code)]
pub fn f32_mono_to_stereo_48k_s16le_raw(samples: &[f32], src_rate: u32) -> Bytes {
    let mut stream = StreamingStereo48kResampler::new(src_rate);
    let estimate =
        (samples.len() as u64 * u64::from(WEBRTC_PCM_SAMPLE_RATE) / u64::from(src_rate.max(1)) + 2)
            * 4;
    let mut out = BytesMut::with_capacity(estimate as usize);
    out.extend_from_slice(&stream.push_f32(samples));
    out.extend_from_slice(&stream.finish());
    out.freeze()
}

/// Extra zero bytes needed to round `len` up to a multiple of [`STEREO_FRAME_20MS_BYTES`].
fn pad_len_to_20ms(len: usize) -> usize {
    match len % STEREO_FRAME_20MS_BYTES {
        0 => 0,
        remainder => STEREO_FRAME_20MS_BYTES - remainder,
    }
}

/// Pad trailing silence so stereo PCM length is a multiple of 20 ms @ 48 kHz.
pub fn align_stereo_pcm_to_20ms(stereo: Bytes) -> (Bytes, u32) {
    if stereo.is_empty() {
        return (stereo, 1);
    }

    let pad = pad_len_to_20ms(stereo.len());
    let aligned = if pad == 0 {
        stereo
    } else {
        let mut padded = stereo.to_vec();
        padded.resize(stereo.len() + pad, 0);
        Bytes::from(padded)
    };

    let duration_ms = stereo_duration_ms(aligned.len());
    (aligned, duration_ms)
}

/// In-place variant of [`align_stereo_pcm_to_20ms`]: pads `pcm` with zeros to a multiple of
/// [`STEREO_FRAME_20MS_BYTES`] without copying the buffer and returns the duration in ms.
/// Empty input stays empty and returns 1, like the `Bytes` form.
pub fn pad_stereo_pcm_to_20ms(pcm: &mut Vec<u8>) -> u32 {
    if pcm.is_empty() {
        return 1;
    }
    let pad = pad_len_to_20ms(pcm.len());
    if pad != 0 {
        pcm.resize(pcm.len() + pad, 0);
    }
    stereo_duration_ms(pcm.len())
}

fn stereo_duration_ms(stereo_len: usize) -> u32 {
    duration_ms_from_mono_s16le(stereo_len / WEBRTC_PCM_CHANNELS, WEBRTC_PCM_SAMPLE_RATE)
}

/// Continuous mono→stereo 48 kHz converter for progressive TTS deltas.
///
/// Chunked [`Self::push_f32`] + [`Self::finish`] matches one-shot
/// [`f32_mono_to_stereo_48k_s16le_raw`] on the concatenated source (same linear
/// resample phase across chunk boundaries — avoids STT dropouts from per-chunk resample).
///
/// Memory is bounded: source samples that no later output can reference are dropped after
/// every emit, so only a couple of samples stay buffered regardless of utterance length.
pub struct StreamingStereo48kResampler {
    src_rate: u32,
    /// Source samples (i16, same quantization as the one-shot path) from absolute index
    /// `base` up to `total_src`.
    src: Vec<i16>,
    /// Absolute source index of `src[0]`.
    base: u64,
    /// Source samples pushed so far.
    total_src: u64,
    /// Next 48 kHz mono output index in the continuous stream.
    next_out: u64,
}

impl StreamingStereo48kResampler {
    pub fn new(src_rate: u32) -> Self {
        Self {
            src_rate: src_rate.max(1),
            src: Vec::new(),
            base: 0,
            total_src: 0,
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
        self.total_src += samples.len() as u64;
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
        let stereo = if self.src_rate == dst_rate {
            self.emit_passthrough()
        } else {
            self.emit_linear(dst_rate, finished)
        };
        stereo.freeze()
    }

    fn emit_passthrough(&mut self) -> BytesMut {
        // Every source sample maps 1:1 and consumed samples are dropped, so `src` holds
        // exactly what has not been emitted yet.
        let mut out = BytesMut::zeroed(self.src.len() * 4);
        for (frame, &sample) in out.chunks_exact_mut(4).zip(&self.src) {
            write_stereo_sample(frame, sample);
        }
        self.src.clear();
        self.base = self.total_src;
        self.next_out = self.total_src;
        out
    }

    fn emit_linear(&mut self, dst_rate: u32, finished: bool) -> BytesMut {
        let total_src = self.total_src;
        let max_out = if finished {
            // Match `resample_linear_i16` batch length.
            ((total_src * u64::from(dst_rate)) / u64::from(self.src_rate)).max(1)
        } else {
            // Need `left + 1` available without clamping to the final sample.
            if total_src < 2 {
                return BytesMut::new();
            }
            ((total_src - 1) * u64::from(dst_rate)) / u64::from(self.src_rate)
        };

        if self.next_out >= max_out {
            return BytesMut::new();
        }

        let first_out = self.next_out;
        let count = (max_out - first_out) as usize;
        let mut output = BytesMut::zeroed(count * 4);
        for (i, frame) in output.chunks_exact_mut(4).enumerate() {
            let src_pos =
                (first_out + i as u64) as f64 * f64::from(self.src_rate) / f64::from(dst_rate);
            let left_abs = src_pos.floor() as u64;
            let right_abs = (left_abs + 1).min(total_src - 1);
            let frac = (src_pos - left_abs as f64) as f32;
            let left = f32::from(self.src[(left_abs - self.base) as usize]);
            let right = f32::from(self.src[(right_abs - self.base) as usize]);
            let sample = left * (1.0 - frac) + right * frac;
            write_stereo_sample(frame, sample as i16);
        }
        self.next_out = max_out;

        if finished {
            self.src.clear();
            self.base = self.total_src;
        } else {
            // Later outputs start at the same `floor(next_out * src_rate / dst_rate)`.
            let keep_from =
                ((self.next_out as f64 * f64::from(self.src_rate) / f64::from(dst_rate)).floor()
                    as u64)
                    .min(total_src - 1)
                    .max(self.base);
            self.src.drain(..(keep_from - self.base) as usize);
            self.base = keep_from;
        }
        output
    }
}

/// Write one mono sample into a 4-byte interleaved stereo s16le frame (L = R).
fn write_stereo_sample(frame: &mut [u8], sample: i16) {
    let [lo, hi] = sample.to_le_bytes();
    frame.copy_from_slice(&[lo, hi, lo, hi]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::pcm::mono_s16le_to_stereo;

    fn sine_f32(n: usize, freq_hz: f32, sample_rate: u32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (t * freq_hz * 2.0 * std::f32::consts::PI).sin() * 0.5
            })
            .collect()
    }

    #[test]
    fn slice_for_sink_splits_and_reassembles() {
        let input = Bytes::from(
            (0..500_000_u32)
                .map(|i| (i % 251) as u8)
                .collect::<Vec<u8>>(),
        );
        let slices: Vec<Bytes> = slice_for_sink(&input).collect();
        let lengths: Vec<usize> = slices.iter().map(Bytes::len).collect();
        assert_eq!(lengths, vec![192_000, 192_000, 116_000]);
        let joined: Vec<u8> = slices.iter().flat_map(|s| s.iter().copied()).collect();
        assert_eq!(Bytes::from(joined), input);
    }

    #[test]
    fn slice_for_sink_empty_yields_nothing() {
        let input = Bytes::new();
        assert_eq!(slice_for_sink(&input).count(), 0);
    }

    #[test]
    fn slice_for_sink_exact_multiple() {
        let input = Bytes::from(vec![7_u8; 384_000]);
        let lengths: Vec<usize> = slice_for_sink(&input).map(|s| s.len()).collect();
        assert_eq!(lengths, vec![192_000, 192_000]);
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

    /// Verbatim copy of the pre-bounded resampler (keeps every source sample). Test-only oracle
    /// that guards the drain math in [`StreamingStereo48kResampler`].
    struct ReferenceResampler {
        src_rate: u32,
        src: Vec<i16>,
        next_out: u64,
    }

    impl ReferenceResampler {
        fn new(src_rate: u32) -> Self {
            Self {
                src_rate: src_rate.max(1),
                src: Vec::new(),
                next_out: 0,
            }
        }

        fn push_f32(&mut self, samples: &[f32]) -> Bytes {
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

        fn finish(&mut self) -> Bytes {
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
                ((total_src * u64::from(dst_rate)) / u64::from(self.src_rate)).max(1)
            } else {
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

    #[test]
    fn streaming_resampler_keeps_bounded_source() {
        let src_rate = 22_050_u32;
        let samples = sine_f32(src_rate as usize * 10, 220.0, src_rate);
        let oneshot = f32_mono_to_stereo_48k_s16le_raw(&samples, src_rate);

        let mut resampler = StreamingStereo48kResampler::new(src_rate);
        let mut out = Vec::new();
        for piece in samples.chunks(1024) {
            out.extend_from_slice(&resampler.push_f32(piece));
            assert!(
                resampler.src.len() <= 2,
                "source buffer grew to {}",
                resampler.src.len()
            );
        }
        out.extend_from_slice(&resampler.finish());
        assert!(resampler.src.is_empty());
        assert_eq!(Bytes::from(out), oneshot);
    }

    #[test]
    fn streaming_resampler_bounded_for_uneven_rates() {
        for src_rate in [16_000_u32, 24_000, 44_100] {
            let samples = sine_f32(src_rate as usize / 2, 330.0, src_rate);
            for chunk in [1_usize, 7, 333] {
                let mut reference = ReferenceResampler::new(src_rate);
                let mut expected = Vec::new();
                for piece in samples.chunks(chunk) {
                    expected.extend_from_slice(&reference.push_f32(piece));
                }
                expected.extend_from_slice(&reference.finish());

                let mut resampler = StreamingStereo48kResampler::new(src_rate);
                let mut actual = Vec::new();
                for piece in samples.chunks(chunk) {
                    actual.extend_from_slice(&resampler.push_f32(piece));
                    assert!(
                        resampler.src.len() <= chunk + 2,
                        "rate {src_rate} chunk {chunk}: source buffer grew to {}",
                        resampler.src.len()
                    );
                }
                actual.extend_from_slice(&resampler.finish());
                assert!(resampler.src.is_empty());
                assert_eq!(
                    actual, expected,
                    "rate {src_rate} chunk {chunk}: output differs from reference"
                );
            }
        }
    }

    #[test]
    fn pad_stereo_pcm_to_20ms_matches_align() {
        for len in [0_usize, 4, 3840, 3844, 7676] {
            let input: Vec<u8> = (0..len).map(|i| (i % 251) as u8 + 1).collect();
            let (aligned, aligned_ms) = align_stereo_pcm_to_20ms(Bytes::from(input.clone()));
            let mut padded = input;
            let padded_ms = pad_stereo_pcm_to_20ms(&mut padded);
            assert_eq!(padded, aligned.to_vec(), "len {len}");
            assert_eq!(padded_ms, aligned_ms, "len {len}");
        }
    }
}
