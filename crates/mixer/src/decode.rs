//! Opus RTP payload decoding to 20 ms PCM frames.

use audiopus::coder::Decoder;
use audiopus::packet::Packet;
use audiopus::{Channels, MutSignals, SampleRate};
use bytes::Bytes;
use std::convert::TryFrom;
use thiserror::Error;

use crate::frame::{Frame, SAMPLES_PER_FRAME};

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

/// Longest Opus packet: 120 ms at 48 kHz.
const MAX_PACKET_SAMPLES_PER_CHANNEL: usize = 5_760;

/// Samples per channel of a 20 ms packet at 48 kHz (initial `last_packet_samples`).
const DEFAULT_PACKET_SAMPLES: usize = 960;

/// Decodes RTP Opus payloads (2.5 to 120 ms) into 20 ms stereo PCM frames.
///
/// A payload may hold several 20 ms frames (e.g. a 60 ms packet decodes to three). A packet
/// that is not a multiple of 20 ms (2.5, 5 or 10 ms) ends with a zero-padded frame, so
/// each packet starts on a frame boundary.
pub struct OpusDecoder {
    inner: Decoder,
    pcm_scratch: Vec<i16>,
    last_packet_samples: usize,
}

impl OpusDecoder {
    /// Creates a decoder for 48 kHz stereo Opus (WebRTC default).
    pub fn new() -> Result<Self, DecodeError> {
        Ok(Self {
            inner: Decoder::new(SampleRate::Hz48000, Channels::Stereo)?,
            pcm_scratch: vec![0; MAX_PACKET_SAMPLES_PER_CHANNEL * 2],
            last_packet_samples: DEFAULT_PACKET_SAMPLES,
        })
    }

    /// Decodes one Opus payload into zero or more 20 ms [`Frame`]s, in order.
    ///
    /// Returns no frames when the payload is empty or undecodable. Silence is never substituted for a payload that decodes; use
    /// [`Self::try_decode_payload`] to observe errors.
    pub fn decode_payload(&mut self, payload: &[u8]) -> Vec<Frame> {
        self.try_decode_payload(payload).unwrap_or_default()
    }

    /// Like [`Self::decode_payload`] but surfaces why a payload produced no audio.
    pub fn try_decode_payload(&mut self, payload: &[u8]) -> Result<Vec<Frame>, DecodeError> {
        if payload.is_empty() {
            return Err(DecodeError::EmptyPayload);
        }

        let per_channel = {
            let packet =
                Packet::try_from(payload).map_err(|e| DecodeError::Packet(e.to_string()))?;
            audiopus::packet::nb_samples(packet, SampleRate::Hz48000)?
        };
        if per_channel == 0 || per_channel > MAX_PACKET_SAMPLES_PER_CHANNEL {
            return Err(DecodeError::Packet(format!(
                "unsupported packet length: {per_channel} samples per channel"
            )));
        }

        let packet = Packet::try_from(payload).map_err(|e| DecodeError::Packet(e.to_string()))?;
        let decoded = {
            let output = MutSignals::try_from(&mut self.pcm_scratch[..per_channel * 2])
                .map_err(|e| DecodeError::Output(e.to_string()))?;
            self.inner.decode(Some(packet), output, false)?
        };
        self.last_packet_samples = decoded;
        Ok(self.split_scratch(decoded))
    }

    /// Samples per channel of the last successfully decoded packet (default 960 = 20 ms).
    pub fn last_packet_samples(&self) -> usize {
        self.last_packet_samples
    }

    /// Packet loss concealment: asks libopus to synthesize `samples_per_channel` samples
    /// (clamped to 120 ms) for a lost packet. Does not change [`Self::last_packet_samples`].
    pub fn conceal_plc(&mut self, samples_per_channel: usize) -> Result<Vec<Frame>, DecodeError> {
        let n = samples_per_channel.clamp(1, MAX_PACKET_SAMPLES_PER_CHANNEL);
        let decoded = {
            let output = MutSignals::try_from(&mut self.pcm_scratch[..n * 2])
                .map_err(|e| DecodeError::Output(e.to_string()))?;
            self.inner.decode(None::<Packet>, output, false)?
        };
        Ok(self.split_scratch(decoded))
    }

