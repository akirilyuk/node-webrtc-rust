//! Layer A bench: outbound TTS drain cost per 20 ms frame.
//!
//! Run: `cargo bench -p node-webrtc-rust-speech --bench tts_drain`
//!
//! A test TTS provider returns one 10 s chunk (1,920,000 bytes, 500 frames). The agent's drain
//! worker slices and paces it into a writer that only counts frames and bytes. The runtime is
//! `current_thread` with paused tokio time, so pacing sleeps auto-advance and the wall time is
//! pure CPU. Plain `harness = false` main (no criterion): one `perf_probe` line, median of 7
//! runs after one warm-up.
//!
//! `tts_drain_bytes_copied_per_frame` is the number of bytes requested from the allocator on the
//! measuring thread during the send, divided by frames written. It is a proxy for bytes copied:
//! every copy of PCM goes through an allocation of that size.

#[path = "support/probe.rs"]
mod probe;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttConfig, SttVendor, TtsConfig, TtsVendor, VoiceAgentConfig,
};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsAudioChunk, TtsProvider, VendorFactory};
use node_webrtc_rust_speech::{PcmReader, PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

/// 10 s of stereo s16le 48 kHz.
const CHUNK_BYTES: usize = 1_920_000;
const RUNS: usize = 7;

struct TenSecondTts {
    pcm: Bytes,
}

#[async_trait]
impl TtsProvider for TenSecondTts {
    fn vendor_name(&self) -> &'static str {
        "bench"
    }

    async fn synthesize(&self, _text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        Ok(vec![TtsAudioChunk {
            pcm: self.pcm.clone(),
            duration_ms: 10_000,
        }])
    }
}

struct BenchFactory {
    pcm: Bytes,
}

impl VendorFactory for BenchFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(TenSecondTts {
            pcm: self.pcm.clone(),
        }))
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();

    let pcm = Bytes::from(vec![0u8; CHUNK_BYTES]);
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(BenchFactory { pcm }));

    let frames_written = Arc::new(AtomicU64::new(0));
    let bytes_written = Arc::new(AtomicU64::new(0));

    let (ns_per_frame, copied_per_frame) = rt.block_on(async {
        let agent = VoiceAgent::new(VoiceAgentConfig::default(), Arc::new(registry)).unwrap();
        let frames = Arc::clone(&frames_written);
        let bytes = Arc::clone(&bytes_written);
        let writer: PcmWriter = Arc::new(move |pcm, _ms| {
            frames.fetch_add(1, Ordering::Relaxed);
            bytes.fetch_add(pcm.len() as u64, Ordering::Relaxed);
            Ok(())
        });
        let reader: PcmReader = Arc::new(|| Ok(None));
        agent.attach(reader, writer).await.unwrap();
        agent.start(None).await.unwrap();

        // Warm-up: first send spins up the drain worker.
        agent.send_text_to_tts("bench").await.unwrap();

        let mut ns = Vec::with_capacity(RUNS);
        let mut copied = Vec::with_capacity(RUNS);
        for _ in 0..RUNS {
            let frames_before = frames_written.load(Ordering::Relaxed);
            let alloc_before = probe::alloc_bytes_now();
            let started = Instant::now();
            agent.send_text_to_tts("bench").await.unwrap();
            let elapsed = started.elapsed();
            let alloc_delta = probe::alloc_bytes_now() - alloc_before;
            let frames = (frames_written.load(Ordering::Relaxed) - frames_before).max(1);
            ns.push(elapsed.as_nanos() as f64 / frames as f64);
            copied.push(alloc_delta as f64 / frames as f64);
        }
        agent.stop().await.unwrap();
        (median(ns), median(copied))
    });

    assert!(
        bytes_written.load(Ordering::Relaxed) >= (CHUNK_BYTES * RUNS) as u64,
        "writer saw fewer bytes than were synthesized"
    );

    probe::print_probe(
        "tts_drain",
        &[
            ("tts_drain_frames_ns_per_frame", ns_per_frame),
            ("tts_drain_bytes_copied_per_frame", copied_per_frame),
        ],
    );
}
