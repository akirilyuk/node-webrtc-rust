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
    #[error("empty RTP payload")]
    EmptyPayload,
    #[error("Opus packet parse error: {0}")]
    Packet(String),
    #[error("Opus output buffer error: {0}")]
    Output(String),
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
        match self.try_decode_payload(payload) {
            Ok(frame) => frame,
            Err(_) => frame::silence_frame(),
        }
    }

    /// Like [`Self::decode_payload`] but surfaces why a payload was replaced by silence.
    pub fn try_decode_payload(&mut self, payload: &[u8]) -> Result<Frame, DecodeError> {
        if payload.is_empty() {
            return Err(DecodeError::EmptyPayload);
        }

        let packet =
            Packet::try_from(payload).map_err(|e| DecodeError::Packet(e.to_string()))?;

        let output = MutSignals::try_from(&mut self.pcm_scratch[..])
            .map_err(|e| DecodeError::Output(e.to_string()))?;

        self.inner.decode(Some(packet), output, false)?;
        Ok(pcm_to_frame(&self.pcm_scratch))
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
            Encoder::new(SampleRate::Hz48000, Channels::Stereo, Application::Audio).unwrap();
        encoder
            .set_bitrate(Bitrate::BitsPerSecond(192_000))
            .unwrap();
        encoder.set_complexity(10).unwrap();
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
