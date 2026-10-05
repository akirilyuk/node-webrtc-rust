//! `replay_last_utterance` / `update_stt_config` must work after agent TTS playback ends.
//!
//! Config: VAD enabled, `gate_stt` true, `stt_gate_hold_ms` 100, `min_silence_duration_ms` 200.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEvent;
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, TtsProvider, VendorFactory};
use node_webrtc_rust_speech::{VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::sync::broadcast::Receiver;
use tokio::time::{sleep, Duration, Instant};

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
            return Ok(Some(SttTranscript::Final(format!("final-{}", self.tag))));
        }
        if *self.push_bytes.lock().unwrap() >= 1600 && !*self.emitted_partial.lock().unwrap() {
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

impl VendorFactory for TagPushSttFactory {
    fn create_stt(
        &self,
        config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        let tag = config.model.clone().unwrap_or_else(|| "default".into());
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

type Events = Receiver<SpeechEvent>;

async fn make_agent(
    cfg: VoiceAgentConfig,
) -> (
    Arc<VoiceAgent>,
    Arc<Mutex<HashMap<String, Arc<Mutex<usize>>>>>,
) {
    let push_by_tag = Arc::new(Mutex::new(HashMap::new()));
    let factory = TagPushSttFactory {
        push_by_tag: Arc::clone(&push_by_tag),
    };
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(factory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
    let agent = VoiceAgent::new(cfg, Arc::new(registry)).unwrap();
    let pcm_writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let pcm_reader: node_webrtc_rust_speech::PcmReader = Arc::new(|| Ok(None));
    agent.attach(pcm_reader, pcm_writer).await.unwrap();
    agent.start(None).await.unwrap();
    (agent, push_by_tag)
}

async fn wait_kind(rx: &mut Events, kind: SpeechEventKind, secs: u64) -> SpeechEvent {
    tokio::time::timeout(Duration::from_secs(secs), async {
        loop {
            let ev = rx.recv().await.expect("event bus closed");
            if ev.kind == kind {
                return ev;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {kind:?}"))
}

/// Feeds 20 ms silence frames in real time until `AgentSpeakingEnd` is observed.
async fn silence_until_agent_end(agent: &VoiceAgent, rx: &mut Events) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(Instant::now() < deadline, "no AgentSpeakingEnd");
        agent
            .process_inbound_pcm(Bytes::from(vec![0_u8; 3840]), 20)
            .await
            .unwrap();
        sleep(Duration::from_millis(20)).await;
        while let Ok(ev) = rx.try_recv() {
            if ev.kind == SpeechEventKind::AgentSpeakingEnd {
                return;
            }
        }
    }
}

async fn feed_silence(agent: &VoiceAgent, ms: u32) {
    for _ in 0..(ms / 20) {
        agent
            .process_inbound_pcm(Bytes::from(vec![0_u8; 3840]), 20)
            .await
            .unwrap();
        sleep(Duration::from_millis(20)).await;
    }
}

/// utterance -> final -> agent TTS -> playback end -> [extra silence] -> update + replay.
async fn run_scenario(interruptible: bool, extra_silence_ms: Option<u32>) {
    let (agent, push_by_tag) = make_agent(agent_config("a")).await;
    let mut rx = agent.subscribe_events();
    drive_one_utterance(&agent).await;
    wait_kind(&mut rx, SpeechEventKind::UserSpeechFinal, 2).await;

    agent
        .send_text_to_tts_with_options(
            "please wait a moment",
            SendTextToTtsOptions {
                non_blocking: true,
                interruptible,
            },
        )
        .await
        .unwrap();
    silence_until_agent_end(&agent, &mut rx).await;
    if let Some(ms) = extra_silence_ms {
        feed_silence(&agent, ms).await;
    }

    while rx.try_recv().is_ok() {}
    let mut next = agent_config("b").stt.unwrap();
    next.language = Some("de".into());
    agent.update_stt_config(next).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if rx.recv().await.unwrap().kind == SpeechEventKind::SttConfigUpdated {
                return;
            }
        }
    })
    .await
    .expect("stt_config_updated must be emitted immediately (not deferred)");

    agent
        .replay_last_utterance()
        .await
        .expect("first replay call must succeed");
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
    let b = push_by_tag
        .lock()
        .unwrap()
        .get("b")
        .map(|c| *c.lock().unwrap());
    assert!(b.unwrap_or(0) > 0, "STT b must receive replay PCM");
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn replay_after_agent_playback_applies_stt_update_and_replays() {
    // hold 100 ms + 500 ms
    run_scenario(true, Some(600)).await;
}

#[tokio::test]
async fn replay_after_noninterruptible_playback_applies_stt_update_and_replays() {
    run_scenario(false, Some(600)).await;
}

#[tokio::test]
async fn replay_immediately_after_agent_playback_end() {
    run_scenario(true, None).await;
}

#[tokio::test]
async fn utterance_spoken_during_agent_playback_still_finalizes() {
    let (agent, _push) = make_agent(agent_config("a")).await;
    let mut rx = agent.subscribe_events();
    // Protected (non-interruptible) job keeps playing while the caller talks over it.
    agent
        .send_text_to_tts_with_options(
            "this is a rather long agent sentence for overlap",
            SendTextToTtsOptions {
                non_blocking: true,
                interruptible: false,
            },
        )
        .await
        .unwrap();
    wait_kind(&mut rx, SpeechEventKind::AgentSpeakingStart, 5).await;
    for _ in 0..20 {
        agent
            .process_inbound_pcm(Bytes::from(loud_stereo_frame()), 20)
            .await
            .unwrap();
        sleep(Duration::from_millis(20)).await;
    }
    for _ in 0..15 {
        agent
            .process_inbound_pcm(Bytes::from(vec![0_u8; 3840]), 20)
            .await
            .unwrap();
        sleep(Duration::from_millis(20)).await;
    }
    // Playback ends; keep feeding silence so the post-playback hold can finalize.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut saw_final = false;
    while Instant::now() < deadline && !saw_final {
        agent
            .process_inbound_pcm(Bytes::from(vec![0_u8; 3840]), 20)
            .await
            .unwrap();
        sleep(Duration::from_millis(20)).await;
        while let Ok(ev) = rx.try_recv() {
            if ev.kind == SpeechEventKind::UserSpeechFinal {
                saw_final = true;
            }
        }
    }
    assert!(
        saw_final,
        "utterance spoken during playback must still finalize"
    );
    agent.stop().await.unwrap();
}
