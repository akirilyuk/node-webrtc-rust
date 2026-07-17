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
    vad.barge_in.require_stt_partial = false;

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
    // block_in_place: do not block the tokio worker outright — inbound VAD must run during drain.
    let writer: PcmWriter = Arc::new(move |_pcm, ms| {
        *written_clone.lock().unwrap() += ms;
        tokio::task::block_in_place(|| std::thread::sleep(Duration::from_millis(4)));
        Ok(())
    });
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start(None).await.unwrap();

    let long_text = "interrupt this mock playback please ".repeat(8);
    let expected_ms = mock_tts_duration_ms(&long_text);
    let loud = loud_stereo_frame();

    let agent_arc = Arc::new(agent);
    let agent_interrupt = Arc::clone(&agent_arc);
    let loud_spawn = loud.clone();

    tokio::spawn(async move {
        sleep(Duration::from_millis(40)).await;
        for _ in 0..8 {
            agent_interrupt
                .process_inbound_pcm(Bytes::from(loud_spawn.clone()), 20)
                .await
                .unwrap();
        }
    });

    agent_arc.send_text_to_tts(&long_text).await.unwrap();
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
    vad.barge_in.require_stt_partial = false;

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
    agent.start(None).await.unwrap();

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
    vad.barge_in.require_stt_partial = false;

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
    agent.start(None).await.unwrap();

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
    vad.barge_in.require_stt_partial = false;

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
    agent.start(None).await.unwrap();

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

struct PartialOnPollStt {
    saw_push: std::sync::Mutex<bool>,
    partial_emitted: std::sync::Mutex<bool>,
}

