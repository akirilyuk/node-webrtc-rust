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

struct FinalizingStt {
    finalize_calls: Arc<Mutex<usize>>,
    poll_calls: Arc<Mutex<u32>>,
    emit_final_on_poll: u32,
    pending_final: Arc<Mutex<Option<String>>>,
}

/// Emits `Final` only on the first `poll_transcript` after `finalize_utterance` (Sherpa-like).
struct DelayedFinalStt {
    finalize_calls: Arc<Mutex<usize>>,
    finalized: Arc<Mutex<bool>>,
}

#[async_trait::async_trait]
impl SttProvider for DelayedFinalStt {
    fn vendor_name(&self) -> &'static str {
        "delayed-final"
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
        if *self.finalized.lock().unwrap() {
            *self.finalized.lock().unwrap() = false;
            return Ok(Some(SttTranscript::Final("hello delayed".into())));
        }
        Ok(None)
    }

    async fn finalize_utterance(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        *self.finalize_calls.lock().unwrap() += 1;
        *self.finalized.lock().unwrap() = true;
        Ok(())
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
}

struct DelayedFinalFactory {
    finalize_calls: Arc<Mutex<usize>>,
    finalized: Arc<Mutex<bool>>,
}

impl VendorFactory for DelayedFinalFactory {
    fn create_stt(
        &self,
        _config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(DelayedFinalStt {
            finalize_calls: Arc::clone(&self.finalize_calls),
            finalized: Arc::clone(&self.finalized),
        }))
    }

    fn create_tts(
        &self,
        _config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn TtsProvider>> {
        MockFactory.create_tts(_config)
    }
}

#[async_trait::async_trait]
impl SttProvider for FinalizingStt {
    fn vendor_name(&self) -> &'static str {
        "finalizing"
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
        if let Some(text) = self.pending_final.lock().unwrap().take() {
            return Ok(Some(SttTranscript::Final(text)));
        }
        let call = {
            let mut guard = self.poll_calls.lock().unwrap();
            *guard += 1;
            *guard
        };
        if call == self.emit_final_on_poll {
            return Ok(Some(SttTranscript::Final("hello".into())));
        }
        Ok(None)
    }

    async fn finalize_utterance(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        *self.finalize_calls.lock().unwrap() += 1;
        *self.pending_final.lock().unwrap() = Some("mock-finalize".into());
        Ok(())
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
}

struct FinalizingFactory {
    finalize_calls: Arc<Mutex<usize>>,
    poll_calls: Arc<Mutex<u32>>,
    emit_final_on_poll: u32,
    pending_final: Arc<Mutex<Option<String>>>,
}

impl VendorFactory for FinalizingFactory {
    fn create_stt(
        &self,
        _config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(FinalizingStt {
            finalize_calls: Arc::clone(&self.finalize_calls),
            poll_calls: Arc::clone(&self.poll_calls),
            emit_final_on_poll: self.emit_final_on_poll,
            pending_final: Arc::clone(&self.pending_final),
        }))
    }

    fn create_tts(
        &self,
        _config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn TtsProvider>> {
        MockFactory.create_tts(_config)
    }
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

#[tokio::test]
async fn gate_stt_defers_user_speaking_end_until_hold_expires() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 60;

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

    let mut end_frame: Option<usize> = None;
    for i in 0..4 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
        while let Ok(event) = rx.try_recv() {
            if event.kind == node_webrtc_rust_speech::events::SpeechEventKind::UserSpeakingEnd {
                end_frame = Some(i);
            }
        }
    }

    assert_eq!(
        end_frame,
        Some(3),
        "with gate_stt, user_speaking_end must not fire on first VAD SpeechEnd frame; \
         expect it when gate hold (60 ms) expires (~4 silent 20 ms frames)"
    );
}

