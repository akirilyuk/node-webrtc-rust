//! PCM → negotiated RTP payload encoding.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate};
use bytes::Bytes;
use webrtc::api::media_engine::MIME_TYPE_OPUS;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters;

use crate::error::CoreError;

/// Max Opus payload per RTP packet: outbound RTP MTU 1200 (webrtc-rs `RTP_OUTBOUND_MTU`) minus RTP header (12 B), header extensions and SRTP auth tag headroom.
pub const OPUS_MAX_PAYLOAD_BYTES: usize = 1000;

/// Default Opus encode bitrate (stereo Audio mode) when `WEBRTC_OPUS_BITRATE_BPS` is unset.
pub const OPUS_TARGET_BITRATE_BPS: i32 = 400_000;

const OPUS_BITRATE_MIN_BPS: i32 = 6_000;
const OPUS_BITRATE_MAX_BPS: i32 = 510_000;

static RESOLVED_OPUS_BITRATE_BPS_FROM_ENV: OnceLock<Option<i32>> = OnceLock::new();
static RESOLVED_OPUS_APPLICATION: OnceLock<Application> = OnceLock::new();

/// Parsed `WEBRTC_OPUS_BITRATE_BPS` (process-wide, read once).
///
/// `None` when unset, blank, or invalid — SDP omits `maxaveragebitrate`; encode uses
/// [`OPUS_TARGET_BITRATE_BPS`].
pub fn opus_bitrate_bps_from_env() -> Option<i32> {
    *RESOLVED_OPUS_BITRATE_BPS_FROM_ENV.get_or_init(|| {
        match std::env::var("WEBRTC_OPUS_BITRATE_BPS") {
            Ok(raw) => parse_opus_bitrate_bps_from_env(Some(raw.as_str())),
            Err(_) => None,
        }
    })
}

/// Opus encode bitrate: env override when set, otherwise [`OPUS_TARGET_BITRATE_BPS`].
pub fn opus_target_bitrate_bps() -> i32 {
    opus_bitrate_bps_from_env().unwrap_or(OPUS_TARGET_BITRATE_BPS)
}

fn resolved_opus_application() -> Application {
    *RESOLVED_OPUS_APPLICATION.get_or_init(|| match std::env::var("WEBRTC_OPUS_APPLICATION") {
        Ok(raw) => parse_opus_application(Some(raw.as_str())),
        Err(_) => Application::Audio,
    })
}

fn parse_opus_bitrate_bps_from_env(raw: Option<&str>) -> Option<i32> {
    let Some(raw) = raw else {
        return None;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<i32>() {
        Ok(v) => Some(v.clamp(OPUS_BITRATE_MIN_BPS, OPUS_BITRATE_MAX_BPS)),
        Err(_) => {
            eprintln!(
                "WEBRTC_OPUS_BITRATE_BPS={trimmed:?} invalid; omitting maxaveragebitrate from SDP and using encode default {OPUS_TARGET_BITRATE_BPS}"
            );
            None
        }
    }
}

fn parse_opus_application(raw: Option<&str>) -> Application {
    let Some(raw) = raw else {
        return Application::Audio;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Application::Audio;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "audio" => Application::Audio,
        "voip" => Application::Voip,
        "lowdelay" => Application::LowDelay,
        other => {
            eprintln!("WEBRTC_OPUS_APPLICATION={other:?} invalid; using default \"audio\"");
            Application::Audio
        }
    }
}

/// Opus SDP fmtp advertised on local PCM tracks (FEC + stereo; optional bitrate cap).
pub fn opus_sdp_fmtp_line() -> String {
    let mut line = "minptime=10;useinbandfec=1;stereo=1".to_string();
    if let Some(bps) = opus_bitrate_bps_from_env() {
        line.push_str(&format!(";maxaveragebitrate={bps}"));
    }
    line
}

/// Rewrite Opus `a=fmtp:` lines to our encode/advertise params.
///
/// Needed for answers: webrtc-rs copies the remote offer's weak default fmtp
/// (`minptime=10;useinbandfec=1`) even when the local MediaEngine has stereo + FEC fmtp.
pub fn enrich_opus_sdp_fmtp(sdp: &str) -> String {
    let target = opus_sdp_fmtp_line();
    let mut opus_pts = std::collections::HashSet::new();
    for line in sdp.lines() {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let Some(rest) = trimmed.strip_prefix("a=rtpmap:") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let (Some(pt), Some(codec)) = (parts.next(), parts.next()) else {
            continue;
        };
        if codec.to_ascii_lowercase().starts_with("opus/") {
            opus_pts.insert(pt.to_string());
        }
    }
    if opus_pts.is_empty() {
        return sdp.to_string();
    }

    let nl = if sdp.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = Vec::new();
    for line in sdp.lines() {
        let trimmed = line.trim_end_matches('\r');
        if let Some(rest) = trimmed.strip_prefix("a=fmtp:") {
            if let Some((pt, _)) = rest.split_once(|c: char| c.is_whitespace()) {
                if opus_pts.contains(pt) {
                    out.push(format!("a=fmtp:{pt} {target}"));
                    continue;
                }
            }
        }
        out.push(trimmed.to_string());
    }
    let mut joined = out.join(nl);
    if sdp.ends_with('\n') {
        joined.push_str(nl);
    }
    joined
}

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
        self.mime_type.to_ascii_lowercase().contains("opus")
    }
}

