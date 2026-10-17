//! 24 kHz mono s16le (Fish `format: "pcm"`, `sample_rate: 24000`) to 48 kHz stereo s16le.

use bytes::Bytes;
use node_webrtc_rust_speech::pcm::mono_s16le_to_stereo;

/// Streaming converter. Network chunks can split a sample in the middle, so a trailing odd
/// byte is kept for the next push.
#[derive(Debug, Default)]
pub(crate) struct Pcm24kAssembler {
    carry: Option<u8>,
}

impl Pcm24kAssembler {
    /// Returns 48 kHz stereo s16le for every complete 24 kHz sample received so far.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Bytes {
        let mut joined: Vec<u8> = Vec::with_capacity(bytes.len() + 1);
        if let Some(byte) = self.carry.take() {
            joined.push(byte);
        }
        joined.extend_from_slice(bytes);
        if joined.len() % 2 == 1 {
            self.carry = joined.pop();
        }
        upsample_24k_to_stereo_48k(&joined)
    }

    /// Drops a lone trailing byte (half a sample).
    pub(crate) fn finish(&mut self) {
        self.carry = None;
    }
}

/// Duplicate each 24 kHz sample (x2 to 48 kHz), then duplicate into both channels.
fn upsample_24k_to_stereo_48k(mono_24k: &[u8]) -> Bytes {
    let mut mono_48k = Vec::with_capacity(mono_24k.len() * 2);
    for sample in mono_24k.as_chunks::<2>().0 {
        mono_48k.extend_from_slice(sample);
        mono_48k.extend_from_slice(sample);
    }
    mono_s16le_to_stereo(&mono_48k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, WEBRTC_PCM_SAMPLE_RATE};

    #[test]
    fn four_hundred_eighty_bytes_become_1920() {
        let mut assembler = Pcm24kAssembler::default();
        assert_eq!(assembler.push(&[0_u8; 480]).len(), 1920);
    }

    #[test]
    fn odd_split_equals_one_push() {
        let input: Vec<u8> = (1..=6).collect();
        let mut whole = Pcm24kAssembler::default();
        let expected = whole.push(&input);

        let mut split = Pcm24kAssembler::default();
        let mut got = split.push(&input[..3]).to_vec();
        got.extend_from_slice(&split.push(&input[3..]));
        assert_eq!(got, expected.to_vec());
        assert_eq!(got.len(), 24);
    }

    #[test]
    fn finish_drops_a_lone_byte() {
        let mut assembler = Pcm24kAssembler::default();
        assert!(assembler.push(&[7]).is_empty());
        assembler.finish();
        assert_eq!(assembler.push(&[1, 2]).len(), 8);
    }

    #[test]
    fn samples_are_duplicated_in_time_and_channels() {
        let mut assembler = Pcm24kAssembler::default();
        let out = assembler.push(&[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(
            out.to_vec(),
            vec![
                1, 2, 1, 2, 1, 2, 1, 2, // sample 0: two 48 kHz frames, L and R
                3, 4, 3, 4, 3, 4, 3, 4, // sample 1
            ]
        );
    }

    #[test]
    fn duration_matches_between_24k_mono_and_48k_stereo() {
        let mut assembler = Pcm24kAssembler::default();
        let input = vec![0_u8; 4800];
        let out = assembler.push(&input);
        assert_eq!(
            duration_ms_from_mono_s16le(input.len(), 24_000),
            duration_ms_from_mono_s16le(out.len() / 2, WEBRTC_PCM_SAMPLE_RATE)
        );
    }
}
