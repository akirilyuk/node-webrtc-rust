//! STT pre-roll integration with VoiceAgent + gate_stt.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, TtsProvider, VendorFactory};
use node_webrtc_rust_speech::{VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

fn loud_stereo_frame() -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
    }
    pcm
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
        _config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(CountingStt {
            bytes: Arc::clone(&self.bytes),
        }))
    }

    fn create_tts(
        &self,
        _config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn TtsProvider>> {
        MockFactory.create_tts(_config)
    }
}

fn silent_stereo_frame() -> Vec<u8> {
    vec![0_u8; 3840]
}

#[tokio::test]
async fn gate_stt_pre_roll_ignores_leading_silence() {
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
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 60;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = true;

    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
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
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start().await.unwrap();

    for _ in 0..10 {
        agent
            .process_inbound_pcm(Bytes::from(silent_stereo_frame()), 20)
            .await
            .unwrap();
    }
    assert_eq!(
        *bytes.lock().unwrap(),
        0,
        "leading silence must not reach STT or fill pre-roll"
    );

    let frame = loud_stereo_frame();
    // min_speech_duration_ms=60 → SpeechStart on the 3rd 20 ms voice frame.
    for _ in 0..3 {
        agent
            .process_inbound_pcm(Bytes::from(frame.clone()), 20)
            .await
            .unwrap();
    }
    assert!(
        *bytes.lock().unwrap() >= 640 * 3,
        "speech start should flush only voice frames from pre-roll"
    );
}

#[tokio::test]
async fn gate_stt_hold_passes_trailing_speech_after_speech_end() {
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
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 200;

    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
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
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start().await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    for _ in 0..3 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    let during_speech = *bytes.lock().unwrap();
    assert!(during_speech > 0, "speech should reach STT");

    for _ in 0..2 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }
    let after_end = *bytes.lock().unwrap();
    assert!(after_end > during_speech, "endpoint tail silence should reach STT");

    agent
        .process_inbound_pcm(Bytes::from(loud.clone()), 20)
        .await
        .unwrap();
    assert!(
        *bytes.lock().unwrap() > after_end,
        "trailing speech within gate-hold must still reach STT"
    );
}

#[tokio::test]
async fn gate_stt_pending_gate_disabled_waits_for_speech_start() {
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
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 60;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = true;
    vad.gate_stt_open_on_pending = false;

    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
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
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start().await.unwrap();

    let frame = loud_stereo_frame();
    agent
        .process_inbound_pcm(Bytes::from(frame.clone()), 20)
        .await
        .unwrap();
    assert_eq!(
        *bytes.lock().unwrap(),
        0,
        "with gate_stt_open_on_pending=false, first frame stays in pre-roll only"
    );

    for _ in 0..2 {
        agent
            .process_inbound_pcm(Bytes::from(frame.clone()), 20)
            .await
            .unwrap();
    }
    assert!(
        *bytes.lock().unwrap() >= 640 * 3,
        "STT opens only after SpeechStart when pending gate is disabled"
    );
}

#[tokio::test]
async fn gate_stt_pre_roll_includes_frames_before_speech_start() {
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
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 60;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = true;

    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
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
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let reader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start().await.unwrap();

    let frame = loud_stereo_frame();
    agent
        .process_inbound_pcm(Bytes::from(frame.clone()), 20)
        .await
        .unwrap();
    assert!(
        *bytes.lock().unwrap() >= 640,
        "pending gate opens STT while accumulating min_speech_duration_ms"
    );

    agent
        .process_inbound_pcm(Bytes::from(frame.clone()), 20)
        .await
        .unwrap();
    agent
        .process_inbound_pcm(Bytes::from(frame.clone()), 20)
        .await
        .unwrap();
    assert!(
        *bytes.lock().unwrap() >= 640 * 3,
        "speech start should flush pre-roll including prior frames"
    );

    let after_start = *bytes.lock().unwrap();
    agent
        .process_inbound_pcm(Bytes::from(frame), 20)
        .await
        .unwrap();
    assert!(
        *bytes.lock().unwrap() > after_start,
        "subsequent speech frames should stream to STT"
    );
}

#[tokio::test]
async fn min_silence_default_300_requires_fifteen_silent_frames_to_end() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 300;
    vad.speech_pad_ms = 20;
    vad.gate_stt = false;

    let config = VoiceAgentConfig {
        stt: None,
        tts: None,
        vad,
        ..Default::default()
    };

    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let mut rx = agent.subscribe_events();
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start().await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    for _ in 0..3 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    let mut saw_end = false;
    for i in 0..20 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
        while let Ok(event) = rx.try_recv() {
            if event.kind == node_webrtc_rust_speech::events::SpeechEventKind::UserSpeakingEnd {
                saw_end = true;
                assert!(
                    i >= 14,
                    "default min_silence 300 ms needs ~15 silent 20 ms frames, got end at frame {i}"
                );
            }
        }
    }
    assert!(saw_end, "expected user_speaking_end after sustained silence");
}
