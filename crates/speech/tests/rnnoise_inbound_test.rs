//! VoiceAgent integration: RNNoise inbound preprocessing before VAD.

use std::sync::Arc;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    NoiseSuppressionConfig, NoiseSuppressionProvider, SttVendor, TtsConfig, TtsVendor, VadConfig,
    VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::{VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

fn stereo_frame(samples_per_channel: usize, sample_at: impl Fn(usize) -> i16) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(samples_per_channel * 4);
    for i in 0..samples_per_channel {
        let s = sample_at(i);
        pcm.extend_from_slice(&s.to_le_bytes());
        pcm.extend_from_slice(&s.to_le_bytes());
    }
    pcm
}

fn white_noise_stereo_frame(seed: u32) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3_840);
    let mut state = seed.max(1);
    for _ in 0..960 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let sample = ((state >> 16) as i16).wrapping_mul(4);
        pcm.extend_from_slice(&sample.to_le_bytes());
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    pcm
}

fn sine_440_stereo_frame() -> Vec<u8> {
    stereo_frame(960, |i| {
        let t = i as f32 / 48_000.0;
        let sample = (t * 440.0 * 2.0 * std::f32::consts::PI).sin() * (i16::MAX as f32 * 0.35);
        sample as i16
    })
}

fn loud_dc_stereo_frame() -> Vec<u8> {
    stereo_frame(960, |_| i16::MAX / 3)
}

fn vad_rnnoise_config(provider: NoiseSuppressionProvider) -> VoiceAgentConfig {
    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 40;
    vad.gate_stt = false;
    vad.barge_in.use_vad = false;

    VoiceAgentConfig {
        noise_suppression: NoiseSuppressionConfig { provider },
        stt: None,
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
        }),
        vad,
        ..Default::default()
    }
}

async fn start_agent(config: VoiceAgentConfig) -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    agent
        .attach(Arc::new(|| Ok(None)), Arc::new(|_, _| Ok(())))
        .await
        .unwrap();
    agent.start(None).await.unwrap();
    agent
}

#[tokio::test]
async fn rnnoise_white_noise_does_not_lock_vad_like_dc() {
    let agent_none = start_agent(vad_rnnoise_config(NoiseSuppressionProvider::None)).await;
    let mut events_none = agent_none.subscribe_events();

    for _ in 0..8 {
        agent_none
            .process_inbound_pcm(Bytes::from(loud_dc_stereo_frame()), 20)
            .await
            .unwrap();
    }

    let mut saw_vad_none = false;
    while let Ok(event) = events_none.try_recv() {
        if event.kind == SpeechEventKind::VadTriggered {
            saw_vad_none = true;
        }
    }
    assert!(
        saw_vad_none,
        "control: provider none + loud DC must emit VadTriggered (event bus works)"
    );

    let agent_rn = start_agent(vad_rnnoise_config(NoiseSuppressionProvider::Rnnoise)).await;
    let mut events_rn = agent_rn.subscribe_events();

    let noise = white_noise_stereo_frame(42);
    for i in 0..10 {
        let mut frame = noise.clone();
        frame[0] ^= (i as u8).wrapping_mul(7);
        agent_rn
            .process_inbound_pcm(Bytes::from(frame), 20)
            .await
            .unwrap();
    }
    while events_rn.try_recv().is_ok() {}

    for i in 0..8 {
        let mut frame = noise.clone();
        frame[2] ^= (i as u8).wrapping_mul(11);
        agent_rn
            .process_inbound_pcm(Bytes::from(frame), 20)
            .await
            .unwrap();
    }

    let mut saw_vad_rn = false;
    while let Ok(event) = events_rn.try_recv() {
        if event.kind == SpeechEventKind::VadTriggered {
            saw_vad_rn = true;
        }
    }
    assert!(
        !saw_vad_rn,
        "RNNoise + white noise should not trigger VAD after warmup"
    );

    agent_none.stop().await.unwrap();
    agent_rn.stop().await.unwrap();
}

#[tokio::test]
async fn rnnoise_preserves_440hz_speech_for_vad() {
    let agent = start_agent(vad_rnnoise_config(NoiseSuppressionProvider::Rnnoise)).await;
    let mut events = agent.subscribe_events();

    for i in 0..6 {
        let mut frame = sine_440_stereo_frame();
        frame[4] ^= (i as u8).wrapping_mul(3);
        agent
            .process_inbound_pcm(Bytes::from(frame), 20)
            .await
            .unwrap();
    }

    let mut saw_speech_start = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == SpeechEventKind::VadTriggered {
            saw_speech_start = true;
        }
    }
    assert!(
        saw_speech_start,
        "440 Hz tone should still trigger VAD with RNNoise enabled"
    );

    agent.stop().await.unwrap();
}