#[async_trait::async_trait]
impl SttProvider for PartialOnPollStt {
    fn vendor_name(&self) -> &'static str {
        "partial-on-poll"
    }

    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        *self.saw_push.lock().unwrap() = true;
        Ok(())
    }

    async fn poll_transcript(
        &mut self,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<SttTranscript>> {
        let pushed = *self.saw_push.lock().unwrap();
        if !pushed {
            return Ok(None);
        }
        let mut emitted = self.partial_emitted.lock().unwrap();
        if *emitted {
            return Ok(None);
        }
        *emitted = true;
        Ok(Some(SttTranscript::Partial("stop".into())))
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
}

struct PartialOnPollFactory;

impl VendorFactory for PartialOnPollFactory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(PartialOnPollStt {
            saw_push: std::sync::Mutex::new(false),
            partial_emitted: std::sync::Mutex::new(false),
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
async fn stt_partial_gated_barge_flushes_agent_tts_after_partial() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(PartialOnPollFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 40;
    vad.gate_stt = true;
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = true;
    vad.barge_in.flush_tts = true;
    vad.barge_in.require_stt_partial = true;
    vad.barge_in.agent_playback_guard_ms = 0;

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
    let mut events = agent.subscribe_events();
    let written_ms: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let written_clone = Arc::clone(&written_ms);
    let writer: PcmWriter = Arc::new(move |_pcm, ms| {
        *written_clone.lock().unwrap() += ms;
        std::thread::sleep(Duration::from_millis(4));
        Ok(())
    });
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start(None).await.unwrap();

    let long_text = "wait for stt partial before barge ".repeat(6);
    let expected_ms = mock_tts_duration_ms(&long_text);
    let loud = loud_stereo_frame();

    let agent_arc = Arc::new(agent);
    let agent_tts = Arc::clone(&agent_arc);
    let text = long_text.clone();

    tokio::spawn(async move {
        sleep(Duration::from_millis(60)).await;
        for _ in 0..8 {
            agent_tts
                .process_inbound_pcm(Bytes::from(loud.clone()), 20)
                .await
                .unwrap();
        }
    });

    agent_arc.send_text_to_tts(&text).await.unwrap();
    agent_arc.wait_tts_playback_idle().await.unwrap();
    agent_arc.stop().await.unwrap();

    let played_ms = *written_ms.lock().unwrap();
    assert!(
        played_ms < expected_ms * 60 / 100,
        "STT partial should gate barge and cut TTS early: played {played_ms} ms, synth ~{expected_ms} ms"
    );

    let mut saw_barge_in = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == SpeechEventKind::BargeIn {
            saw_barge_in = true;
        }
    }
    assert!(saw_barge_in, "expected BargeIn after STT partial");
}

#[tokio::test]
async fn stt_partial_gated_barge_ignores_vad_without_transcript() {
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
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = true;
    vad.barge_in.flush_tts = true;
    vad.barge_in.require_stt_partial = true;

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
    agent.start(None).await.unwrap();

    let long_text = "tone should not barge without stt partial ".repeat(6);
    let expected_ms = mock_tts_duration_ms(&long_text);
    let loud = loud_stereo_frame();

    let agent_arc = Arc::new(agent);
    let agent_tts = Arc::clone(&agent_arc);
    let text = long_text.clone();

    tokio::spawn(async move {
        sleep(Duration::from_millis(60)).await;
        for _ in 0..6 {
            agent_tts
                .process_inbound_pcm(Bytes::from(loud.clone()), 20)
                .await
                .unwrap();
        }
    });

    agent_arc.send_text_to_tts(&text).await.unwrap();
    agent_arc.wait_tts_playback_idle().await.unwrap();
    agent_arc.stop().await.unwrap();

    let played_ms = *written_ms.lock().unwrap();
    assert!(
        played_ms > expected_ms * 80 / 100,
        "without STT partial, VAD noise must not barge: played {played_ms} ms, expected ~{expected_ms} ms"
    );

    let mut saw_barge_in = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == SpeechEventKind::BargeIn {
            saw_barge_in = true;
        }
    }
    assert!(!saw_barge_in, "tone without STT partial must not emit BargeIn");
}

#[tokio::test]
async fn c1_no_partial_emits_user_stt_not_found() {
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
    vad.stt_listen_timeout_ms = 200;
    vad.barge_in.enabled = false;

    let config = VoiceAgentConfig {
        stt: Some(node_webrtc_rust_speech::SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
        }),
        tts: None,
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
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = vec![0_u8; 3840];
    for _ in 0..4 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    for _ in 0..20 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }
    agent.stop().await.unwrap();

    let mut saw_not_found = false;
    let mut saw_final = false;
    let mut saw_vad_triggered = false;
    while let Ok(event) = events.try_recv() {
        match event.kind {
            SpeechEventKind::UserSttNotFound => saw_not_found = true,
            SpeechEventKind::UserSpeechFinal => saw_final = true,
            SpeechEventKind::VadTriggered => saw_vad_triggered = true,
            _ => {}
        }
    }
    assert!(saw_vad_triggered, "expected vad_triggered on SpeechStart");
    assert!(saw_not_found, "C1: expected user_stt_not_found when no partial");
    assert!(!saw_final, "C1: must not emit user_speech_final without partial");
}

struct PartialOnlyStt {
    partial_emitted: std::sync::Mutex<bool>,
}

