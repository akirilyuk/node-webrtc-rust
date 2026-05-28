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

#[test]
fn min_silence_duration_delays_speech_end() {
    let mut config = VadConfig::default();
    config.threshold = 0.05;
    config.min_speech_duration_ms = 40;
    config.min_silence_duration_ms = 60;
    config.speech_pad_ms = 20;

    let mut vad = VadEngine::new(config).unwrap();
    let loud = stereo_frame(960, i16::MAX / 3);
    let silence = stereo_frame(960, 0);

    for _ in 0..3 {
        let (t, _) = vad.process_webrtc_pcm(&loud, 20).unwrap();
        assert!(!t.contains(&VadTransition::SpeechEnd));
    }
    assert!(vad.is_speaking());

    let (t, _) = vad.process_webrtc_pcm(&silence, 20).unwrap();
    assert!(!t.contains(&VadTransition::SpeechEnd), "one silent frame is not enough");
    assert!(vad.is_speaking());

    let (t, _) = vad.process_webrtc_pcm(&silence, 20).unwrap();
    assert!(!t.contains(&VadTransition::SpeechEnd), "40 ms silence < 60 ms min");

    let (t, _) = vad.process_webrtc_pcm(&silence, 20).unwrap();
    assert!(t.contains(&VadTransition::SpeechEnd));
    assert!(!vad.is_speaking());
}

#[test]
fn short_intra_utterance_gap_does_not_end_speech() {
    let mut config = VadConfig::default();
    config.threshold = 0.05;
    config.min_speech_duration_ms = 40;
    config.min_silence_duration_ms = 300;
    config.speech_pad_ms = 20;

    let mut vad = VadEngine::new(config).unwrap();
    let loud = stereo_frame(960, i16::MAX / 3);
    let silence = stereo_frame(960, 0);

    for _ in 0..3 {
        vad.process_webrtc_pcm(&loud, 20).unwrap();
    }
    assert!(vad.is_speaking());

    for _ in 0..2 {
        let (t, _) = vad.process_webrtc_pcm(&silence, 20).unwrap();
        assert!(!t.contains(&VadTransition::SpeechEnd), "TTS word-gap style pause");
    }
    assert!(vad.is_speaking());

    vad.process_webrtc_pcm(&loud, 20).unwrap();
    assert!(vad.is_speaking());
}

#[test]
fn large_speech_pad_does_not_shorten_min_speech_before_start() {
    let mut config = VadConfig::default();
    config.threshold = 0.05;
    config.min_speech_duration_ms = 60;
    config.min_silence_duration_ms = 20;
    config.speech_pad_ms = 500;

    let mut vad = VadEngine::new(config).unwrap();
    let loud = stereo_frame(960, i16::MAX / 3);

    let (t, _) = vad.process_webrtc_pcm(&loud, 20).unwrap();
    assert!(!t.contains(&VadTransition::SpeechStart));
    assert!(vad.is_pending_speech());

    let (t, _) = vad.process_webrtc_pcm(&loud, 20).unwrap();
    assert!(!t.contains(&VadTransition::SpeechStart));

    let (t, _) = vad.process_webrtc_pcm(&loud, 20).unwrap();
    assert!(t.contains(&VadTransition::SpeechStart));
}

#[test]
fn is_pending_speech_only_before_speech_start() {
    let mut config = VadConfig::default();
    config.threshold = 0.05;
    config.min_speech_duration_ms = 60;
    config.min_silence_duration_ms = 20;

    let mut vad = VadEngine::new(config).unwrap();
    let loud = stereo_frame(960, i16::MAX / 3);
    let silence = stereo_frame(960, 0);

    assert!(!vad.is_pending_speech());
    vad.process_webrtc_pcm(&loud, 20).unwrap();
    assert!(vad.is_pending_speech());

    for _ in 0..2 {
        vad.process_webrtc_pcm(&loud, 20).unwrap();
    }
    assert!(!vad.is_pending_speech());
    assert!(vad.is_speaking());

    for _ in 0..2 {
        vad.process_webrtc_pcm(&silence, 20).unwrap();
    }
    assert!(!vad.is_pending_speech());
}

#[test]
fn vad_disabled_emits_no_transitions() {
    let mut config = VadConfig::default();
    config.enabled = false;
    config.threshold = 0.05;

    let mut vad = VadEngine::new(config).unwrap();
    let loud = stereo_frame(960, i16::MAX / 3);
    let (transitions, active) = vad.process_webrtc_pcm(&loud, 20).unwrap();
    assert!(transitions.is_empty());
    assert!(!active);
    assert!(!vad.is_speaking());
}

#[test]
fn speech_end_allows_new_utterance() {
    let mut config = VadConfig::default();
    config.threshold = 0.05;
    config.min_speech_duration_ms = 40;
    config.min_silence_duration_ms = 40;
    config.speech_pad_ms = 20;

    let mut vad = VadEngine::new(config).unwrap();
    let loud = stereo_frame(960, i16::MAX / 3);
    let silence = stereo_frame(960, 0);

    for _ in 0..3 {
        vad.process_webrtc_pcm(&loud, 20).unwrap();
    }
    for _ in 0..3 {
        vad.process_webrtc_pcm(&silence, 20).unwrap();
    }
    assert!(!vad.is_speaking());

    let (t, _) = vad.process_webrtc_pcm(&loud, 20).unwrap();
    assert!(!t.contains(&VadTransition::SpeechStart));
    assert!(vad.is_pending_speech());

    for _ in 0..2 {
        vad.process_webrtc_pcm(&loud, 20).unwrap();
    }
    assert!(vad.is_speaking());
}
