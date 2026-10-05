//! Perf probe: Opus encode cost per 20 ms stereo frame across application / complexity / bitrate.
//!
//! Prints one `opus_probe …` line per combination. Run:
//! `cargo test --release -p node-webrtc-rust-core --test opus_encode_cost_probe -- --ignored --nocapture`

use std::time::Instant;

use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate};

const FRAMES_TOTAL: usize = 1550;
const WARMUP: usize = 50;
const TIMED: usize = 1500;
const SAMPLES_PER_CHANNEL: usize = 960;

/// Mono speech-like signal duplicated to L = R, one 20 ms frame per entry.
fn build_frames() -> Vec<Vec<i16>> {
    let mut frames = Vec::with_capacity(FRAMES_TOTAL);
    for f in 0..FRAMES_TOTAL {
        let mut frame = Vec::with_capacity(SAMPLES_PER_CHANNEL * 2);
        for i in 0..SAMPLES_PER_CHANNEL {
            let t = (f * SAMPLES_PER_CHANNEL + i) as f64 / 48_000.0;
            let tau = 2.0 * std::f64::consts::PI;
            let tone = 6000.0 * (tau * 200.0 * t).sin()
                + 3000.0 * (tau * 700.0 * t).sin()
                + 1000.0 * (tau * 2200.0 * t).sin();
            let envelope = 0.5 + 0.5 * (tau * 4.0 * t).sin();
            let sample = (tone * envelope) as i16;
            frame.push(sample);
            frame.push(sample);
        }
        frames.push(frame);
    }
    frames
}

#[test]
#[ignore = "perf probe"]
fn opus_encode_cost_table() {
    let frames = build_frames();
    let mut out = vec![0_u8; 4000];

    for (app_name, app) in [("audio", Application::Audio), ("voip", Application::Voip)] {
        for complexity in [10_u8, 5, 1] {
            for bitrate in [400_000_i32, 128_000, 48_000] {
                let mut encoder =
                    Encoder::new(SampleRate::Hz48000, Channels::Stereo, app).expect("encoder new");
                encoder
                    .set_bitrate(Bitrate::BitsPerSecond(bitrate))
                    .expect("set_bitrate");
                encoder.set_complexity(complexity).expect("set_complexity");

                for frame in &frames[..WARMUP] {
                    encoder.encode(frame, &mut out).expect("warm-up encode");
                }

                let mut payload_total = 0_usize;
                let started = Instant::now();
                for frame in &frames[WARMUP..WARMUP + TIMED] {
                    let len = encoder.encode(frame, &mut out).expect("timed encode");
                    payload_total += len;
                }
                let elapsed = started.elapsed();

                let us_per_frame = elapsed.as_secs_f64() * 1_000_000.0 / TIMED as f64;
                let core_pct = us_per_frame / 20_000.0 * 100.0;
                let avg_payload = payload_total as f64 / TIMED as f64;
                println!(
                    "opus_probe app={app_name} complexity={complexity} bitrate={bitrate} us_per_frame={us_per_frame:.1} core_pct_per_session={core_pct:.3} avg_payload_bytes={avg_payload:.1}"
                );
            }
        }
    }
}
