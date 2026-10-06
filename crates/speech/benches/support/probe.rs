//! Shared helpers for the speech perf benches and the allocation guard test.
//!
//! Included with `#[path]` (not a crate module), so every binary that includes it gets its own
//! `#[global_allocator]`. The counters are thread-local: only allocations made on the measuring
//! thread are counted, which keeps the numbers independent of background threads (libtest, the
//! criterion harness). Drive the agent on a `current_thread` runtime so all agent work runs on
//! that thread.
#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttVendor, TtsVendor, VadConfig, VoiceAgentConfig};
use node_webrtc_rust_speech::{PcmReader, PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static ALLOC_BYTES: Cell<u64> = const { Cell::new(0) };
}

pub struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        ALLOC_BYTES.with(|c| c.set(c.get() + layout.size() as u64));
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        ALLOC_BYTES.with(|c| c.set(c.get() + layout.size() as u64));
        System.alloc_zeroed(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        ALLOC_BYTES.with(|c| c.set(c.get() + new_size as u64));
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Allocations (alloc + alloc_zeroed + realloc) made on the calling thread so far.
pub fn allocs_now() -> u64 {
    ALLOCS.with(|c| c.get())
}

/// Bytes requested from the allocator on the calling thread so far.
pub fn alloc_bytes_now() -> u64 {
    ALLOC_BYTES.with(|c| c.get())
}

/// Print one `perf_probe {json}` line (collected by `scripts/perf/run-perf-baseline.sh`).
pub fn print_probe(name: &str, metrics: &[(&str, f64)]) {
    let sha = std::env::var("PERF_SHA").unwrap_or_else(|_| "unknown".into());
    let body = metrics
        .iter()
        .map(|(k, v)| format!("\"{k}\":{v}"))
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "perf_probe {{\"name\":\"{name}\",\"sha\":\"{sha}\",\"params\":{{}},\"metrics\":{{{body}}}}}"
    );
}

pub fn mock_registry() -> Arc<VendorRegistry> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
    Arc::new(registry)
}

/// Energy VAD on, STT gated on VAD (the production inbound shape), mock vendors.
pub fn inbound_config() -> VoiceAgentConfig {
    VoiceAgentConfig {
        vad: VadConfig {
            enabled: true,
            gate_stt: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A started agent with a no-op PCM writer attached.
pub async fn started_agent(config: VoiceAgentConfig) -> Arc<VoiceAgent> {
    let agent = VoiceAgent::new(config, mock_registry()).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let reader: PcmReader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent
}

/// 20 ms stereo s16le 48 kHz silence (3840 bytes).
pub fn silence_frame() -> Bytes {
    Bytes::from(vec![0u8; 3840])
}

/// 20 ms stereo s16le 48 kHz, 440 Hz sine, amplitude 10000, L = R (3840 bytes).
pub fn tone_frame() -> Bytes {
    let mut pcm = Vec::with_capacity(3840);
    for i in 0..960 {
        let t = i as f64 / 48_000.0;
        let sample = (10_000.0 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    Bytes::from(pcm)
}

pub const ALLOC_WARMUP_FRAMES: usize = 50;
pub const ALLOC_MEASURED_FRAMES: usize = 500;

/// Allocations per `process_inbound_pcm` call over 500 voiced frames after 50 warm-up frames.
/// Runs on its own `current_thread` runtime so the thread-local counter sees all agent work.
pub fn measure_inbound_allocs_per_frame() -> f64 {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let agent = started_agent(inbound_config()).await;
        let frame = tone_frame();
        for _ in 0..ALLOC_WARMUP_FRAMES {
            agent.process_inbound_pcm(frame.clone(), 20).await.unwrap();
        }
        let before = allocs_now();
        for _ in 0..ALLOC_MEASURED_FRAMES {
            agent.process_inbound_pcm(frame.clone(), 20).await.unwrap();
        }
        let after = allocs_now();
        agent.stop().await.unwrap();
        (after - before) as f64 / ALLOC_MEASURED_FRAMES as f64
    })
}
