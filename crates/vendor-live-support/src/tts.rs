//! TTS live measurement and the streaming contract every cloud TTS vendor must meet.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::audio::rms;

/// Upper bound for one synthesis call (`infra.unbounded-external-wait`).
pub const SYNTHESIS_TIMEOUT: Duration = Duration::from_secs(60);

/// What one progressive synthesis looked like from the sink's side.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TtsSample {
    /// Call start to the first chunk on the sink.
    pub ttfa_ms: u64,
    /// Call start to the call returning.
    pub total_ms: u64,
    /// Sum of the chunks' own `duration_ms`.
    pub audio_ms: u64,
    pub chunks: usize,
    /// Worst time a chunk arrived after the audio before it would have finished playing; above
    /// zero means an audible gap.
    pub stall_ms: u64,
    /// Bytes that reached the sink.
    pub bytes: usize,
    /// Bytes in the chunks the call returned.
    pub returned_bytes: usize,
    /// The first sink chunk arrived before the call returned (it streamed).
    pub first_before_return: bool,
    /// RMS of the sink audio (s16le).
    pub rms: f64,
}

/// `stall_ms` from `(arrival offset ms, chunk duration ms)` pairs in arrival order:
/// max over k of `max(0, (arrival_k - arrival_0) - sum(duration of chunks before k))`.
pub fn stall_ms(arrivals: &[(u64, u32)]) -> u64 {
    let Some(&(first, _)) = arrivals.first() else {
        return 0;
    };
    let mut played = 0u64;
    let mut worst = 0u64;
    for &(at, duration) in arrivals {
        worst = worst.max((at - first).saturating_sub(played));
        played += u64::from(duration);
    }
    worst
}

