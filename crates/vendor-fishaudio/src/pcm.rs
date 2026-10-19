//! 24 kHz mono s16le (Fish `format: "pcm"`, `sample_rate: 24000`) to 48 kHz stereo s16le.

use bytes::Bytes;
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pcm::MonoToStereo48k;

const FISH_PCM_RATE: u32 = 24_000;

/// Streaming converter: one instance per synthesis. Linear interpolation across chunk
/// boundaries, and a trailing odd byte of a network chunk is kept for the next push.
pub(crate) struct Pcm24kAssembler {
    up: MonoToStereo48k,
}

impl Pcm24kAssembler {
    pub(crate) fn new() -> SpeechResult<Self> {
        Ok(Self {
            up: MonoToStereo48k::new(FISH_PCM_RATE)?,
        })
    }

    /// Returns 48 kHz stereo s16le for the samples that are complete so far.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Bytes {
        Bytes::from(self.up.push(bytes))
    }

    /// Flushes the tail (the last sample held to the end of its interval). Call once, at the
    /// end of an uncancelled synthesis. A lone trailing byte (half a sample) is dropped.
    pub(crate) fn finish(&mut self) -> Bytes {
        Bytes::from(self.up.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::pcm::{duration_ms_from_stereo_48k, WEBRTC_PCM_SAMPLE_RATE};

    fn whole(input: &[u8]) -> Vec<u8> {
        let mut a = Pcm24kAssembler::new().unwrap();
        let mut out = a.push(input).to_vec();
        out.extend_from_slice(&a.finish());
        out
    }

    #[test]
    fn four_hundred_eighty_bytes_become_1920_after_finish() {
        assert_eq!(whole(&[0_u8; 480]).len(), 1920);
    }

    #[test]
    fn odd_split_equals_one_push() {
        let input: Vec<u8> = (1..=6).collect();
        let expected = whole(&input);

        let mut split = Pcm24kAssembler::new().unwrap();
        let mut got = split.push(&input[..3]).to_vec();
        got.extend_from_slice(&split.push(&input[3..]));
        got.extend_from_slice(&split.finish());
        assert_eq!(got, expected);
        assert_eq!(got.len(), 24);
    }

    #[test]
    fn finish_drops_a_lone_byte() {
        let mut a = Pcm24kAssembler::new().unwrap();
        assert!(a.push(&[7]).is_empty());
        assert!(a.finish().is_empty());
    }

    #[test]
    fn samples_are_interpolated_not_held() {
        let out = whole(&[0, 0, 100, 0]);
        let left: Vec<i16> = out
            .as_chunks::<4>()
            .0
            .iter()
            .map(|f| i16::from_le_bytes([f[0], f[1]]))
            .collect();
        assert_eq!(left, vec![0, 50, 100, 100]);
    }

    #[test]
    fn duration_matches_between_24k_mono_and_48k_stereo() {
        let input = vec![0_u8; 4800];
        let out = whole(&input);
        let ms = duration_ms_from_stereo_48k(out.len());
        let src = (input.len() / 2) as u32 * 1000 / FISH_PCM_RATE;
        assert_eq!(ms, src);
        assert_eq!(WEBRTC_PCM_SAMPLE_RATE, 48_000);
    }
}
