//! STT pre-roll integration with VoiceAgent + gate_stt.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::pcm::{i16_samples_to_bytes, pcm_rms_i16, stereo_48k_to_mono_16k};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, TtsProvider, VendorFactory};
use node_webrtc_rust_speech::stt_pre_roll::stt_pre_roll_capacity_ms;
use node_webrtc_rust_speech::{VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

fn loud_stereo_frame() -> Vec<u8> {
    stereo_frame_with_sample(i16::MAX / 3)
}

fn stereo_frame_with_sample(sample: i16) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&sample.to_le_bytes());
        pcm.extend_from_slice(&sample.to_le_bytes());
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

struct RecordingStt {
    chunks: Arc<Mutex<Vec<Bytes>>>,
}

#[async_trait::async_trait]
impl SttProvider for RecordingStt {
    fn vendor_name(&self) -> &'static str {
        "recording"
    }

    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        self.chunks.lock().unwrap().push(pcm);
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

struct RecordingFactory {
    chunks: Arc<Mutex<Vec<Bytes>>>,
}

impl VendorFactory for RecordingFactory {
    fn create_stt(
        &self,
        _config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(RecordingStt {
            chunks: Arc::clone(&self.chunks),
        }))
    }

    fn create_tts(
        &self,
        _config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn TtsProvider>> {
        MockFactory.create_tts(_config)
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

/// Stereo 48 kHz frame with mono RMS ≈ 0.02 (below default energy VAD threshold 0.05).
fn soft_onset_stereo_frame() -> Vec<u8> {
    let sample: i16 = 655;
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&sample.to_le_bytes());
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    let mono = stereo_48k_to_mono_16k(&pcm);
    assert!(
        pcm_rms_i16(&mono) < 0.05,
        "soft onset frame must stay below VAD threshold"
    );
    pcm
}

fn concat_stt_chunks(chunks: &[Bytes]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in chunks {
        out.extend_from_slice(chunk);
    }
    out
}

fn sherpa_like_vad_config() -> VadConfig {
    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 200;
    vad.speech_pad_ms = 500;
    vad.min_silence_duration_ms = 20;
    vad.gate_stt = true;
    vad.gate_stt_open_on_pending = false;
    vad
}

async fn agent_with_recording_stt(vad: VadConfig) -> (Arc<VoiceAgent>, Arc<Mutex<Vec<Bytes>>>) {
    let chunks = Arc::new(Mutex::new(Vec::new()));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(RecordingFactory {
            chunks: Arc::clone(&chunks),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    (agent, chunks)
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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    for _ in 0..10 {
        agent
            .process_inbound_pcm(Bytes::from(silent_stereo_frame()), 20)
            .await
            .unwrap();
    }
    assert_eq!(
        *bytes.lock().unwrap(),
        0,
        "leading silence must not reach STT (may buffer in pre-roll ring)"
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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

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
    assert!(
        after_end > during_speech,
        "endpoint tail silence should reach STT"
    );

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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

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
    let reader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start(None).await.unwrap();

    let frame = loud_stereo_frame();
    agent
        .process_inbound_pcm(Bytes::from(frame.clone()), 20)
        .await
        .unwrap();
    assert_eq!(
        *bytes.lock().unwrap(),
        0,
        "STT stream opens on vad_triggered (SpeechStart), not during pending accumulation"
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
        "SpeechStart should flush pre-roll including prior frames plus current audio"
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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

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
    assert!(
        saw_end,
        "expected user_speaking_end after sustained silence"
    );
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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

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
async fn set_stt_enabled_false_suppresses_speaking_events() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 20;
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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent.set_stt_enabled(false).await;

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    for _ in 0..3 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    for _ in 0..8 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    let mut saw_start = false;
    let mut saw_end = false;
    while let Ok(event) = rx.try_recv() {
        match event.kind {
            node_webrtc_rust_speech::events::SpeechEventKind::UserSpeakingStart => {
                saw_start = true;
            }
            node_webrtc_rust_speech::events::SpeechEventKind::UserSpeakingEnd => {
                saw_end = true;
            }
            _ => {}
        }
    }
    assert!(
        !saw_start && !saw_end,
        "set_stt_enabled(false) must suppress user_speaking_start/end (VAD still runs)"
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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

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

/// `user_speaking_end` must immediately precede `user_speech_final` (public STT lifecycle contract).
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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

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
        end_idx.map(|i| i + 1),
        final_idx,
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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

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
    assert!(
        saw_final,
        "user_speech_final must be emitted for the first utterance"
    );
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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

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
    assert!(
        saw_speaking_end,
        "user_speaking_end must accompany finalize"
    );

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

/// Cold inbound (no frames before the talker): first STT push must be padded to ring capacity.
#[tokio::test]
async fn gate_stt_cold_inbound_pre_roll_pad_to_capacity() {
    let vad = sherpa_like_vad_config();
    let capacity_ms = stt_pre_roll_capacity_ms(&vad);
    let capacity_bytes = (capacity_ms as usize * 16_000 / 1000) * 2;
    let (agent, chunks) = agent_with_recording_stt(vad).await;

    let loud = loud_stereo_frame();
    let loud_mono = i16_samples_to_bytes(&stereo_48k_to_mono_16k(&loud));

    let mut pre_flush_mono = Vec::new();
    for _ in 0..20 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
        if chunks.lock().unwrap().is_empty() {
            pre_flush_mono.extend_from_slice(loud_mono.as_ref());
        } else {
            break;
        }
    }

    let recorded = chunks.lock().unwrap();
    assert!(
        !recorded.is_empty(),
        "SpeechStart must flush pre-roll to STT"
    );
    let first = recorded[0].as_ref();
    assert_eq!(
        first.len(),
        capacity_bytes,
        "cold inbound flush must be padded to ring capacity ({} ms)",
        capacity_ms
    );
    let pad_len = capacity_bytes - pre_flush_mono.len();
    assert!(pad_len > 0, "cold path must left-pad short pre-roll");
    assert!(
        first[..pad_len].iter().all(|&b| b == 0),
        "lead-in pad must be digital silence"
    );
    assert_eq!(
        &first[pad_len..],
        pre_flush_mono.as_slice(),
        "tail must equal buffered onset frames"
    );
}

/// Brief-gap SpeechStart with STT stream already open must not left-pad the pre-roll flush.
#[tokio::test]
async fn gate_stt_brief_gap_pre_roll_flush_not_padded() {
    let mut vad = sherpa_like_vad_config();
    vad.min_silence_duration_ms = 20;
    vad.stt_gate_hold_ms = 200;
    let capacity_bytes = (stt_pre_roll_capacity_ms(&vad) as usize * 16_000 / 1000) * 2;
    let (agent, chunks) = agent_with_recording_stt(vad).await;

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    // First phrase — opens STT stream.
    for _ in 0..12 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    // Brief silence (SpeechEnd but hold not expired — same utterance).
    for _ in 0..2 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }
    let after_first = chunks.lock().unwrap().len();
    assert!(after_first > 0, "first phrase must reach STT");

    // Resume speech — brief-gap SpeechStart, stream already open.
    for _ in 0..12 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    let recorded = chunks.lock().unwrap();
    for chunk in recorded.iter().skip(after_first) {
        assert_ne!(
            chunk.len(),
            capacity_bytes,
            "brief-gap pre-roll flush must not be left-padded to full ring capacity"
        );
    }
}

#[tokio::test]
async fn gate_stt_pre_roll_flush_includes_soft_onset_before_vad_speech_start() {
    let vad = sherpa_like_vad_config();
    let capacity_bytes = (stt_pre_roll_capacity_ms(&vad) as usize * 16_000 / 1000) * 2;
    let (agent, chunks) = agent_with_recording_stt(vad).await;

    let silent = silent_stereo_frame();
    let soft = soft_onset_stereo_frame();
    let first_loud = stereo_frame_with_sample(5_000);
    let loud = loud_stereo_frame();
    let soft_mono = i16_samples_to_bytes(&stereo_48k_to_mono_16k(&soft));
    let first_loud_mono = i16_samples_to_bytes(&stereo_48k_to_mono_16k(&first_loud));
    let frame_bytes = 640usize;

    for _ in 0..60 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }
    for _ in 0..3 {
        agent
            .process_inbound_pcm(Bytes::from(soft.clone()), 20)
            .await
            .unwrap();
    }
    agent
        .process_inbound_pcm(Bytes::from(first_loud.clone()), 20)
        .await
        .unwrap();
    for _ in 0..9 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    for _ in 0..5 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    let all = concat_stt_chunks(&chunks.lock().unwrap().clone());
    let soft_ref = soft_mono.as_ref();
    assert!(
        all.windows(soft_mono.len()).any(|w| w == soft_ref),
        "STT must receive soft-onset mono bytes from pre-roll flush"
    );

    let soft_idx = all
        .windows(soft_mono.len())
        .position(|w| w == soft_ref)
        .expect("soft onset present");
    assert!(
        soft_idx >= frame_bytes,
        "soft onset must not be the first bytes in the flush"
    );
    let preceding = &all[soft_idx - frame_bytes..soft_idx];
    assert!(
        preceding.iter().all(|&b| b == 0),
        "bytes immediately before soft onset must be leading silence (contiguous ring)"
    );

    assert!(
        all.len() <= capacity_bytes + frame_bytes * 20,
        "flushed pre-roll must respect ring capacity (plus post-start direct frames)"
    );

    let first_loud_ref = first_loud_mono.as_ref();
    let first_loud_count = all
        .windows(first_loud_mono.len())
        .filter(|w| *w == first_loud_ref)
        .count();
    assert_eq!(
        first_loud_count, 1,
        "tagged first loud frame must appear exactly once (no ring+direct duplication)"
    );
}

#[tokio::test]
async fn gate_stt_pre_roll_drops_stale_burst_after_long_silence() {
    let mut vad = sherpa_like_vad_config();
    vad.stt_gate_hold_ms = 60;
    vad.min_silence_duration_ms = 20;
    let (agent, chunks) = agent_with_recording_stt(vad).await;

    let silent = silent_stereo_frame();
    let stale_burst = stereo_frame_with_sample(9_000);
    let new_speech = loud_stereo_frame();
    let stale_mono = i16_samples_to_bytes(&stereo_48k_to_mono_16k(&stale_burst));

    // 700 ms loud burst (fills ring to capacity).
    for _ in 0..35 {
        agent
            .process_inbound_pcm(Bytes::from(stale_burst.clone()), 20)
            .await
            .unwrap();
    }
    // SpeechEnd + gate-hold drain.
    for _ in 0..4 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }
    // 3 s silence — ring should contain only zeros.
    for _ in 0..150 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    let before_second = concat_stt_chunks(&chunks.lock().unwrap().clone());
    let stale_ref = stale_mono.as_ref();
    assert!(
        before_second
            .windows(stale_mono.len())
            .any(|w| w == stale_ref),
        "first burst should have reached STT directly"
    );

    for _ in 0..10 {
        agent
            .process_inbound_pcm(Bytes::from(new_speech.clone()), 20)
            .await
            .unwrap();
    }

    let all = concat_stt_chunks(&chunks.lock().unwrap().clone());
    let second_start = before_second.len();
    let second_utterance = &all[second_start..];
    assert!(
        !second_utterance
            .windows(stale_mono.len())
            .any(|w| w == stale_ref),
        "pre-roll flush for second utterance must not splice stale burst bytes"
    );
}

#[tokio::test]
async fn gate_stt_agent_tts_pre_roll_includes_pre_vad_lead_in() {
    use node_webrtc_rust_speech::PcmWriter;
    use tokio::time::{sleep, Duration};

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
    vad.min_silence_duration_ms = 40;
    vad.speech_pad_ms = 200;
    vad.gate_stt = true;
    vad.barge_in.enabled = true;
    vad.barge_in.require_stt_partial = false;

    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
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
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let long_text = "agent keeps talking ".repeat(6);
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    let agent_arc = Arc::new(agent);
    let agent_tts = Arc::clone(&agent_arc);
    let text = long_text.clone();
    tokio::spawn(async move {
        agent_tts.send_text_to_tts(&text).await.unwrap();
    });
    sleep(Duration::from_millis(80)).await;

    // Simulate user starting to speak while VAD is still accumulating (pre SpeechStart).
    for _ in 0..8 {
        agent_arc
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }
    for _ in 0..3 {
        agent_arc
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    agent_arc.stop().await.unwrap();

    assert!(
        *bytes.lock().unwrap() >= 640 * 8,
        "during agent TTS, STT pre-roll flush should include continuous lookback before VAD SpeechStart"
    );
}