/// Runs one `synthesize_progressive` with a sink and timestamps every chunk.
pub async fn measure_tts(provider: &dyn TtsProvider, text: &str) -> SpeechResult<TtsSample> {
    let (tx, mut rx) = mpsc::unbounded_channel::<TtsAudioChunk>();
    let sink = TtsProgressiveSink {
        tx,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let started = Instant::now();
    let receiver = tokio::spawn(async move {
        let mut arrivals: Vec<(u64, u32)> = Vec::new();
        let mut pcm: Vec<u8> = Vec::new();
        while let Some(chunk) = rx.recv().await {
            arrivals.push((started.elapsed().as_millis() as u64, chunk.duration_ms));
            pcm.extend_from_slice(&chunk.pcm);
        }
        (arrivals, pcm)
    });

    let result = tokio::time::timeout(
        SYNTHESIS_TIMEOUT,
        provider.synthesize_progressive(text, Some(sink)),
    )
    .await;
    let returned_ms = started.elapsed().as_millis() as u64;
    let returned = match result {
        Ok(result) => result?,
        Err(_) => {
            return Err(SpeechError::Tts(format!(
                "{} synthesis did not finish within {} s",
                provider.vendor_name(),
                SYNTHESIS_TIMEOUT.as_secs()
            )))
        }
    };
    // The sink moved into the call and is dropped by now, so the receiver finishes.
    let (arrivals, pcm) = receiver
        .await
        .map_err(|err| SpeechError::Internal(format!("sink task: {err}")))?;

    Ok(TtsSample {
        ttfa_ms: arrivals.first().map_or(returned_ms, |(at, _)| *at),
        total_ms: returned_ms,
        audio_ms: arrivals.iter().map(|(_, d)| u64::from(*d)).sum(),
        chunks: arrivals.len(),
        stall_ms: stall_ms(&arrivals),
        bytes: pcm.len(),
        returned_bytes: returned.iter().map(|c| c.pcm.len()).sum(),
        first_before_return: arrivals.first().is_some_and(|(at, _)| *at < returned_ms),
        rms: rms(&pcm),
    })
}

/// True when `bytes` of 48 kHz stereo s16le (192 bytes per ms) last `audio_ms` within
/// `max(20 ms, 2 %)`.
pub fn duration_matches_bytes(bytes: usize, audio_ms: u64) -> bool {
    let from_bytes = bytes as f64 / 192.0;
    let tolerance = (audio_ms as f64 * 0.02).max(20.0);
    (from_bytes - audio_ms as f64).abs() <= tolerance
}

/// Catalog `audio.sample-rate-mismatch` guard for mock-server tests: the chunks a provider
/// returned (or sent to the sink) for `source_ms` of vendor audio must declare that duration, and
/// their bytes (48 kHz stereo s16le, 192 bytes per ms) must last the same, both within
/// `max(20 ms, 2 %)`. Audio forwarded at the vendor's own rate fails the second check.
pub fn assert_chunks_match_source(label: &str, chunks: &[TtsAudioChunk], source_ms: u64) {
    let declared: u64 = chunks.iter().map(|c| u64::from(c.duration_ms)).sum();
    let bytes: usize = chunks.iter().map(|c| c.pcm.len()).sum();
    let tolerance = (source_ms as f64 * 0.02).max(20.0);
    assert!(
        (declared as f64 - source_ms as f64).abs() <= tolerance,
        "{label}: chunks declare {declared} ms for {source_ms} ms of source audio"
    );
    assert!(
        duration_matches_bytes(bytes, declared),
        "{label}: {bytes} bytes are {:.0} ms of 48 kHz stereo but the chunks declare {declared} ms (sample rate mismatch)",
        bytes as f64 / 192.0
    );
}

/// The streaming contract (`contract.duplicate-emit`): streamed in at least two chunks, the first
/// before the call returned, sink bytes equal returned bytes, sane duration and level.
/// Prints `<label>_ttfa_ms <n>`.
pub fn assert_tts_contract(label: &str, sample: &TtsSample) {
    println!("{label}_ttfa_ms {}", sample.ttfa_ms);
    assert!(
        sample.chunks >= 2,
        "{label}: expected >= 2 sink chunks, got {}",
        sample.chunks
    );
    assert!(
        sample.first_before_return,
        "{label}: the first sink chunk arrived after the call returned (not streamed)"
    );
    assert_eq!(
        sample.bytes, sample.returned_bytes,
        "{label}: sink bytes differ from returned bytes"
    );
    // Catalog `audio.sample-rate-mismatch`: the bytes are 48 kHz stereo s16le (192 bytes per ms),
    // so they must add up to the duration the chunks declare. Audio forwarded at its own rate
    // (16 kHz mono played as 48 kHz) is a third as long as it claims.
    println!(
        "{label}_bytes_per_ms {:.1}",
        sample.bytes as f64 / sample.audio_ms.max(1) as f64
    );
    assert!(
        duration_matches_bytes(sample.bytes, sample.audio_ms),
        "{label}: {} bytes are {:.0} ms of 48 kHz stereo but the chunks declare {} ms (sample rate mismatch)",
        sample.bytes,
        sample.bytes as f64 / 192.0,
        sample.audio_ms
    );
    assert!(
        (1500..=15000).contains(&sample.audio_ms),
        "{label}: audio duration {} ms outside 1500..=15000",
        sample.audio_ms
    );
    assert!(
        sample.rms > 200.0,
        "{label}: RMS {:.1} <= 200: audio is near-silent",
        sample.rms
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use bytes::Bytes;

    /// Emits three 20 ms chunks on the sink, 15 ms apart, and returns them.
    struct FakeTts {
        send_to_sink: bool,
    }

    #[async_trait]
    impl TtsProvider for FakeTts {
        fn vendor_name(&self) -> &'static str {
            "fake"
        }

        async fn synthesize(&self, _text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
            Ok(Vec::new())
        }

        async fn synthesize_progressive(
            &self,
            _text: &str,
            sink: Option<TtsProgressiveSink>,
        ) -> SpeechResult<Vec<TtsAudioChunk>> {
            let mut out = Vec::new();
            for _ in 0..3 {
                tokio::time::sleep(Duration::from_millis(15)).await;
                let chunk = TtsAudioChunk {
                    pcm: Bytes::from(vec![0x10; 3840]),
                    duration_ms: 20,
                };
                if self.send_to_sink {
                    sink.as_ref().expect("sink").send(chunk.clone());
                }
                out.push(chunk);
            }
            Ok(out)
        }
    }

    #[tokio::test]
    async fn measure_counts_chunks_and_bytes_from_the_sink() {
        let sample = measure_tts(&FakeTts { send_to_sink: true }, "hi")
            .await
            .unwrap();
        assert_eq!(sample.chunks, 3);
        assert_eq!(sample.bytes, 3 * 3840);
        assert_eq!(sample.bytes, sample.returned_bytes);
        assert_eq!(sample.audio_ms, 60);
        assert!(sample.first_before_return);
        assert!(sample.ttfa_ms >= 10 && sample.ttfa_ms <= sample.total_ms);
    }

    #[tokio::test]
    async fn measure_exposes_a_vendor_that_never_streams() {
        let sample = measure_tts(
            &FakeTts {
                send_to_sink: false,
            },
            "hi",
        )
        .await
        .unwrap();
        assert_eq!(sample.chunks, 0);
        assert_eq!(sample.returned_bytes, 3 * 3840);
        assert!(!sample.first_before_return);
    }

    #[test]
    fn stall_is_zero_when_audio_arrives_ahead_of_playback() {
        // 100 ms chunks arriving every 50 ms.
        let arrivals: Vec<(u64, u32)> = (0..5).map(|k| (10 + k * 50, 100)).collect();
        assert_eq!(stall_ms(&arrivals), 0);
    }

    #[test]
    fn stall_is_the_worst_gap_after_playback_would_have_finished() {
        // chunk 0 at 0 (100 ms), chunk 1 at 100 (100 ms), chunk 2 at 450: the audio before it
        // ends at 200, so it is 250 ms late.
        let arrivals = [(0u64, 100u32), (100, 100), (450, 100)];
        assert_eq!(stall_ms(&arrivals), 250);
    }

    #[test]
    fn stall_of_empty_and_single_is_zero() {
        assert_eq!(stall_ms(&[]), 0);
        assert_eq!(stall_ms(&[(500, 100)]), 0);
    }

    fn sample() -> TtsSample {
        TtsSample {
            ttfa_ms: 120,
            total_ms: 900,
            audio_ms: 3000,
            chunks: 6,
            stall_ms: 0,
            bytes: 576_000,
            returned_bytes: 576_000,
            first_before_return: true,
            rms: 1500.0,
        }
    }

    #[test]
    fn contract_accepts_a_good_sample() {
        assert_tts_contract("good", &sample());
    }

    #[test]
    #[should_panic(expected = "sink bytes differ from returned bytes")]
    fn contract_rejects_duplicate_emit() {
        let mut s = sample();
        s.bytes = 2000;
        assert_tts_contract("dup", &s);
    }

    #[test]
    #[should_panic(expected = "sample rate mismatch")]
    fn contract_rejects_audio_at_the_wrong_rate() {
        // 3 s of 16 kHz mono forwarded as if it were 48 kHz stereo: a third of the bytes.
        let mut s = sample();
        s.bytes = 192_000;
        s.returned_bytes = 192_000;
        assert_tts_contract("fast", &s);
    }

    fn chunk(bytes: usize, ms: u32) -> TtsAudioChunk {
        TtsAudioChunk {
            pcm: bytes::Bytes::from(vec![0u8; bytes]),
            duration_ms: ms,
        }
    }

    #[test]
    fn chunks_match_source_accepts_1s_of_48k_stereo() {
        assert_chunks_match_source("ok", &[chunk(96_000, 500), chunk(96_000, 500)], 1000);
    }

    #[test]
    #[should_panic(expected = "sample rate mismatch")]
    fn chunks_match_source_rejects_16k_mono_sent_as_48k() {
        // 1 s of 16 kHz mono is 32000 bytes; doubled to stereo that is 64000, not 192000.
        assert_chunks_match_source("fast", &[chunk(64_000, 1000)], 1000);
    }

    #[test]
    #[should_panic(expected = "declare")]
    fn chunks_match_source_rejects_a_wrong_declared_duration() {
        assert_chunks_match_source("short", &[chunk(64_000, 333)], 1000);
    }

    #[test]
    fn duration_tolerance_is_two_percent_or_20_ms() {
        assert!(duration_matches_bytes(192 * 1019, 1000));
        assert!(!duration_matches_bytes(192 * 1021, 1000));
        assert!(duration_matches_bytes(192 * 120, 100));
        assert!(!duration_matches_bytes(192 * 121, 100));
    }

    #[test]
    #[should_panic(expected = "not streamed")]
    fn contract_rejects_a_non_streaming_vendor() {
        let mut s = sample();
        s.first_before_return = false;
        assert_tts_contract("buffered", &s);
    }

    #[test]
    #[should_panic(expected = ">= 2 sink chunks")]
    fn contract_rejects_a_single_chunk() {
        let mut s = sample();
        s.chunks = 1;
        assert_tts_contract("one", &s);
    }

    #[test]
    #[should_panic(expected = "near-silent")]
    fn contract_rejects_silence() {
        let mut s = sample();
        s.rms = 3.0;
        assert_tts_contract("quiet", &s);
    }
}
