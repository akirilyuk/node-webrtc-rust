//! Layer A bench: cost of one `VoiceAgent::process_inbound_pcm` (mock STT, energy VAD, gate open).
//!
//! Run: `cargo bench -p node-webrtc-rust-speech --bench inbound_frame`
//!
//! Emits criterion benches `inbound_frame_silence_ns` and `inbound_frame_voiced_ns`, and one
//! `perf_probe` line with `inbound_frame_allocs` (allocations per voiced frame, thread-local
//! counting allocator, current-thread runtime; deterministic).

#[path = "support/probe.rs"]
mod probe;

use criterion::{criterion_group, criterion_main, Criterion};

fn bench_inbound(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let agent = rt.block_on(probe::started_agent(probe::inbound_config()));

    let silence = probe::silence_frame();
    c.bench_function("inbound_frame_silence_ns", |b| {
        b.iter(|| {
            rt.block_on(agent.process_inbound_pcm(silence.clone(), 20))
                .unwrap()
        })
    });

    // Tone keeps VAD speaking, so the STT gate stays open for the whole measurement.
    let tone = probe::tone_frame();
    for _ in 0..probe::ALLOC_WARMUP_FRAMES {
        rt.block_on(agent.process_inbound_pcm(tone.clone(), 20))
            .unwrap();
    }
    c.bench_function("inbound_frame_voiced_ns", |b| {
        b.iter(|| {
            rt.block_on(agent.process_inbound_pcm(tone.clone(), 20))
                .unwrap()
        })
    });
    rt.block_on(agent.stop()).unwrap();

    let allocs = probe::measure_inbound_allocs_per_frame();
    probe::print_probe("inbound_frame_allocs", &[("inbound_frame_allocs", allocs)]);
}

criterion_group!(benches, bench_inbound);
criterion_main!(benches);
