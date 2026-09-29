//! `VoiceAgent::replay_last_utterance` and related language-switch nwr surface.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttConfig, SttVendor, TtsConfig, TtsVendor, UpdateTtsConfigOptions, VadConfig,
    VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, TtsProvider, VendorFactory};
use node_webrtc_rust_speech::utterance_replay::UtteranceReplayBuffer;
use node_webrtc_rust_speech::{VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::time::{sleep, Duration};

fn loud_stereo_frame() -> Vec<u8> {
    let sample = i16::MAX / 3;
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&sample.to_le_bytes());
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    pcm
}

struct TagPushStt {
    tag: String,
    push_bytes: Arc<Mutex<usize>>,
    pending_final: Arc<Mutex<bool>>,
    emitted_partial: Mutex<bool>,
}

#[async_trait]
impl SttProvider for TagPushStt {
    fn vendor_name(&self) -> &'static str {
        "tag-push"
    }

    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        *self.emitted_partial.lock().unwrap() = false;
        Ok(())
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        *self.push_bytes.lock().unwrap() += pcm.len();
        Ok(())
    }

    async fn poll_transcript(
        &mut self,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<SttTranscript>> {
        if *self.pending_final.lock().unwrap() {
            *self.pending_final.lock().unwrap() = false;
            return Ok(Some(SttTranscript::Final(format!(
                "final-{}",
                self.tag
            ))));
        }
        if *self.push_bytes.lock().unwrap() >= 1600
            && !*self.emitted_partial.lock().unwrap()
        {
            *self.emitted_partial.lock().unwrap() = true;
            return Ok(Some(SttTranscript::Partial("…".into())));
        }
        Ok(None)
    }

    async fn finalize_utterance(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        *self.pending_final.lock().unwrap() = true;
        Ok(())
    }
}

struct TagPushSttFactory {
    push_by_tag: Arc<Mutex<HashMap<String, Arc<Mutex<usize>>>>>,
}

impl TagPushSttFactory {
    fn push_count(&self, tag: &str) -> usize {
        self.push_by_tag
            .lock()
            .unwrap()
            .get(tag)
            .map(|c| *c.lock().unwrap())
            .unwrap_or(0)
    }
}

