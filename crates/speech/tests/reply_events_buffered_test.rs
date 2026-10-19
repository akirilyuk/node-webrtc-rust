//! Reply events on the legacy buffered synthesis path (`VOICE_TTS_STREAM_CHUNKS=0`): the whole
//! reply is enqueued at once and must still produce one start and one end.
//!
//! Own test binary: the env var is read per synthesis job, so it must not leak into other tests.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttConfig, TtsConfig, TtsVendor, VoiceAgentConfig,
};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider, VendorFactory};
use node_webrtc_rust_speech::{PcmWriter, SttProvider, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

struct OneChunkTts;

#[async_trait]
impl TtsProvider for OneChunkTts {
    fn vendor_name(&self) -> &'static str {
        "mock-one-chunk"
    }

    async fn synthesize(&self, _text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        // 200 ms of 48 kHz stereo s16 silence.
        Ok(vec![TtsAudioChunk {
            pcm: Bytes::from(vec![0_u8; 192 * 200]),
            duration_ms: 200,
        }])
    }
}

struct OneChunkFactory;

impl VendorFactory for OneChunkFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(OneChunkTts))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn buffered_path_emits_one_start_and_one_end_per_reply() {
    std::env::set_var("VOICE_TTS_STREAM_CHUNKS", "0");
    let mut registry = VendorRegistry::new();
    registry.register_tts(TtsVendor::Mock, Arc::new(OneChunkFactory));
    let config = VoiceAgentConfig {
        stt: None,
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
            transport: None,
            sentence_case: Default::default(),
        }),
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let mut rx = agent.subscribe_events();
    let seen: Arc<Mutex<Vec<SpeechEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            sink.lock().unwrap().push(event);
        }
    });

    for id in ["x", "y"] {
        agent
            .send_text_to_tts_with_options(
                &format!("buffered reply {id}"),
                SendTextToTtsOptions {
                    non_blocking: true,
                    reply_id: Some(id.to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let ends = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.kind == SpeechEventKind::AgentReplyEnd)
            .count();
        if ends >= 2 {
            break;
        }
        assert!(Instant::now() < deadline, "timed out waiting for two ends");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    agent.wait_tts_playback_idle().await.unwrap();

    let order: Vec<(SpeechEventKind, String)> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                SpeechEventKind::AgentReplyStart | SpeechEventKind::AgentReplyEnd
            )
        })
        .map(|e| (e.kind, e.reply_id.clone().unwrap_or_default()))
        .collect();
    assert_eq!(
        order,
        vec![
            (SpeechEventKind::AgentReplyStart, "x".to_string()),
            (SpeechEventKind::AgentReplyEnd, "x".to_string()),
            (SpeechEventKind::AgentReplyStart, "y".to_string()),
            (SpeechEventKind::AgentReplyEnd, "y".to_string()),
        ]
    );
    for end in seen
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e.kind == SpeechEventKind::AgentReplyEnd)
    {
        assert_eq!(end.played_ms, Some(200));
        assert_eq!(end.interrupted, Some(false));
    }
    agent.stop().await.unwrap();
}
