//! Leftover empty `user_speech_final` while VAD is still speaking must not close
//! the STT stream. The real utterance that continues in the same VAD speech
//! must still get a non-empty final.
//!
//! This crate test scripts a mock `SttProvider`. The dedicated-service path is
//! covered by `vendor-cluster-speech` `leftover_empty_final_fullstack_test`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

fn loud_stereo_frame() -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
    }
    pcm
}

fn silent_stereo_frame() -> Vec<u8> {
    vec![0_u8; 3840]
}

struct LeftoverThenRealStt {
    pushes: Arc<AtomicU32>,
    leftover_emitted: Arc<AtomicBool>,
    partial_emitted: Arc<AtomicBool>,
    finalized: Arc<AtomicBool>,
    real_emitted: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl SttProvider for LeftoverThenRealStt {
    fn vendor_name(&self) -> &'static str {
        "leftover-then-real"
    }

    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        self.pushes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn poll_transcript(
        &mut self,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<SttTranscript>> {
        if !self.leftover_emitted.load(Ordering::Relaxed) && self.pushes.load(Ordering::Relaxed) > 0
        {
            self.leftover_emitted.store(true, Ordering::Relaxed);
            return Ok(Some(SttTranscript::Final(String::new())));
        }
        if self.leftover_emitted.load(Ordering::Relaxed)
            && !self.partial_emitted.load(Ordering::Relaxed)
            && self.pushes.load(Ordering::Relaxed) > 8
        {
            self.partial_emitted.store(true, Ordering::Relaxed);
            return Ok(Some(SttTranscript::Partial("one two three".into())));
        }
        if self.finalized.load(Ordering::Relaxed) && !self.real_emitted.load(Ordering::Relaxed) {
            self.real_emitted.store(true, Ordering::Relaxed);
            return Ok(Some(SttTranscript::Final("one two three".into())));
        }
        Ok(None)
    }

    async fn finalize_utterance(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        self.finalized.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
}

struct LeftoverThenRealFactory {
    pushes: Arc<AtomicU32>,
    leftover_emitted: Arc<AtomicBool>,
}

impl VendorFactory for LeftoverThenRealFactory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(LeftoverThenRealStt {
            pushes: Arc::clone(&self.pushes),
            leftover_emitted: Arc::clone(&self.leftover_emitted),
            partial_emitted: Arc::new(AtomicBool::new(false)),
            finalized: Arc::new(AtomicBool::new(false)),
            real_emitted: Arc::new(AtomicBool::new(false)),
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
async fn empty_leftover_final_while_speaking_does_not_drop_real_utterance() {
    let pushes = Arc::new(AtomicU32::new(0));
    let leftover_emitted = Arc::new(AtomicBool::new(false));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(LeftoverThenRealFactory {
            pushes: Arc::clone(&pushes),
            leftover_emitted: Arc::clone(&leftover_emitted),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 40;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 80;
    vad.barge_in.enabled = false;

    let config = VoiceAgentConfig {
        stt: Some(node_webrtc_rust_speech::SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
            endpoint: None,
        }),
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
        }),
        vad,
        ..Default::default()
    };

    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let mut events = agent.subscribe_events();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    for _ in 0..20 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    assert!(
        leftover_emitted.load(Ordering::Relaxed),
        "scripted STT must emit the leftover empty Final while VAD is speaking"
    );
    let pushes_after_leftover = pushes.load(Ordering::Relaxed);
    assert!(
        pushes_after_leftover > 8,
        "STT stream must stay open after leftover empty Final (pushes={pushes_after_leftover})"
    );

    for _ in 0..20 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    agent.stop().await.unwrap();

    let mut empty_finals = 0_u32;
    let mut nonempty_finals = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.kind == SpeechEventKind::UserSpeechFinal {
            let text = event.text.clone().unwrap_or_default();
            if text.trim().is_empty() {
                empty_finals += 1;
            } else {
                nonempty_finals.push(text);
            }
        }
    }

    assert_eq!(
        empty_finals, 0,
        "empty leftover Final while VAD is speaking must not be emitted"
    );
    assert!(
        nonempty_finals.iter().any(|text| text.contains("one")),
        "real utterance must still emit a non-empty user_speech_final, got {nonempty_finals:?}"
    );
}
