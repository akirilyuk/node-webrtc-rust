//! PCM → negotiated RTP payload encoding.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate};
use bytes::Bytes;
use webrtc::api::media_engine::MIME_TYPE_OPUS;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters;

use crate::error::CoreError;

const OPUS_OUTPUT_CAPACITY: usize = 4_000;

/// Audio format agreed during SDP negotiation for one track binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedAudioFormat {
    pub mime_type: String,
    pub clock_rate: u32,
    pub channels: u16,
}

impl NegotiatedAudioFormat {
    /// Default advertised WebRTC Opus (48 kHz stereo) before bind completes.
    pub fn advertised_opus() -> Self {
        Self {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: 48_000,
            channels: 2,
        }
    }

    /// Builds from the codec returned by [`TrackLocal::bind`](webrtc::track::track_local::TrackLocal::bind).
    pub fn from_codec(codec: &RTCRtpCodecParameters) -> Result<Self, CoreError> {
        let mime_type = codec.capability.mime_type.clone();
        if !mime_type.to_ascii_lowercase().starts_with("audio/") {
            return Err(CoreError::Track(format!(
                "expected audio codec, got {mime_type}"
            )));
        }

        Ok(Self {
            mime_type,
            clock_rate: codec.capability.clock_rate,
            channels: codec.capability.channels,
        })
    }

    fn is_opus(&self) -> bool {
        self.mime_type
            .to_ascii_lowercase()
            .contains("opus")
    }
}

/// Encodes interleaved stereo PCM into payloads for the negotiated codec.
#[derive(Clone)]
pub struct PcmEncoder {
    opus: Arc<Mutex<OpusEncoderState>>,
}

struct OpusEncoderState {
    encoder: Encoder,
    pcm_scratch: Vec<i16>,
    opus_scratch: Vec<u8>,
}

impl PcmEncoder {
    pub fn new() -> Result<Self, CoreError> {
        let mut encoder =
            Encoder::new(SampleRate::Hz48000, Channels::Stereo, Application::Voip)
                .map_err(|e| CoreError::Track(format!("Opus encoder init: {e}")))?;
        encoder
            .set_bitrate(Bitrate::BitsPerSecond(64_000))
            .map_err(|e| CoreError::Track(format!("Opus encoder bitrate: {e}")))?;

        Ok(Self {
            opus: Arc::new(Mutex::new(OpusEncoderState {
                encoder,
                pcm_scratch: Vec::new(),
                opus_scratch: vec![0u8; OPUS_OUTPUT_CAPACITY],
            })),
        })
    }

    /// Encodes PCM using the negotiated (or advertised default) audio format.
    pub fn encode(
        &self,
        format: &NegotiatedAudioFormat,
        pcm: &[u8],
        duration: Duration,
    ) -> Result<(Bytes, Duration), CoreError> {
        if format.is_opus() {
            self.encode_opus(format, pcm, duration)
        } else {
            Err(CoreError::Track(format!(
                "PCM writeSample does not support negotiated codec {} yet",
                format.mime_type
            )))
        }
    }

