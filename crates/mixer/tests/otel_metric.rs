//! `voice_mix_frames_total{outcome}` equals the sum of the per-source `SourceStats` counters.
//!
//! Own test binary: the instrument is cached against the global meter provider on first use,
//! so the provider must be installed before any queue activity in this process.
#![cfg(feature = "otel")]

use std::collections::HashMap;
use std::time::Instant;

use bytes::Bytes;
use node_webrtc_rust_mixer::{ElasticFrameQueue, Frame, FRAME_BYTES};
use opentelemetry::global;
use opentelemetry_sdk::metrics::data::Sum;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

fn tone(id: u32) -> Frame {
    let value = (1_000 + id * 10) as i16;
    let mut pcm = vec![0u8; FRAME_BYTES];
    for i in 0..FRAME_BYTES / 2 {
        pcm[i * 2..i * 2 + 2].copy_from_slice(&value.to_le_bytes());
    }
    Frame::new(Bytes::from(pcm), None)
}

#[test]
fn mix_frames_counter_matches_source_stats_by_outcome() {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    global::set_meter_provider(provider.clone());

    let t0 = Instant::now();

    // Source A: bunched (30 frames at once -> time-compression), then drained into a gap
    // (-> one concealment frame).
    let mut a = ElasticFrameQueue::new();
    for id in 0..30 {
        a.push(tone(id), t0);
    }
    for _ in 0..80 {
        a.pop();
    }

    // Source B: producer faster than real time (60 frames into a 50-frame queue ->
    // overflow merges), plus two undecodable packets.
    let mut b = ElasticFrameQueue::new();
    for id in 0..60 {
        b.push(tone(id), t0);
    }
    b.note_decode_error("test", t0);
    b.note_decode_error("test", t0);

    let (sa, sb) = (a.stats(), b.stats());
    let expected: HashMap<&str, u64> = HashMap::from([
        ("played", sa.played + sb.played),
        ("compressed", sa.compressed + sb.compressed),
        ("concealed", sa.concealed + sb.concealed),
        (
            "overflow_compressed",
            sa.overflow_compressed + sb.overflow_compressed,
        ),
        ("decode_error", sa.decode_errors + sb.decode_errors),
    ]);
    assert!(expected["compressed"] > 0, "scenario must compress");
    assert!(expected["concealed"] > 0, "scenario must conceal");
    assert!(
        expected["overflow_compressed"] > 0,
        "scenario must overflow"
    );
    assert_eq!(expected["decode_error"], 2);

    provider.force_flush().expect("flush");
    let mut got: HashMap<String, u64> = HashMap::new();
    let mut metric_name = String::new();
    for rm in exporter.get_finished_metrics().expect("metrics") {
        for scope in &rm.scope_metrics {
            for metric in &scope.metrics {
                if metric.name != "voice_mix_frames" {
                    continue;
                }
                metric_name = metric.name.to_string();
                let sum = metric
                    .data
                    .as_any()
                    .downcast_ref::<Sum<u64>>()
                    .expect("u64 sum");
                for dp in &sum.data_points {
                    assert_eq!(dp.attributes.len(), 1, "only the outcome label");
                    let kv = dp.attributes.first().expect("attribute");
                    assert_eq!(kv.key.as_str(), "outcome");
                    got.insert(kv.value.to_string(), dp.value);
                }
            }
        }
    }
    assert_eq!(metric_name, "voice_mix_frames");
    for (outcome, want) in expected {
        assert_eq!(got.get(outcome).copied(), Some(want), "outcome {outcome}");
    }
    assert_eq!(got.len(), 5);
}