impl VendorFactory for TagPushSttFactory {
    fn create_stt(
        &self,
        config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        let tag = config
            .model
            .clone()
            .unwrap_or_else(|| "default".into());
        let counter = {
            let mut map = self.push_by_tag.lock().unwrap();
            map.entry(tag.clone())
                .or_insert_with(|| Arc::new(Mutex::new(0)))
                .clone()
        };
        Ok(Box::new(TagPushStt {
            tag,
            push_bytes: counter,
            pending_final: Arc::new(Mutex::new(false)),
            emitted_partial: Mutex::new(false),
        }))
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

fn agent_config(stt_model: &str) -> VoiceAgentConfig {
    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.gate_stt = true;
    vad.min_silence_duration_ms = 200;
    vad.stt_gate_hold_ms = 100;
    VoiceAgentConfig {
        stt: Some(SttConfig {
            provider: SttVendor::Mock,
            model: Some(stt_model.into()),
            model_path: Some(format!("/models/stt/{stt_model}.onnx")),
            language: Some("en".into()),
            api_key: None,
            endpoint: Some("grpc://stt-pool-a".into()),
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
    }
}

async fn drive_one_utterance(agent: &VoiceAgent) {
    for _ in 0..40 {
        agent
            .process_inbound_pcm(Bytes::from(loud_stereo_frame()), 20)
            .await
            .unwrap();
    }
    sleep(Duration::from_millis(50)).await;
    for _ in 0..80 {
        agent
            .process_inbound_pcm(Bytes::from(vec![0_u8; 3840]), 20)
            .await
            .unwrap();
    }
    sleep(Duration::from_millis(400)).await;
}

#[tokio::test]
async fn replay_last_utterance_re_emits_final_on_new_stt_provider() {
    let push_by_tag = Arc::new(Mutex::new(HashMap::new()));
    let factory = TagPushSttFactory {
        push_by_tag: Arc::clone(&push_by_tag),
    };
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(factory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let agent = VoiceAgent::new(agent_config("a"), Arc::new(registry)).unwrap();
    let pcm_writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let pcm_reader: node_webrtc_rust_speech::PcmReader = Arc::new(|| Ok(None));
    agent.attach(pcm_reader, pcm_writer).await.unwrap();
    agent.start(None).await.unwrap();

    let mut rx = agent.subscribe_events();
    drive_one_utterance(&agent).await;

    let mut next = agent_config("b").stt.unwrap();
    next.model = Some("b".into());
    next.language = Some("de".into());
    agent.update_stt_config(next).await.unwrap();

    while rx.try_recv().is_ok() {}

    agent.replay_last_utterance().await.unwrap();

    let factory_ref = TagPushSttFactory {
        push_by_tag: Arc::clone(&push_by_tag),
    };
    assert!(
        factory_ref.push_count("b") > 0,
        "new STT provider should receive replay PCM"
    );

    let replay_final = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let ev = rx.recv().await.expect("event bus closed");
            if ev.kind == SpeechEventKind::UserSpeechFinal && ev.replay == Some(true) {
                return ev;
            }
        }
    })
    .await
    .expect("timed out waiting for replay final");
    assert!(replay_final.replaces_utterance_id.is_some());
    agent.stop().await.unwrap();
}

#[test]
fn replay_last_utterance_ring_buffer_overflow() {
    let mut buf = UtteranceReplayBuffer::with_max_bytes(200);
    buf.begin_utterance();
    buf.push(&[0_u8; 120]);
    buf.push(&[0_u8; 120]);
    assert!(buf.overflowed());
}

#[tokio::test]
async fn replay_last_utterance_returns_err_when_overflow_snapshot() {
    let push_by_tag = Arc::new(Mutex::new(HashMap::new()));
    let factory = TagPushSttFactory {
        push_by_tag: Arc::clone(&push_by_tag),
    };
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(factory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let agent = VoiceAgent::new(agent_config("a"), Arc::new(registry)).unwrap();
    let pcm_writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let pcm_reader: node_webrtc_rust_speech::PcmReader = Arc::new(|| Ok(None));
    agent.attach(pcm_reader, pcm_writer).await.unwrap();
    agent.start(None).await.unwrap();

    for _ in 0..900 {
        agent
            .process_inbound_pcm(Bytes::from(loud_stereo_frame()), 20)
            .await
            .unwrap();
    }
    for _ in 0..150 {
        agent
            .process_inbound_pcm(Bytes::from(vec![0_u8; 3840]), 20)
            .await
            .unwrap();
    }
    sleep(Duration::from_millis(900)).await;

    let err = agent.replay_last_utterance().await.unwrap_err();
    assert!(
        err.to_string().contains("replay unavailable"),
        "unexpected error: {err}"
    );
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn replay_last_utterance_utterance_ids_on_start_and_final() {
    let push_by_tag = Arc::new(Mutex::new(HashMap::new()));
    let factory = TagPushSttFactory {
        push_by_tag: Arc::clone(&push_by_tag),
    };
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(factory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let agent = VoiceAgent::new(agent_config("a"), Arc::new(registry)).unwrap();
    let pcm_writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let pcm_reader: node_webrtc_rust_speech::PcmReader = Arc::new(|| Ok(None));
    agent.attach(pcm_reader, pcm_writer).await.unwrap();
    agent.start(None).await.unwrap();

    let mut rx = agent.subscribe_events();
    drive_one_utterance(&agent).await;

    let mut start_id = None;
    let mut final_id = None;
    while let Ok(ev) = rx.try_recv() {
        if ev.kind == SpeechEventKind::UserSpeakingStart {
            start_id = ev.utterance_id.clone();
        }
        if ev.kind == SpeechEventKind::UserSpeechFinal && ev.replay != Some(true) {
            final_id = ev.utterance_id.clone();
        }
    }
    assert_eq!(start_id, final_id);
    assert!(start_id.is_some());
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn update_tts_cancel_inflight_flushes_pending_synthesis() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let agent = VoiceAgent::new(agent_config("a"), Arc::new(registry)).unwrap();
    let pcm_writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let pcm_reader: node_webrtc_rust_speech::PcmReader = Arc::new(|| Ok(None));
    agent.attach(pcm_reader, pcm_writer).await.unwrap();
    agent.start(None).await.unwrap();

    let mut rx = agent.subscribe_events();
    agent
        .send_text_to_tts("long blocking phrase for cancel test")
        .await
        .unwrap();

    let mut next_tts = agent_config("a").tts.unwrap();
    next_tts.voice = Some("other-voice".into());
    agent
        .update_tts_config(
            next_tts,
            UpdateTtsConfigOptions {
                cancel_inflight: true,
            },
        )
        .await
        .unwrap();

    let mut saw_agent_end = false;
    sleep(Duration::from_millis(50)).await;
    while let Ok(ev) = rx.try_recv() {
        if ev.kind == SpeechEventKind::AgentSpeakingEnd {
            saw_agent_end = true;
        }
    }
    assert!(saw_agent_end, "cancel_inflight should flush TTS playback");
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn replay_last_utterance_stt_config_updated_carries_model_and_endpoint() {
    let push_by_tag = Arc::new(Mutex::new(HashMap::new()));
    let factory = TagPushSttFactory {
        push_by_tag: Arc::clone(&push_by_tag),
    };
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(factory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let agent = VoiceAgent::new(agent_config("a"), Arc::new(registry)).unwrap();
    let pcm_writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let pcm_reader: node_webrtc_rust_speech::PcmReader = Arc::new(|| Ok(None));
    agent.attach(pcm_reader, pcm_writer).await.unwrap();
    agent.start(None).await.unwrap();

    let mut rx = agent.subscribe_events();
    let mut next = agent_config("b").stt.unwrap();
    next.language = Some("de".into());
    next.endpoint = Some("grpc://stt-pool-b".into());
    agent.update_stt_config(next).await.unwrap();
    sleep(Duration::from_millis(20)).await;

    let mut saw = false;
    while let Ok(ev) = rx.try_recv() {
        if ev.kind == SpeechEventKind::SttConfigUpdated {
            assert_eq!(ev.language.as_deref(), Some("de"));
            assert_eq!(ev.model_path.as_deref(), Some("/models/stt/b.onnx"));
            assert_eq!(ev.endpoint.as_deref(), Some("grpc://stt-pool-b"));
            saw = true;
        }
    }
    assert!(saw);
    agent.stop().await.unwrap();
}