    fn encode_opus(
        &self,
        format: &NegotiatedAudioFormat,
        pcm: &[u8],
        duration: Duration,
    ) -> Result<(Bytes, Duration), CoreError> {
        if format.clock_rate != 48_000 || format.channels != 2 {
            return Err(CoreError::Track(format!(
                "Opus encoder supports 48 kHz stereo only (negotiated {} Hz {} ch)",
                format.clock_rate, format.channels
            )));
        }

        let samples_per_channel = samples_per_channel_for_duration(duration, pcm.len())?;
        let expected_bytes = samples_per_channel * format.channels as usize * 2;
        if pcm.len() != expected_bytes {
            return Err(CoreError::Track(format!(
                "PCM length {} does not match {} ms frame (expected {} bytes)",
                pcm.len(),
                duration.as_millis(),
                expected_bytes
            )));
        }

        let mut state = self
            .opus
            .lock()
            .map_err(|_| CoreError::Track("Opus encoder lock poisoned".into()))?;

        state.pcm_scratch.clear();
        state.pcm_scratch.reserve(pcm.len() / 2);
        for chunk in pcm.chunks_exact(2) {
            state.pcm_scratch.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }

        let pcm_samples = std::mem::take(&mut state.pcm_scratch);
        let mut opus_buf = std::mem::take(&mut state.opus_scratch);
        let len = state
            .encoder
            .encode(&pcm_samples, &mut opus_buf)
            .map_err(|e| CoreError::Track(format!("Opus encode: {e}")))?;
        state.pcm_scratch = pcm_samples;
        state.opus_scratch = opus_buf;

        if len == 0 {
            return Err(CoreError::Track("Opus encoder produced empty payload".into()));
        }

        Ok((
            Bytes::copy_from_slice(&state.opus_scratch[..len]),
            duration,
        ))
    }
}