    /// Recovers the packet that preceded `next_payload` from its in-band FEC data (libopus
    /// falls back to PLC when the packet carries none). Does not change
    /// [`Self::last_packet_samples`].
    pub fn decode_fec(
        &mut self,
        next_payload: &[u8],
        samples_per_channel: usize,
    ) -> Result<Vec<Frame>, DecodeError> {
        if next_payload.is_empty() {
            return Err(DecodeError::EmptyPayload);
        }
        let n = samples_per_channel.clamp(1, MAX_PACKET_SAMPLES_PER_CHANNEL);
        let packet =
            Packet::try_from(next_payload).map_err(|e| DecodeError::Packet(e.to_string()))?;
        let decoded = {
            let output = MutSignals::try_from(&mut self.pcm_scratch[..n * 2])
                .map_err(|e| DecodeError::Output(e.to_string()))?;
            self.inner.decode(Some(packet), output, true)?
        };
        Ok(self.split_scratch(decoded))
    }

    /// Splits the first `decoded` samples per channel of the scratch buffer into 20 ms
    /// frames; the last chunk is zero-padded.
    fn split_scratch(&self, decoded: usize) -> Vec<Frame> {
        let pcm = &self.pcm_scratch[..decoded * 2];
        let mut frames = Vec::with_capacity(decoded.div_ceil(SAMPLES_PER_FRAME / 2));
        for chunk in pcm.chunks(SAMPLES_PER_FRAME) {
            let mut samples = [0i16; SAMPLES_PER_FRAME];
            samples[..chunk.len()].copy_from_slice(chunk);
            frames.push(pcm_to_frame(&samples));
        }
        frames
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
    use crate::frame;
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

    fn tone(ms: usize) -> Vec<i16> {
        (0..48 * ms)
            .flat_map(|n| {
                let v = ((n as f32 * 440.0 * 2.0 * std::f32::consts::PI / 48_000.0).sin()
                    * 10_000.0) as i16;
                [v, v]
            })
            .collect()
    }

    fn rms(frame: &Frame) -> f64 {
        let n = frame.pcm.len() / 2;
        let sum: f64 = frame
            .pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f64::from(i16::from_le_bytes([c[0], c[1]])).powi(2))
            .sum();
        (sum / n as f64).sqrt()
    }