#[async_trait::async_trait]
impl SttProvider for PartialOnlyStt {
    fn vendor_name(&self) -> &'static str {
        "partial-only"
    }

    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn poll_transcript(
        &mut self,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<SttTranscript>> {
        let mut emitted = self.partial_emitted.lock().unwrap();
        if *emitted {
            return Ok(None);
        }
        *emitted = true;
        Ok(Some(SttTranscript::Partial("hello stall".into())))
    }

    async fn finalize_utterance(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
}

struct PartialOnlyFactory;

impl VendorFactory for PartialOnlyFactory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(PartialOnlyStt {
            partial_emitted: std::sync::Mutex::new(false),
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
async fn c2_partial_stall_forces_user_speech_final() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(PartialOnlyFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 40;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 100;
    vad.utterance_finalize_timeout_ms = 200;
    vad.barge_in.enabled = false;

    let config = VoiceAgentConfig {
        stt: Some(node_webrtc_rust_speech::SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
        }),
        tts: None,
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
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = vec![0_u8; 3840];
    for _ in 0..6 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    for _ in 0..25 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }
    agent.stop().await.unwrap();

    let mut saw_partial = false;
    let mut saw_final = false;
    let mut final_text = String::new();
    while let Ok(event) = events.try_recv() {
        match event.kind {
            SpeechEventKind::UserSpeechPartial => saw_partial = true,
            SpeechEventKind::UserSpeechFinal => {
                saw_final = true;
                final_text = event.text.unwrap_or_default();
            }
            _ => {}
        }
    }
    assert!(saw_partial, "expected user_speech_partial before C2 forced final");
    assert!(saw_final, "C2: expected forced user_speech_final after partial stall");
    assert!(
        final_text.contains("hello"),
        "C2 final should fall back to last partial, got {final_text:?}"
    );
}

#[tokio::test]
async fn c2_partial_stall_no_pcm_forces_user_speech_final() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(PartialOnlyFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 40;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 100;
    vad.utterance_finalize_timeout_ms = 200;
    vad.barge_in.enabled = false;

    let config = VoiceAgentConfig {
        stt: Some(node_webrtc_rust_speech::SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
        }),
        tts: None,
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
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = vec![0_u8; 3840];
    for _ in 0..6 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    // Hold expires and C2 timer arms, but stop PCM before PCM-clock C2 can fire.
    for _ in 0..8 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    tokio::time::sleep(std::time::Duration::from_millis(350)).await;

    let mut saw_partial = false;
    let mut saw_final = false;
    let mut final_text = String::new();
    while let Ok(event) = events.try_recv() {
        match event.kind {
            SpeechEventKind::UserSpeechPartial => saw_partial = true,
            SpeechEventKind::UserSpeechFinal => {
                saw_final = true;
                final_text = event.text.unwrap_or_default();
            }
            _ => {}
        }
    }
    assert!(saw_partial, "expected user_speech_partial before wall-clock C2");
    assert!(
        saw_final,
        "C2 wall-clock: expected forced user_speech_final with zero PCM after partial"
    );
    assert!(
        final_text.contains("hello"),
        "C2 final should fall back to last partial, got {final_text:?}"
    );

    agent.stop().await.unwrap();
}

#[tokio::test]
async fn c2_does_not_force_during_active_vad_speech() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(PartialOnlyFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    // Long min silence — no SpeechEnd during continuous loud frames.
    vad.min_silence_duration_ms = 5000;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 100;
    vad.utterance_finalize_timeout_ms = 200;
    vad.barge_in.enabled = false;

    let config = VoiceAgentConfig {
        stt: Some(node_webrtc_rust_speech::SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
        }),
        tts: None,
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
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    for _ in 0..6 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    for _ in 0..25 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    tokio::time::sleep(std::time::Duration::from_millis(350)).await;

    let mut saw_partial = false;
    let mut saw_final = false;
    while let Ok(event) = events.try_recv() {
        match event.kind {
            SpeechEventKind::UserSpeechPartial => saw_partial = true,
            SpeechEventKind::UserSpeechFinal => saw_final = true,
            _ => {}
        }
    }
    assert!(saw_partial, "expected partial during active speech");
    assert!(
        !saw_final,
        "C2 must not force user_speech_final while VAD still sees speech"
    );

    agent.stop().await.unwrap();
}

async fn run_gated_utterance(agent: &VoiceAgent, loud: &[u8], silent: &[u8]) {
    for _ in 0..6 {
        agent
            .process_inbound_pcm(Bytes::from(loud.to_vec()), 20)
            .await
            .unwrap();
    }
    for _ in 0..25 {
        agent
            .process_inbound_pcm(Bytes::from(silent.to_vec()), 20)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn second_turn_stt_finalizes_after_prior_utterance_final() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 40;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 1000;
    vad.utterance_finalize_timeout_ms = 200;
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
    let mut events = agent.subscribe_events();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = vec![0_u8; 3840];

    run_gated_utterance(&agent, &loud, &silent).await;
    agent.send_text_to_tts("echo").await.unwrap();
    sleep(Duration::from_millis(400)).await;

    // Brief-gap SpeechStart while post-TTS gate hold is still active (multi-turn E2E).
    run_gated_utterance(&agent, &loud, &silent).await;

    agent.stop().await.unwrap();

    let mut finals = 0_u32;
    while let Ok(event) = events.try_recv() {
        if event.kind == SpeechEventKind::UserSpeechFinal {
            finals += 1;
        }
    }
    assert_eq!(
        finals, 2,
        "turn 2 must emit user_speech_final after turn 1 completed (brief-gap SpeechStart)"
    );
}

#[tokio::test]
async fn barge_disabled_stt_on_vad_during_agent_tts() {
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
    let mut events = agent.subscribe_events();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start(None).await.unwrap();

    let long_text = "agent keeps talking ".repeat(6);
    let loud = loud_stereo_frame();
    let agent_arc = Arc::new(agent);
    let agent_tts = Arc::clone(&agent_arc);
    let text = long_text.clone();
    tokio::spawn(async move {
        agent_tts.send_text_to_tts(&text).await.unwrap();
    });
    sleep(Duration::from_millis(80)).await;
    for _ in 0..8 {
        agent_arc
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    agent_arc.stop().await.unwrap();

    let stt_bytes = *bytes.lock().unwrap();
    assert!(stt_bytes > 0, "STT should receive audio on vad_triggered during agent TTS");

    let mut saw_barge = false;
    let mut saw_vad_triggered = false;
    let mut saw_stt_start = false;
    while let Ok(event) = events.try_recv() {
        match event.kind {
            SpeechEventKind::BargeIn => saw_barge = true,
            SpeechEventKind::VadTriggered => saw_vad_triggered = true,
            SpeechEventKind::SttStreamStart => saw_stt_start = true,
            _ => {}
        }
    }
    assert!(saw_vad_triggered, "expected vad_triggered");
    assert!(saw_stt_start, "expected stt_stream_start");
    assert!(!saw_barge, "bargeIn disabled must not emit barge_in during agent TTS");
}

struct SlowMockTts {
    delay: Duration,
    inner: Box<dyn node_webrtc_rust_speech::TtsProvider>,
}

#[async_trait::async_trait]
impl node_webrtc_rust_speech::TtsProvider for SlowMockTts {
    fn vendor_name(&self) -> &'static str {
        "mock-slow"
    }

    async fn synthesize(
        &self,
        text: &str,
    ) -> node_webrtc_rust_speech::SpeechResult<Vec<node_webrtc_rust_speech::TtsAudioChunk>> {
        sleep(self.delay).await;
        self.inner.synthesize(text).await
    }
}

struct SlowMockFactory {
    delay: Duration,
}

impl VendorFactory for SlowMockFactory {
    fn create_stt(
        &self,
        config: &node_webrtc_rust_speech::SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        Ok(Box::new(SlowMockTts {
            delay: self.delay,
            inner: MockFactory.create_tts(config)?,
        }))
    }
}

#[tokio::test]
async fn barge_flush_during_slow_synthesis_discards_late_pcm() {
    let mut registry = VendorRegistry::new();
    registry.register_tts(
        TtsVendor::Mock,
        Arc::new(SlowMockFactory {
            delay: Duration::from_millis(500),
        }),
    );

    let agent = VoiceAgent::new(speaker_config(), Arc::new(registry)).unwrap();
    let written_ms: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let written_clone = Arc::clone(&written_ms);
    let writer: PcmWriter = Arc::new(move |_pcm, ms| {
        *written_clone.lock().unwrap() += ms;
        Ok(())
    });
    let reader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start(None).await.unwrap();

    let long_text = "late synthesis must not replay after barge flush ".repeat(8);
    let expected_full_ms = mock_tts_duration_ms(&long_text);
    let agent = Arc::new(agent);
    let agent_flush = Arc::clone(&agent);
    let text = long_text.clone();
    let speak = tokio::spawn(async move { agent.send_text_to_tts(&text).await });
    sleep(Duration::from_millis(80)).await;
    agent_flush.flush_tts().await.unwrap();
    speak.await.unwrap().unwrap();

    let played_ms = *written_ms.lock().unwrap();
    assert!(
        played_ms < expected_full_ms / 4,
        "flush during synthesize should drop late PCM: played {played_ms} ms, synth ~{expected_full_ms} ms"
    );
    agent_flush.stop().await.unwrap();
}
