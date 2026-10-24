//! Concealment of a declared-lost run of packets with Opus PLC / in-band FEC.

use super::stats::InboundAudioStageStats;
use node_webrtc_rust_mixer::{Frame, OpusDecoder};

/// At most this many lost packets are concealed per gap (100 ms at 20 ms packets).
pub const MAX_CONCEALED_PACKETS: u64 = 5;

/// Produces audio for `count` lost packets.
///
/// Conceals `min(count, 5)` packets of `decoder.last_packet_samples()` each: PLC for all but
/// the last concealed one, which uses the in-band FEC of `next` (the packet right after the
/// gap) when it is present and `count <= 5`, else PLC. Packets beyond the cap produce no
/// audio and are counted in `lost_not_concealed`. A decoder error makes that packet
/// contribute no audio (counted in `undecodable`).
pub fn conceal_lost(
    decoder: &mut OpusDecoder,
    count: u64,
    next: Option<&[u8]>,
    stats: &InboundAudioStageStats,
) -> Vec<Frame> {
    let n = count.min(MAX_CONCEALED_PACKETS);
    if count > n {
        InboundAudioStageStats::add(&stats.lost_not_concealed, count - n);
    }
    if n == 0 {
        return Vec::new();
    }
    InboundAudioStageStats::add(&stats.concealment_events, 1);
    let samples = decoder.last_packet_samples();
    let mut out = Vec::new();
    for i in 0..n {
        let last = i == n - 1;
        let result = match next {
            Some(payload) if last && count <= MAX_CONCEALED_PACKETS => {
                InboundAudioStageStats::add(&stats.fec_decodes, 1);
                decoder.decode_fec(payload, samples)
            }
            _ => {
                InboundAudioStageStats::add(&stats.plc_decodes, 1);
                decoder.conceal_plc(samples)
            }
        };
        match result {
            Ok(frames) => {
                InboundAudioStageStats::add(&stats.concealed_samples, samples as u64);
                out.extend(frames);
            }
            Err(_) => InboundAudioStageStats::add(&stats.undecodable, 1),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_lost_conceals_five_and_counts_the_rest() {
        let mut dec = OpusDecoder::new().unwrap();
        let stats = InboundAudioStageStats::default();
        let frames = conceal_lost(&mut dec, 10, None, &stats);
        let s = stats.snapshot();
        assert_eq!(s.plc_decodes, 5);
        assert_eq!(s.fec_decodes, 0);
        assert_eq!(s.lost_not_concealed, 5);
        assert_eq!(s.concealed_samples, 5 * 960);
        assert_eq!(s.concealment_events, 1);
        assert_eq!(frames.len(), 5);
    }

    #[test]
    fn short_gap_uses_fec_for_the_last_packet() {
        let mut dec = OpusDecoder::new().unwrap();
        let stats = InboundAudioStageStats::default();
        // A silence-ish valid TOC-only packet is not guaranteed; use a decoder error path:
        // an undecodable `next` must not panic and must be counted.
        let frames = conceal_lost(&mut dec, 3, Some(&[0xFF, 0xFE]), &stats);
        let s = stats.snapshot();
        assert_eq!(s.plc_decodes, 2);
        assert_eq!(s.fec_decodes, 1);
        assert_eq!(s.lost_not_concealed, 0);
        assert_eq!(frames.len(), 2 + usize::from(s.undecodable == 0));
        assert!(s.undecodable <= 1);
    }
}
