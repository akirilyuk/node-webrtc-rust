//! VoiceAgent integration: barge-in during TTS drain.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::time::{sleep, Duration};

fn loud_stereo_frame() -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
    }
    pcm
}

fn mock_tts_duration_ms(text: &str) -> u32 {
    (text.len() as u32 * 50).clamp(100, 5000)
}

fn speaker_config() -> VoiceAgentConfig {
    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 40;
    vad.gate_stt = false;
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = true;
    vad.barge_in.flush_tts = true;
    vad.barge_in.agent_playback_guard_ms = 0;

    VoiceAgentConfig {
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

struct CountingStt {
    bytes: Arc<Mutex<usize>>,
}

#[async_trait::async_trait]
impl SttProvider for CountingStt {
    fn vendor_name(&self) -> &'static str {
        "counting"
    }

    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        *self.bytes.lock().unwrap() += pcm.len();
        Ok(())
    }

    async fn poll_transcript(
        &mut self,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<SttTranscript>> {
        Ok(None)
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
}

struct CountingFactory {
    bytes: Arc<Mutex<usize>>,
}

impl VendorFactory for CountingFactory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(CountingStt {
            bytes: Arc::clone(&self.bytes),
        }))
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

#[tokio::test]
async fn barge_in_during_tts_drain_truncates_outbound_pcm() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let agent = VoiceAgent::new(speaker_config(), Arc::new(registry)).unwrap();
    let mut events = agent.subscribe_events();

    let written_ms: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let written_clone = Arc::clone(&written_ms);
    // Pace outbound frames so barge-in can fire mid-drain (mock TTS enqueues ~5 s of PCM at once).
    let writer: PcmWriter = Arc::new(move |_pcm, ms| {
        *written_clone.lock().unwrap() += ms;
        std::thread::sleep(Duration::from_millis(4));
        Ok(())
    });
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start().await.unwrap();

    let long_text = "interrupt this mock playback please ".repeat(8);
    let expected_ms = mock_tts_duration_ms(&long_text);
    let loud = loud_stereo_frame();

    let agent_arc = std::sync::Arc::new(agent);
    let agent_interrupt = std::sync::Arc::clone(&agent_arc);
    let loud_spawn = loud.clone();

    tokio::spawn(async move {
        sleep(Duration::from_millis(80)).await;
        for _ in 0..6 {
            agent_interrupt
                .process_inbound_pcm(Bytes::from(loud_spawn.clone()), 20)
                .await
                .unwrap();
        }
    });

    let agent_tts = std::sync::Arc::clone(&agent_arc);
    let text = long_text.clone();
    tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current()
            .block_on(async move { agent_tts.send_text_to_tts(&text).await })
    })
    .await
    .unwrap()
    .unwrap();
    agent_arc.wait_tts_playback_idle().await.unwrap();

    agent_arc.stop().await.unwrap();

    let played_ms = *written_ms.lock().unwrap();
    assert!(
        played_ms < expected_ms * 60 / 100,
        "barge-in should cut TTS early: played {played_ms} ms, mock synth ~{expected_ms} ms"
    );
    assert!(played_ms > 0, "some TTS should play before barge-in");

    let mut saw_barge_in = false;
    let mut saw_agent_end = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == SpeechEventKind::BargeIn {
            saw_barge_in = true;
        }
        if event.kind == SpeechEventKind::AgentSpeakingEnd {
            saw_agent_end = true;
        }
    }
    assert!(saw_barge_in, "expected BargeIn event on user SpeechStart");
    assert!(
        saw_agent_end,
        "barge-in flush must emit agent_speaking_end so app state can accept the next reply"
    );
}

#[tokio::test]
async fn agent_playback_guard_suppresses_early_barge_in() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 40;
    vad.gate_stt = false;
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = true;
    vad.barge_in.flush_tts = true;
    vad.barge_in.agent_playback_guard_ms = 500;

    let config = VoiceAgentConfig {
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
    };

    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let mut events = agent.subscribe_events();
    let written_ms: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let written_clone = Arc::clone(&written_ms);
    let writer: PcmWriter = Arc::new(move |_pcm, ms| {
        *written_clone.lock().unwrap() += ms;
        Ok(())
    });
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start().await.unwrap();

    let long_text = "guard should protect this playback ".repeat(6);
    let expected_ms = mock_tts_duration_ms(&long_text);
    let loud = loud_stereo_frame();

    let agent_arc = Arc::new(agent);
    let agent_tts = Arc::clone(&agent_arc);
    let text = long_text.clone();
    let tts_task = tokio::spawn(async move {
        agent_tts.send_text_to_tts(&text).await
    });

    sleep(Duration::from_millis(80)).await;
    for _ in 0..6 {
        agent_arc
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    tts_task.await.unwrap().unwrap();
    agent_arc.wait_tts_playback_idle().await.unwrap();
    agent_arc.stop().await.unwrap();

    let played_ms = *written_ms.lock().unwrap();
    assert!(
        played_ms > expected_ms * 80 / 100,
        "playback guard should allow most of TTS: played {played_ms} ms, expected ~{expected_ms} ms"
    );

    let mut saw_barge_in = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == SpeechEventKind::BargeIn {
            saw_barge_in = true;
        }
    }
    assert!(
        !saw_barge_in,
        "VAD speech during guard window must not emit BargeIn"
    );
}

#[tokio::test]
async fn use_vad_false_skips_auto_barge_on_speech_start() {
    let mut registry = VendorRegistry::new();
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = false;
    vad.barge_in.flush_tts = true;

    let config = VoiceAgentConfig {
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
    };

    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let mut events = agent.subscribe_events();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start().await.unwrap();

    let loud = loud_stereo_frame();
    for _ in 0..6 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    let mut saw_barge_in = false;
    let mut saw_speech_start = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == SpeechEventKind::BargeIn {
            saw_barge_in = true;
        }
        if event.kind == SpeechEventKind::UserSpeakingStart {
            saw_speech_start = true;
        }
    }
    assert!(saw_speech_start, "VAD should still emit user_speaking_start");
    assert!(!saw_barge_in, "use_vad=false must not auto barge-in on SpeechStart");
}

#[tokio::test]
async fn speech_end_during_agent_speaking_defers_finalize_until_playback_ends() {
    let bytes = Arc::new(Mutex::new(0_usize));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(CountingFactory {
            bytes: Arc::clone(&bytes),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 40;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 500;
    vad.barge_in.enabled = false;

    let config = VoiceAgentConfig {
        stt: Some(node_webrtc_rust_speech::SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
        }),
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
        }),
        vad,
        ..Default::default()
    };

    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start().await.unwrap();

    let long_text = "agent still speaking on outbound ".repeat(6);
    let loud = loud_stereo_frame();
    let silent = vec![0_u8; 3840];

    let agent_arc = Arc::new(agent);
    let agent_tts = Arc::clone(&agent_arc);
    let text = long_text.clone();
    let tts_task = tokio::spawn(async move {
        agent_tts.send_text_to_tts(&text).await.unwrap();
        agent_tts.wait_tts_playback_idle().await.unwrap();
    });

    sleep(Duration::from_millis(80)).await;
    for _ in 0..3 {
        agent_arc
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    for _ in 0..3 {
        agent_arc
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }
    let during_agent = *bytes.lock().unwrap();
    tts_task.await.unwrap();
    agent_arc.stop().await.unwrap();
    let after_playback = *bytes.lock().unwrap();

    assert!(
        during_agent <= after_playback.saturating_add(3840 * 4),
        "SpeechEnd during agent_speaking should not finalize STT until playback ends; \
         bytes during agent={during_agent}, after={after_playback}"
    );
}
