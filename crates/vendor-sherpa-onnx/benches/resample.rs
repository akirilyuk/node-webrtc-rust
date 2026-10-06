//! Layer A bench: resampling 10 s of 22,050 Hz mono f32 to stereo 48 kHz s16le.
//!
//! Run: `cargo bench -p node-webrtc-rust-vendor-sherpa-onnx --bench resample`
//!
//! Criterion benches:
//! - `resample_22k_10s_ns`: one-shot `f32_mono_to_stereo_48k_s16le`
//! - `resample_stream_22k_10s_ns`: `StreamingStereo48kResampler` fed in 0.5 s pushes + `finish`
//!
//! One `perf_probe` line reports the peak live heap bytes above the starting level
//! (`resample_peak_bytes` one-shot, `resample_stream_peak_bytes` streaming), measured with a
//! counting global allocator.
//!
//! `audio.rs` is private to the crate, so it is included by path (no product-code change).

#[path = "../src/audio.rs"]
#[allow(dead_code, unused_imports)]
mod audio;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use criterion::{criterion_group, criterion_main, Criterion};

struct PeakAlloc;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn add_live(n: usize) {
    let now = LIVE.fetch_add(n, Ordering::Relaxed) + n;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for PeakAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        add_live(layout.size());
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        add_live(layout.size());
        System.alloc_zeroed(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= layout.size() {
            add_live(new_size - layout.size());
        } else {
            LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: PeakAlloc = PeakAlloc;

const SRC_RATE: u32 = 22_050;
const PUSH_SAMPLES: usize = 11_025; // 0.5 s

fn sine_10s() -> Vec<f32> {
    (0..SRC_RATE as usize * 10)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SRC_RATE as f32).sin() * 0.5)
        .collect()
}

fn run_oneshot(samples: &[f32]) -> usize {
    audio::f32_mono_to_stereo_48k_s16le(samples, SRC_RATE)
        .0
        .len()
}

fn run_stream(samples: &[f32]) -> usize {
    let mut resampler = audio::StreamingStereo48kResampler::new(SRC_RATE);
    let mut out = 0;
    for chunk in samples.chunks(PUSH_SAMPLES) {
        out += resampler.push_f32(chunk).len();
    }
    out + resampler.finish().len()
}

/// Peak live bytes above the level at entry while `f` runs (its return value is dropped inside).
fn peak_bytes(f: impl FnOnce() -> usize) -> f64 {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let out_len = f();
    std::hint::black_box(out_len);
    PEAK.load(Ordering::Relaxed).saturating_sub(base) as f64
}

fn bench_resample(c: &mut Criterion) {
    let samples = sine_10s();

    c.bench_function("resample_22k_10s_ns", |b| {
        b.iter(|| std::hint::black_box(run_oneshot(&samples)))
    });
    c.bench_function("resample_stream_22k_10s_ns", |b| {
        b.iter(|| std::hint::black_box(run_stream(&samples)))
    });

    let oneshot = peak_bytes(|| run_oneshot(&samples));
    let stream = peak_bytes(|| run_stream(&samples));
    let sha = std::env::var("PERF_SHA").unwrap_or_else(|_| "unknown".into());
    println!(
        "perf_probe {{\"name\":\"resample\",\"sha\":\"{sha}\",\"params\":{{}},\"metrics\":{{\"resample_peak_bytes\":{oneshot},\"resample_stream_peak_bytes\":{stream}}}}}"
    );
}

criterion_group!(benches, bench_resample);
criterion_main!(benches);