/// Encode bitrate for a packet of `duration_ms`, capped so the payload stays within
/// [`OPUS_MAX_PAYLOAD_BYTES`]. `duration_ms == 0` returns `base_bps` unchanged.
pub fn opus_bitrate_for_duration(base_bps: i32, duration_ms: u32) -> i32 {
    if duration_ms == 0 {
        return base_bps;
    }
    let cap = (OPUS_MAX_PAYLOAD_BYTES as u64 * 8 * 1000 / duration_ms as u64) as i64;
    let capped = (base_bps as i64).min(cap) as i32;
    capped.max(OPUS_BITRATE_MIN_BPS)
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
    base_bitrate_bps: i32,
    current_bitrate_bps: i32,
}

impl PcmEncoder {
    pub fn new() -> Result<Self, CoreError> {
        Self::new_with_bitrate(opus_target_bitrate_bps())
    }

    fn new_with_bitrate(bitrate_bps: i32) -> Result<Self, CoreError> {
        let application = resolved_opus_application();
        let mut encoder = Encoder::new(SampleRate::Hz48000, Channels::Stereo, application)
            .map_err(|e| CoreError::Track(format!("Opus encoder init: {e}")))?;
        encoder
            .set_bitrate(Bitrate::BitsPerSecond(bitrate_bps))
            .map_err(|e| CoreError::Track(format!("Opus encoder bitrate: {e}")))?;
        encoder
            .set_complexity(10)
            .map_err(|e| CoreError::Track(format!("Opus encoder complexity: {e}")))?;

        Ok(Self {
            opus: Arc::new(Mutex::new(OpusEncoderState {
                encoder,
                pcm_scratch: Vec::new(),
                opus_scratch: vec![0u8; OPUS_MAX_PAYLOAD_BYTES],
                base_bitrate_bps: bitrate_bps,
                current_bitrate_bps: bitrate_bps,
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

        let effective = opus_bitrate_for_duration(
            state.base_bitrate_bps,
            duration.as_millis().min(u32::MAX as u128) as u32,
        );
        if effective != state.current_bitrate_bps {
            state
                .encoder
                .set_bitrate(Bitrate::BitsPerSecond(effective))
                .map_err(|e| CoreError::Track(format!("Opus encoder bitrate: {e}")))?;
            state.current_bitrate_bps = effective;
        }

        state.pcm_scratch.clear();
        state.pcm_scratch.reserve(pcm.len() / 2);
        for chunk in pcm.chunks_exact(2) {
            state
                .pcm_scratch
                .push(i16::from_le_bytes([chunk[0], chunk[1]]));
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
            return Err(CoreError::Track(
                "Opus encoder produced empty payload".into(),
            ));
        }

        Ok((Bytes::copy_from_slice(&state.opus_scratch[..len]), duration))
    }
}

fn samples_per_channel_for_duration(
    duration: Duration,
    pcm_len: usize,
) -> Result<usize, CoreError> {
    let from_duration = ((duration.as_micros() as u64 * 48_000) / 1_000_000) as usize;
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

    fn noise_pcm(seed: &mut u64, duration_ms: usize) -> Vec<u8> {
        let samples = 48 * duration_ms * 2;
        let mut out = Vec::with_capacity(samples * 2);
        for _ in 0..samples {
            *seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let v = ((*seed >> 33) as i32 % 65536 - 32768) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    fn test_format() -> NegotiatedAudioFormat {
        NegotiatedAudioFormat::advertised_opus()
    }

    #[test]
    fn opus_bitrate_for_duration_caps_long_packets() {
        assert_eq!(opus_bitrate_for_duration(400_000, 20), 400_000);
        assert_eq!(opus_bitrate_for_duration(400_000, 40), 200_000);
        assert_eq!(opus_bitrate_for_duration(400_000, 60), 133_333);
        assert_eq!(opus_bitrate_for_duration(510_000, 20), 400_000);
        assert_eq!(opus_bitrate_for_duration(64_000, 60), 64_000);
        assert_eq!(opus_bitrate_for_duration(400_000, 0), 400_000);
    }

    fn assert_packets_fit(base: i32) {
        let enc = PcmEncoder::new_with_bitrate(base).unwrap();
        let fmt = test_format();
        let mut seed = 12345u64;
        for ms in [20usize, 40, 60] {
            for _ in 0..50 {
                let pcm = noise_pcm(&mut seed, ms);
                let (payload, _) = enc
                    .encode(&fmt, &pcm, Duration::from_millis(ms as u64))
                    .unwrap();
                assert!(
                    payload.len() <= OPUS_MAX_PAYLOAD_BYTES,
                    "{ms} ms packet {} B > budget at base {base}",
                    payload.len()
                );
            }
        }
    }

    #[test]
    fn opus_packets_fit_mtu_budget_at_every_duration() {
        assert_packets_fit(OPUS_TARGET_BITRATE_BPS);
    }

    #[test]
    fn opus_packets_fit_mtu_budget_with_max_base_bitrate() {
        assert_packets_fit(OPUS_BITRATE_MAX_BPS);
    }

    #[test]
    fn opus_bitrate_restored_after_long_frame() {
        let enc = PcmEncoder::new_with_bitrate(400_000).unwrap();
        let fmt = test_format();
        let mut seed = 7u64;
        enc.encode(&fmt, &noise_pcm(&mut seed, 60), Duration::from_millis(60))
            .unwrap();
        assert_eq!(enc.opus.lock().unwrap().current_bitrate_bps, 133_333);
        enc.encode(&fmt, &noise_pcm(&mut seed, 20), Duration::from_millis(20))
            .unwrap();
        let st = enc.opus.lock().unwrap();
        assert_eq!(st.current_bitrate_bps, st.base_bitrate_bps);
    }
    use audiopus::coder::Decoder;
    use audiopus::packet::Packet;
    use audiopus::{MutSignals, SampleRate};

    #[test]
    fn opus_sdp_fmtp_omits_maxaveragebitrate_when_env_unset() {
        assert!(
            std::env::var("WEBRTC_OPUS_BITRATE_BPS")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .is_none(),
            "WEBRTC_OPUS_BITRATE_BPS is set; OnceLock captures it for the process. Unset it, or run via scripts/ci/run-pr-integration.sh."
        );
        let line = opus_sdp_fmtp_line();
        assert_eq!(line, "minptime=10;useinbandfec=1;stereo=1");
        assert!(!line.contains("maxaveragebitrate"));
        assert!(line.contains("useinbandfec=1"));
        assert!(line.contains("stereo=1"));
        assert_eq!(opus_bitrate_bps_from_env(), None);
        assert_eq!(opus_target_bitrate_bps(), OPUS_TARGET_BITRATE_BPS);
        assert_eq!(OPUS_TARGET_BITRATE_BPS, 400_000);
    }

    #[test]
    fn enrich_opus_sdp_fmtp_upgrades_weak_answer_fmtp() {
        let sdp = "\
a=rtpmap:111 opus/48000/2\r\n\
a=fmtp:111 minptime=10;useinbandfec=1\r\n\
a=rtpmap:9 G722/8000\r\n\
";
        let enriched = enrich_opus_sdp_fmtp(sdp);
        assert!(enriched.contains(&format!("a=fmtp:111 {}", opus_sdp_fmtp_line())));
        assert!(!enriched.contains("maxaveragebitrate"));
        assert!(enriched.contains("stereo=1"));
        assert!(enriched.contains("a=rtpmap:9 G722/8000"));
    }

    #[test]
    fn parse_opus_bitrate_bps_from_env_unset_blank_clamps_and_rejects_invalid() {
        assert_eq!(parse_opus_bitrate_bps_from_env(None), None);
        assert_eq!(parse_opus_bitrate_bps_from_env(Some("")), None);
        assert_eq!(
            parse_opus_bitrate_bps_from_env(Some(" 192000 ")),
            Some(192_000)
        );
        assert_eq!(parse_opus_bitrate_bps_from_env(Some("64000")), Some(64_000));
        assert_eq!(
            parse_opus_bitrate_bps_from_env(Some("1000")),
            Some(OPUS_BITRATE_MIN_BPS)
        );
        assert_eq!(
            parse_opus_bitrate_bps_from_env(Some("999999")),
            Some(OPUS_BITRATE_MAX_BPS)
        );
        assert_eq!(parse_opus_bitrate_bps_from_env(Some("not-a-number")), None);
    }

    #[test]
    fn parse_opus_application_case_insensitive() {
        assert_eq!(parse_opus_application(None), Application::Audio);
        assert_eq!(parse_opus_application(Some("")), Application::Audio);
        assert_eq!(parse_opus_application(Some("audio")), Application::Audio);
        assert_eq!(parse_opus_application(Some("AUDIO")), Application::Audio);
        assert_eq!(parse_opus_application(Some("VoIP")), Application::Voip);
        assert_eq!(
            parse_opus_application(Some("lowdelay")),
            Application::LowDelay
        );
        assert_eq!(
            parse_opus_application(Some("LowDelay")),
            Application::LowDelay
        );
        assert_eq!(parse_opus_application(Some("music")), Application::Audio);
    }

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
                sdp_fmtp_line: opus_sdp_fmtp_line(),
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
            let sample =
                ((idx as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 10_000.0) as i16;
            chunk.copy_from_slice(&sample.to_le_bytes());
        }
        let (tone_opus, _) = encoder
            .encode(&format, &tone, Duration::from_millis(20))
            .unwrap();

        assert_ne!(silent_opus, tone_opus);
    }
}
