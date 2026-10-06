//! Layer A bench: `PcmEncoder::encode` on one 20 ms stereo speech-like frame at today's default
//! settings (Opus Audio mode, 400 kbit/s, complexity 10, or whatever `WEBRTC_OPUS_*` selects).
//!
//! Run: `cargo bench -p node-webrtc-rust-core --bench opus_encode`
//!
//! Criterion bench `opus_encode_20ms_ns`. The full application / complexity / bitrate table is
//! `crates/core/tests/opus_encode_cost_probe.rs`; this bench only tracks the default.

use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use node_webrtc_rust_core::pcm_encoder::{NegotiatedAudioFormat, PcmEncoder};

const FRAMES: usize = 64;
const SAMPLES_PER_CHANNEL: usize = 960;

/// Mono speech-like signal duplicated to L = R; `FRAMES` distinct 20 ms frames as s16le bytes.
fn build_frames() -> Vec<Vec<u8>> {
    (0..FRAMES)
        .map(|f| {
            let mut frame = Vec::with_capacity(SAMPLES_PER_CHANNEL * 4);
            for i in 0..SAMPLES_PER_CHANNEL {
                let t = (f * SAMPLES_PER_CHANNEL + i) as f64 / 48_000.0;
                let tau = 2.0 * std::f64::consts::PI;
                let tone = 6000.0 * (tau * 200.0 * t).sin()
                    + 3000.0 * (tau * 700.0 * t).sin()
                    + 1000.0 * (tau * 2200.0 * t).sin();
                let envelope = 0.5 + 0.5 * (tau * 4.0 * t).sin();
                let sample = (tone * envelope) as i16;
                frame.extend_from_slice(&sample.to_le_bytes());
                frame.extend_from_slice(&sample.to_le_bytes());
            }
            frame
        })
        .collect()
}

fn bench_opus(c: &mut Criterion) {
    let encoder = PcmEncoder::new().expect("encoder");
    let format = NegotiatedAudioFormat::advertised_opus();
    let frames = build_frames();
    let duration = Duration::from_millis(20);

    // Warm the encoder state.
    for frame in &frames {
        encoder.encode(&format, frame, duration).expect("warm-up");
    }

    let mut next = 0usize;
    c.bench_function("opus_encode_20ms_ns", |b| {
        b.iter(|| {
            let frame = &frames[next % FRAMES];
            next += 1;
            std::hint::black_box(encoder.encode(&format, frame, duration).expect("encode"))
        })
    });
}

criterion_group!(benches, bench_opus);
criterion_main!(benches);