fn samples_per_channel_for_duration(duration: Duration, pcm_len: usize) -> Result<usize, CoreError> {
    let from_duration =
        ((duration.as_micros() as u64 * 48_000) / 1_000_000) as usize;
    let from_buffer = pcm_len / (2 * 2);

    if from_duration == 0 && from_buffer > 0 {
        return Ok(from_buffer);
    }

    if from_duration != from_buffer {
        return Err(CoreError::Track(format!(
            "PCM duration mismatch: {duration:?} implies {from_duration} samples/channel, buffer implies {from_buffer}"
        )));
    }

    Ok(from_duration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use audiopus::coder::Decoder;
    use audiopus::packet::Packet;
    use audiopus::{MutSignals, SampleRate};

    #[test]
    fn encodes_pcm_for_advertised_opus() {
        let encoder = PcmEncoder::new().unwrap();
        let format = NegotiatedAudioFormat::advertised_opus();
        let pcm = vec![0u8; 3_840];
        let (opus, _) = encoder
            .encode(&format, &pcm, Duration::from_millis(20))
            .unwrap();
        assert!(!opus.is_empty());
        assert!(opus.len() < pcm.len());
    }

    #[test]
    fn encodes_pcm_for_negotiated_opus_codec() {
        let encoder = PcmEncoder::new().unwrap();
        let format = NegotiatedAudioFormat::from_codec(&RTCRtpCodecParameters {
            capability: webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48_000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();

        let (opus, _) = encoder
            .encode(&format, &[0u8; 960], Duration::from_millis(5))
            .unwrap();
        assert!(!opus.is_empty());
    }

    #[test]
    fn roundtrip_20ms_frame() {
        let encoder = PcmEncoder::new().unwrap();
        let format = NegotiatedAudioFormat::advertised_opus();
        let mut pcm = vec![0u8; 3_840];
        for (idx, chunk) in pcm.chunks_mut(2).enumerate() {
            let sample = ((idx as f32 * 0.01).sin() * 10_000.0) as i16;
            chunk.copy_from_slice(&sample.to_le_bytes());
        }

        let (opus, _) = encoder
            .encode(&format, &pcm, Duration::from_millis(20))
            .unwrap();

        let mut decoder = Decoder::new(SampleRate::Hz48000, Channels::Stereo).unwrap();
        let mut out = [0i16; 1_920];
        let packet = Packet::try_from(opus.as_ref()).unwrap();
        let decoded = MutSignals::try_from(&mut out[..]).unwrap();
        decoder.decode(Some(packet), decoded, false).unwrap();
    }

    #[test]
    fn advertised_opus_defaults() {
        let format = NegotiatedAudioFormat::advertised_opus();
        assert_eq!(format.mime_type, MIME_TYPE_OPUS);
        assert_eq!(format.clock_rate, 48_000);
        assert_eq!(format.channels, 2);
    }

    #[test]
    fn from_codec_rejects_non_audio_mime() {
        let err = NegotiatedAudioFormat::from_codec(&RTCRtpCodecParameters {
            capability: webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                mime_type: "video/VP8".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("expected audio codec"));
    }

    #[test]
    fn encode_rejects_unsupported_codec() {
        let encoder = PcmEncoder::new().unwrap();
        let format = NegotiatedAudioFormat {
            mime_type: "audio/pcmu".to_owned(),
            clock_rate: 8_000,
            channels: 1,
        };
        let err = encoder
            .encode(&format, &[0u8; 160], Duration::from_millis(20))
            .unwrap_err();
        assert!(err.to_string().contains("does not support"));
    }

    #[test]
    fn encode_rejects_mono_opus_negotiation() {
        let encoder = PcmEncoder::new().unwrap();
        let format = NegotiatedAudioFormat {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: 48_000,
            channels: 1,
        };
        let err = encoder
            .encode(&format, &[0u8; 960], Duration::from_millis(10))
            .unwrap_err();
        assert!(err.to_string().contains("48 kHz stereo only"));
    }

    #[test]
    fn encode_rejects_duration_buffer_mismatch() {
        let encoder = PcmEncoder::new().unwrap();
        let format = NegotiatedAudioFormat::advertised_opus();
        let err = encoder
            .encode(&format, &[0u8; 3_840], Duration::from_millis(10))
            .unwrap_err();
        assert!(err.to_string().contains("duration mismatch"));
    }

    #[test]
    fn encodes_5ms_prime_frame() {
        let encoder = PcmEncoder::new().unwrap();
        let format = NegotiatedAudioFormat::advertised_opus();
        let pcm = vec![0u8; 960];
        let (opus, duration) = encoder
            .encode(&format, &pcm, Duration::from_millis(5))
            .unwrap();
        assert_eq!(duration, Duration::from_millis(5));
        assert!(!opus.is_empty());
        assert!(opus.len() < pcm.len());
    }

    #[test]
    fn consecutive_frames_produce_decodable_opus() {
        let encoder = PcmEncoder::new().unwrap();
        let format = NegotiatedAudioFormat::advertised_opus();
        let mut decoder = Decoder::new(SampleRate::Hz48000, Channels::Stereo).unwrap();

        for frame in 0..5 {
            let mut pcm = vec![0u8; 960];
            for (idx, chunk) in pcm.chunks_mut(2).enumerate() {
                let t = (frame * 480 + idx) as f32 / 48_000.0;
                let sample = (440.0 * t * std::f32::consts::TAU).sin() * 10_000.0;
                chunk.copy_from_slice(&(sample as i16).to_le_bytes());
            }
            let (opus, duration) = encoder
                .encode(&format, &pcm, Duration::from_millis(5))
                .unwrap();
            assert_eq!(duration, Duration::from_millis(5));
            assert!(!opus.is_empty());

            let mut out = [0i16; 480];
            let packet = Packet::try_from(opus.as_ref()).unwrap();
            let decoded = MutSignals::try_from(&mut out[..]).unwrap();
            decoder.decode(Some(packet), decoded, false).unwrap();
        }
    }

    #[test]
    fn non_silent_pcm_produces_varying_opus_payloads() {
        let encoder = PcmEncoder::new().unwrap();
        let format = NegotiatedAudioFormat::advertised_opus();

        let silent = vec![0u8; 3_840];
        let (silent_opus, _) = encoder
            .encode(&format, &silent, Duration::from_millis(20))
            .unwrap();

        let mut tone = vec![0u8; 3_840];
        for (idx, chunk) in tone.chunks_mut(2).enumerate() {
            let sample = ((idx as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin()
                * 10_000.0) as i16;
            chunk.copy_from_slice(&sample.to_le_bytes());
        }
        let (tone_opus, _) = encoder
            .encode(&format, &tone, Duration::from_millis(20))
            .unwrap();

        assert_ne!(silent_opus, tone_opus);
    }
}
