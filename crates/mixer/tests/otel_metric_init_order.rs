//! Why the bindings must start OTel before the first mix frame: an instrument built from
//! `global::meter` before `global::set_meter_provider` stays a no-op forever.
//!
//! Own test binary: the instrument is cached in a process-global `OnceLock`.
#![cfg(feature = "otel")]

use std::time::Instant;

use bytes::Bytes;
use node_webrtc_rust_mixer::{ElasticFrameQueue, Frame, FRAME_BYTES};
use opentelemetry::global;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

fn tone() -> Frame {
    let mut pcm = vec![0u8; FRAME_BYTES];
    for i in 0..FRAME_BYTES / 2 {
        pcm[i * 2..i * 2 + 2].copy_from_slice(&1_000i16.to_le_bytes());
    }
    Frame::new(Bytes::from(pcm), None)
}

fn mix_some(q: &mut ElasticFrameQueue, t0: Instant) {
    for _ in 0..5 {
        q.push(tone(), t0);
    }
    for _ in 0..8 {
        q.pop();
    }
}

#[test]
fn counter_built_before_provider_records_nothing() {
    let t0 = Instant::now();
    let mut q = ElasticFrameQueue::new();

    // No provider yet: this builds and caches the counter against the no-op provider.
    mix_some(&mut q, t0);
    let before = q.stats().played;
    assert!(before > 0, "frames were mixed before the provider");

    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    global::set_meter_provider(provider.clone());

    mix_some(&mut q, t0);
    assert!(
        q.stats().played > before,
        "frames were mixed after the provider"
    );

    provider.force_flush().expect("flush");
    let exported = exporter
        .get_finished_metrics()
        .expect("metrics")
        .iter()
        .flat_map(|rm| rm.scope_metrics.iter())
        .flat_map(|sm| sm.metrics.iter())
        .any(|m| m.name == "voice_mix_frames");
    assert!(
        !exported,
        "a counter cached before the provider never exports"
    );
}