    #[test]
    fn decode_known_opus_frame_produces_3840_byte_pcm() {
        let payload = encode_test_frame(&tone(20));
        let mut decoder = OpusDecoder::new().unwrap();
        let frames = decoder.decode_payload(&payload);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].pcm.len(), frame::FRAME_BYTES);
        assert!(frames[0].is_valid());
    }

    #[test]
    fn packets_of_20_40_and_60_ms_decode_to_1_2_and_3_toned_frames() {
        for (ms, count) in [(20usize, 1usize), (40, 2), (60, 3)] {
            let payload = encode_test_frame(&tone(ms));
            let mut decoder = OpusDecoder::new().unwrap();
            // A second packet lets the codec settle; judge the last packet's frames.
            decoder.decode_payload(&payload);
            let frames = decoder.try_decode_payload(&payload).unwrap();
            assert_eq!(frames.len(), count, "{ms} ms packet");
            for (i, f) in frames.iter().enumerate() {
                assert!(f.is_valid());
                assert!(
                    rms(f) > 3_000.0,
                    "{ms} ms packet, frame {i} is not the tone (rms {})",
                    rms(f)
                );
            }
        }
    }

    #[test]
    fn sub_frame_packet_becomes_one_zero_padded_frame() {
        let payload = encode_test_frame(&tone(10));
        let mut decoder = OpusDecoder::new().unwrap();
        decoder.decode_payload(&payload);
        let frames = decoder.try_decode_payload(&payload).unwrap();
        assert_eq!(frames.len(), 1);
        assert!(frames[0].pcm[frames[0].pcm.len() / 2..]
            .iter()
            .all(|&b| b == 0));
    }

    #[test]
    fn empty_payload_is_an_error_and_yields_no_frames() {
        let mut decoder = OpusDecoder::new().unwrap();
        assert!(matches!(
            decoder.try_decode_payload(&[]),
            Err(DecodeError::EmptyPayload)
        ));
        assert!(decoder.decode_payload(&[]).is_empty());
    }

    #[test]
    fn corrupt_payload_is_an_error_and_does_not_panic() {
        let mut decoder = OpusDecoder::new().unwrap();
        assert!(decoder.try_decode_payload(&[0xFF, 0xFE, 0xFD]).is_err());
        assert!(decoder.decode_payload(&[0xFF, 0xFE, 0xFD]).is_empty());
        // Still usable afterwards.
        let payload = encode_test_frame(&tone(20));
        assert_eq!(decoder.decode_payload(&payload).len(), 1);
    }

    fn tone_at(start_sample: usize, ms: usize) -> Vec<i16> {
        (start_sample..start_sample + 48 * ms)
            .flat_map(|n| {
                let v = ((n as f32 * 440.0 * 2.0 * std::f32::consts::PI / 48_000.0).sin()
                    * 10_000.0) as i16;
                [v, v]
            })
            .collect()
    }

    fn frames_pcm(frames: &[Frame]) -> Vec<f64> {
        frames
            .iter()
            .flat_map(|f| {
                f.pcm
                    .chunks(4)
                    .map(|c| f64::from(i16::from_le_bytes([c[0], c[1]])))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn norm_corr(a: &[f64], b: &[f64]) -> f64 {
        let n = a.len().min(b.len());
        let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
        for i in 0..n {
            ab += a[i] * b[i];
            aa += a[i] * a[i];
            bb += b[i] * b[i];
        }
        if aa == 0.0 || bb == 0.0 {
            return 0.0;
        }
        ab / (aa.sqrt() * bb.sqrt())
    }

    #[test]
    fn fec_recovers_dropped_packet_and_plc_is_non_silent() {
        let mut enc =
            Encoder::new(SampleRate::Hz48000, Channels::Stereo, Application::Voip).unwrap();
        enc.set_bitrate(Bitrate::BitsPerSecond(32_000)).unwrap();
        enc.set_inband_fec(true).unwrap();
        enc.set_packet_loss_perc(20).unwrap();
        let mut packets = Vec::new();
        for i in 0..14usize {
            let pcm = tone_at(i * 960, 20);
            let mut out = vec![0u8; 4_000];
            let len = enc.encode(&pcm, &mut out).unwrap();
            out.truncate(len);
            packets.push(out);
        }
        let drop_idx = 12;
        let original = frames_pcm(&{
            // Reference: decode everything without loss.
            let mut d = OpusDecoder::new().unwrap();
            let mut all = Vec::new();
            for p in &packets[..=drop_idx] {
                all = d.try_decode_payload(p).unwrap();
            }
            all
        });

        // Lossy path: decode up to drop_idx-1, skip drop_idx, recover via FEC from drop_idx+1.
        let mut d = OpusDecoder::new().unwrap();
        for p in &packets[..drop_idx] {
            d.try_decode_payload(p).unwrap();
        }
        assert_eq!(d.last_packet_samples(), 960);
        let fec = frames_pcm(&d.decode_fec(&packets[drop_idx + 1], 960).unwrap());
        assert_eq!(d.last_packet_samples(), 960);
        let fec_corr = norm_corr(&fec, &original);
        let zero_corr = norm_corr(&vec![0.0; original.len()], &original);
        assert!(fec_corr > 0.5, "fec correlation {fec_corr}");
        assert!(zero_corr < 0.5, "zero-fill correlation {zero_corr}");

        // PLC after a tone returns non-silent frames.
        let mut d = OpusDecoder::new().unwrap();
        for p in &packets[..drop_idx] {
            d.try_decode_payload(p).unwrap();
        }
        let plc = d.conceal_plc(960).unwrap();
        assert_eq!(plc.len(), 1);
        assert!(rms(&plc[0]) > 100.0, "plc rms {}", rms(&plc[0]));

        // last_packet_samples follows a 60 ms packet.
        let payload = encode_test_frame(&tone(60));
        let mut d = OpusDecoder::new().unwrap();
        d.try_decode_payload(&payload).unwrap();
        assert_eq!(d.last_packet_samples(), 2_880);
    }
}
