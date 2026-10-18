//! Offline check of the VAD gate that the live STT tests rely on. No key, no network, and no
//! `live` feature: it runs in normal CI.
//!
//! The gate uses the voice agent's default energy VAD (threshold 0.15, 250 ms min speech,
//! 1300 ms min silence, 300 ms pre-roll). A speech segment therefore ends 1300 ms after the
//! last voiced frame, so the trailing silence in these clips is longer than that.

use node_webrtc_rust_speech::config::VadConfig;
use node_webrtc_rust_vendor_live_support::vad_gate::{
    silence_stereo_48k, tone_stereo_48k, vad_gated_frames, FRAME_BYTES,
};

/// Default `min_silence_duration_ms`: the VAD hangover after the last voiced frame.
const HANGOVER_MS: u32 = 1300;
const PRE_ROLL_MS: u32 = 300;

fn clip(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

#[test]
fn tone_between_silences_is_gated_to_one_segment() {
    assert_eq!(VadConfig::default().min_silence_duration_ms, HANGOVER_MS);
    // 1 s silence, 1 s tone at -12 dBFS, 2 s silence.
    let stereo = clip(&[
        silence_stereo_48k(1000),
        tone_stereo_48k(1000, -12.0),
        silence_stereo_48k(2000),
    ]);
    let gated = vad_gated_frames(&stereo);

    println!(
        "vad_gate pushed_ms={} total_ms={} starts={} ends={}",
        gated.pushed_ms, gated.total_ms, gated.starts, gated.ends
    );
    assert_eq!(gated.total_ms, 4000);
    assert_eq!(gated.starts, 1, "one SpeechStart");
    assert_eq!(gated.ends, 1, "one SpeechEnd");
    assert!(
        (1000..=1000 + PRE_ROLL_MS + HANGOVER_MS).contains(&gated.pushed_ms),
        "pushed {} ms",
        gated.pushed_ms
    );
    assert!(
        gated.pushed_ms < gated.total_ms - 1000,
        "the silence padding must not be pushed: {} of {} ms",
        gated.pushed_ms,
        gated.total_ms
    );
    // 20 ms frames of 16 kHz mono s16le: 320 samples, 640 bytes.
    assert_eq!(gated.frames_16k.len() as u32 * 20, gated.pushed_ms);
    assert!(gated.frames_16k.iter().all(|frame| frame.len() == 640));
}

#[test]
fn pre_roll_covers_the_start_of_the_tone() {
    let stereo = clip(&[
        silence_stereo_48k(1000),
        tone_stereo_48k(1000, -12.0),
        silence_stereo_48k(2000),
    ]);
    let gated = vad_gated_frames(&stereo);
    // The first emitted frame is up to 300 ms before SpeechStart, which itself fires 250 ms
    // into the tone: so the tone onset is inside the pushed audio.
    let first_voiced = gated
        .frames_16k
        .iter()
        .position(|frame| {
            frame
                .as_chunks::<2>()
                .0
                .iter()
                .any(|s| i16::from_le_bytes(*s) != 0)
        })
        .expect("voiced frame pushed");
    assert!(
        first_voiced as u32 * 20 <= PRE_ROLL_MS,
        "first voiced frame {first_voiced}"
    );
}

#[test]
fn silence_alone_pushes_nothing() {
    let gated = vad_gated_frames(&silence_stereo_48k(3000));
    assert_eq!((gated.starts, gated.ends, gated.pushed_ms), (0, 0, 0));
    assert!(gated.frames_16k.is_empty());
    assert_eq!(gated.total_ms, 3000);
}

#[test]
fn two_segments_are_gated_separately() {
    let stereo = clip(&[
        silence_stereo_48k(1000),
        tone_stereo_48k(1000, -12.0),
        silence_stereo_48k(2000),
        tone_stereo_48k(1000, -12.0),
        silence_stereo_48k(2000),
    ]);
    let gated = vad_gated_frames(&stereo);
    assert_eq!((gated.starts, gated.ends), (2, 2));
    assert!(
        gated.pushed_ms < gated.total_ms - 1000,
        "{} of {}",
        gated.pushed_ms,
        gated.total_ms
    );
    // Two tones plus pre-roll and hangover each.
    assert!(gated.pushed_ms >= 2000);
}

#[test]
fn a_partial_trailing_frame_is_still_counted() {
    let mut stereo = tone_stereo_48k(1000, -12.0);
    stereo.extend_from_slice(&vec![0; FRAME_BYTES / 2]);
    let gated = vad_gated_frames(&stereo);
    assert_eq!(gated.total_ms, 1010);
    assert_eq!(gated.starts, 1);
}

/// Why the clips above carry 2 s of trailing silence: with the default 1300 ms hangover, 1 s of
/// silence after the tone never ends the segment, so the tail would be pushed to the vendor.
#[test]
fn one_second_of_trailing_silence_is_shorter_than_the_hangover() {
    let stereo = clip(&[
        silence_stereo_48k(1000),
        tone_stereo_48k(1000, -12.0),
        silence_stereo_48k(1000),
    ]);
    let gated = vad_gated_frames(&stereo);
    assert_eq!((gated.starts, gated.ends), (1, 0));
}
