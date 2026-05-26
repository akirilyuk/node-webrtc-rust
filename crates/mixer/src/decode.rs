//! Opus RTP payload decoding to 20 ms PCM frames.

use audiopus::coder::Decoder;
use audiopus::packet::Packet;
use audiopus::{Channels, MutSignals, SampleRate};
use bytes::Bytes;
use std::convert::TryFrom;
use thiserror::Error;

use crate::frame::{self, Frame, SAMPLES_PER_FRAME};

/// Errors from Opus decoding.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("Opus decoder error: {0}")]
    Opus(#[from] audiopus::Error),
}

/// Decodes RTP Opus payloads into fixed 20 ms stereo PCM frames.
pub struct OpusDecoder {
    inner: Decoder,
    pcm_scratch: [i16; SAMPLES_PER_FRAME],
}

impl OpusDecoder {
    /// Creates a decoder for 48 kHz stereo Opus (WebRTC default).
    pub fn new() -> Result<Self, DecodeError> {
        Ok(Self {
            inner: Decoder::new(SampleRate::Hz48000, Channels::Stereo)?,
            pcm_scratch: [0; SAMPLES_PER_FRAME],
        })
    }

    /// Decodes one Opus payload into a [`Frame`].
    ///
    /// Empty payloads and decode failures produce a silence frame (packet loss).
    pub fn decode_payload(&mut self, payload: &[u8]) -> Frame {
        if payload.is_empty() {
            return frame::silence_frame();
        }

        let packet = match Packet::try_from(payload) {
            Ok(packet) => Some(packet),
            Err(_) => return frame::silence_frame(),
        };

        let output = match MutSignals::try_from(&mut self.pcm_scratch[..]) {
            Ok(output) => output,
            Err(_) => return frame::silence_frame(),
        };

        match self.inner.decode(packet, output, false) {
            Ok(_) => pcm_to_frame(&self.pcm_scratch),
            Err(_) => frame::silence_frame(),
        }
    }
}

impl Default for OpusDecoder {
    fn default() -> Self {
        Self::new().expect("Opus decoder init")
    }
}

fn pcm_to_frame(samples: &[i16]) -> Frame {
    let mut pcm = vec![0u8; samples.len() * 2];
    for (idx, sample) in samples.iter().enumerate() {
        pcm[idx * 2..idx * 2 + 2].copy_from_slice(&sample.to_le_bytes());
    }
    Frame::new(Bytes::from(pcm), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use audiopus::coder::Encoder;
    use audiopus::{Application, Bitrate, Channels, SampleRate};

    fn encode_test_frame(samples: &[i16]) -> Vec<u8> {
        let mut encoder =
            Encoder::new(SampleRate::Hz48000, Channels::Stereo, Application::Voip).unwrap();
        encoder
            .set_bitrate(Bitrate::BitsPerSecond(64_000))
            .unwrap();
        let mut output = vec![0u8; 4_000];
        let len = encoder.encode(samples, &mut output).unwrap();
        output.truncate(len);
        output
    }

    #[test]
    fn decode_known_opus_frame_produces_3840_byte_pcm() {
        let samples: Vec<i16> = (0..SAMPLES_PER_FRAME)
            .map(|i| ((i as f32 * 0.01).sin() * 10_000.0) as i16)
            .collect();
        let payload = encode_test_frame(&samples);

        let mut decoder = OpusDecoder::new().unwrap();
        let frame = decoder.decode_payload(&payload);

        assert_eq!(frame.pcm.len(), frame::FRAME_BYTES);
        assert!(frame.is_valid());
    }

    #[test]
    fn empty_payload_returns_silence() {
        let mut decoder = OpusDecoder::new().unwrap();
        let frame = decoder.decode_payload(&[]);
        assert_eq!(frame, frame::silence_frame());
    }

    #[test]
    fn corrupt_payload_returns_silence() {
        let mut decoder = OpusDecoder::new().unwrap();
        let frame = decoder.decode_payload(&[0xFF, 0xFE, 0xFD]);
        assert_eq!(frame, frame::silence_frame());
    }
}