#[tokio::test]
async fn gate_stt_hold_cancelled_when_voice_returns_before_expiry() {
    let finalize_calls = Arc::new(Mutex::new(0_usize));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(FinalizingFactory {
            finalize_calls: Arc::clone(&finalize_calls),
            poll_calls: Arc::new(Mutex::new(0)),
            emit_final_on_poll: 0,
            pending_final: Arc::new(Mutex::new(None)),
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
    // Brief silence arms hold (not long enough to expire 200 ms hold alone from one frame).
    agent
        .process_inbound_pcm(Bytes::from(silent.clone()), 20)
        .await
        .unwrap();
    agent
        .process_inbound_pcm(Bytes::from(silent.clone()), 20)
        .await
        .unwrap();
    // Voice returns before hold drains — must not finalize yet.
    for _ in 0..5 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    let mut saw_end = false;
    while let Ok(event) = rx.try_recv() {
        if event.kind == node_webrtc_rust_speech::events::SpeechEventKind::UserSpeakingEnd {
            saw_end = true;
        }
    }
    assert!(
        !saw_end,
        "user_speaking_end must not fire when voice resumes during gate hold"
    );
    assert_eq!(
        *finalize_calls.lock().unwrap(),
        0,
        "finalize must not run while hold was cancelled by resumed speech"
    );
}

#[tokio::test]
async fn gate_stt_hold_expiry_finalizes_on_same_frame_without_new_speech() {
    let finalize_calls = Arc::new(Mutex::new(0_usize));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(FinalizingFactory {
            finalize_calls: Arc::clone(&finalize_calls),
            poll_calls: Arc::new(Mutex::new(0)),
            emit_final_on_poll: 0,
            pending_final: Arc::new(Mutex::new(None)),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 60;

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

    // Speech end (20 ms silence) + hold drain (60 ms) = 4 silent frames at 20 ms.
    for _ in 0..4 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    assert_eq!(
        *finalize_calls.lock().unwrap(),
        1,
        "STT finalize must run when gate hold expires — not only after the next SpeechStart"
    );
}

#[tokio::test]
async fn gate_stt_hold_skips_finalize_when_poll_already_emitted_final() {
    let finalize_calls = Arc::new(Mutex::new(0_usize));
    let poll_calls = Arc::new(Mutex::new(0_u32));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(FinalizingFactory {
            finalize_calls: Arc::clone(&finalize_calls),
            poll_calls: Arc::clone(&poll_calls),
            emit_final_on_poll: 4,
            pending_final: Arc::new(Mutex::new(None)),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 60;

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
    for _ in 0..4 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    assert_eq!(
        *finalize_calls.lock().unwrap(),
        0,
        "finalize_utterance must be skipped when poll_transcript already returned Final"
    );
}

/// `user_speaking_end` must not precede `user_speech_final` by a separate utterance-close pass.
#[tokio::test]
async fn speaking_end_pairs_with_delayed_stt_final() {
    let finalize_calls = Arc::new(Mutex::new(0_usize));
    let finalized = Arc::new(Mutex::new(false));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(DelayedFinalFactory {
            finalize_calls: Arc::clone(&finalize_calls),
            finalized: Arc::clone(&finalized),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 60;

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
    for _ in 0..4 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    assert_eq!(*finalize_calls.lock().unwrap(), 1);

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event.kind);
    }

    let end_idx = events
        .iter()
        .position(|k| *k == node_webrtc_rust_speech::events::SpeechEventKind::UserSpeakingEnd);
    let final_idx = events
        .iter()
        .position(|k| *k == node_webrtc_rust_speech::events::SpeechEventKind::UserSpeechFinal);

    assert!(end_idx.is_some(), "expected user_speaking_end");
    assert!(final_idx.is_some(), "expected user_speech_final");
    assert_eq!(
        end_idx,
        final_idx.map(|i| i.saturating_sub(1)),
        "user_speaking_end must immediately precede user_speech_final, got order: {events:?}"
    );
}

/// New `SpeechStart` must finalize the previous pending utterance before accepting new audio.
#[tokio::test]
async fn speech_start_completes_pending_utterance_before_new_speech() {
    let finalize_calls = Arc::new(Mutex::new(0_usize));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(FinalizingFactory {
            finalize_calls: Arc::clone(&finalize_calls),
            poll_calls: Arc::new(Mutex::new(0)),
            emit_final_on_poll: 0,
            pending_final: Arc::new(Mutex::new(None)),
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
    let mut rx = agent.subscribe_events();
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start().await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    // First phrase.
    for _ in 0..3 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    // SpeechEnd + gate hold (200 ms); stop ~40 ms before hold would expire (8×20 ms).
    for _ in 0..8 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    assert_eq!(
        *finalize_calls.lock().unwrap(),
        0,
        "hold must not have fully expired before the user resumes"
    );

    // New SpeechStart — must finalize the prior phrase first.
    for _ in 0..3 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    assert_eq!(
        *finalize_calls.lock().unwrap(),
        1,
        "prior utterance must be finalized on SpeechStart, not left pending"
    );

    let mut saw_final = false;
    while let Ok(event) = rx.try_recv() {
        if event.kind == node_webrtc_rust_speech::events::SpeechEventKind::UserSpeechFinal {
            saw_final = true;
        }
    }
    assert!(saw_final, "user_speech_final must be emitted for the first utterance");
}

/// Long pause after a word gap must still finalize without waiting for a new SpeechStart.
#[tokio::test]
async fn gate_stt_finalizes_after_pause_without_new_speech_start() {
    let finalize_calls = Arc::new(Mutex::new(0_usize));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(FinalizingFactory {
            finalize_calls: Arc::clone(&finalize_calls),
            poll_calls: Arc::new(Mutex::new(0)),
            emit_final_on_poll: 0,
            pending_final: Arc::new(Mutex::new(None)),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 60;

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
    let mut rx = agent.subscribe_events();
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start().await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    // First word.
    for _ in 0..3 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    // Pause (speech end + hold drain).
    for _ in 0..4 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    assert_eq!(
        *finalize_calls.lock().unwrap(),
        1,
        "first pause must finalize without a new SpeechStart"
    );

    let mut saw_speaking_end = false;
    while let Ok(event) = rx.try_recv() {
        if event.kind == node_webrtc_rust_speech::events::SpeechEventKind::UserSpeakingEnd {
            saw_speaking_end = true;
        }
    }
    assert!(saw_speaking_end, "user_speaking_end must accompany finalize");

    // Resume counting (second word) — must not require another long pause to arm finalize again.
    for _ in 0..3 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    for _ in 0..4 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    assert_eq!(
        *finalize_calls.lock().unwrap(),
        2,
        "second pause in the same session must also finalize"
    );
}
