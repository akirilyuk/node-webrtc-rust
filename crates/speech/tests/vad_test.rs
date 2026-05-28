//! VAD unit tests.

use node_webrtc_rust_speech::config::VadConfig;
use node_webrtc_rust_speech::pcm::{i16_samples_to_bytes, pcm_rms_i16};
use node_webrtc_rust_speech::vad::{VadEngine, VadTransition};

fn stereo_frame(samples_per_channel: usize, amplitude: i16) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(samples_per_channel * 4);
    for _ in 0..samples_per_channel {
        pcm.extend_from_slice(&amplitude.to_le_bytes());
        pcm.extend_from_slice(&amplitude.to_le_bytes());
    }
    pcm
}

#[test]
fn energy_vad_detects_loud_frame() {
    let mut config = VadConfig::default();
    config.threshold = 0.05;
    config.min_speech_duration_ms = 20;
    config.min_silence_duration_ms = 20;

    let mut vad = VadEngine::new(config).unwrap();
    let loud = stereo_frame(960, i16::MAX / 3);
    let transitions = vad.process_webrtc_pcm(&loud, 20).unwrap();
    assert!(transitions.0.contains(&VadTransition::SpeechStart));
}

#[test]
fn energy_vad_ignores_silence() {
    let mut config = VadConfig::default();
    config.threshold = 0.2;

    let mut vad = VadEngine::new(config).unwrap();
    let silence = stereo_frame(960, 0);
    let transitions = vad.process_webrtc_pcm(&silence, 20).unwrap();
    assert!(transitions.0.is_empty());
}

#[test]
fn resample_produces_mono_16k() {
    let stereo = stereo_frame(960, 1000);
    let mono = node_webrtc_rust_speech::stereo_48k_to_mono_16k(&stereo);
    assert!(!mono.is_empty());
    assert!(pcm_rms_i16(&mono) > 0.0);
    let _bytes = i16_samples_to_bytes(&mono);
}
