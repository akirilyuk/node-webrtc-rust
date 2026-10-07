//! A reply whose synthesis fails ends only that reply: the agent emits `agent_speak_failed`
//! and keeps running, and a refused-then-retried start is reported as `tts_wait`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, TtsConfig, TtsVendor, VoiceAgentConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsOpenWait, TtsProgressiveSink, TtsProvider, VendorFactory,
};
use node_webrtc_rust_speech::{PcmWriter, SttProvider, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::sync::broadcast::Receiver;
use tokio::time::timeout;

/// Fails the first `fail_first` syntheses, then plays one chunk. The second call also reports
/// an open-wait record, as the cluster vendor does after retrying a refused start.
struct FlakyTts {
    calls: Arc<AtomicUsize>,
    fail_first: usize,
    pending_wait: Mutex<Option<TtsOpenWait>>,
}

fn sine_chunk(duration_ms: u32) -> TtsAudioChunk {
    let samples = (48_000 * duration_ms / 1000) as usize;
    let mut pcm = Vec::with_capacity(samples * 4);
    for i in 0..samples {
        let t = i as f32 / 48_000.0;
        let sample =
            ((t * 440.0 * 2.0 * std::f32::consts::PI).sin() * 0.2 * i16::MAX as f32) as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    TtsAudioChunk {
        pcm: Bytes::from(pcm),
        duration_ms,
    }
}

#[async_trait]
impl TtsProvider for FlakyTts {
    fn vendor_name(&self) -> &'static str {
        "mock-flaky"
    }

    fn take_open_wait(&self) -> Option<TtsOpenWait> {
        self.pending_wait.lock().unwrap().take()
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        _text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= self.fail_first {
            return Err(SpeechError::Vendor {
                vendor: "cluster-sherpa".into(),
                message: "speech models are still loading".into(),
            });
        }
        *self.pending_wait.lock().unwrap() = Some(TtsOpenWait {
            wait_ms: 1500,
            attempts: 7,
            reason: "Unavailable speech models are still loading".into(),
        });
        let chunk = sine_chunk(100);
        if let Some(sink) = sink {
            let _ = sink.send(chunk.clone());
        }
        Ok(vec![chunk])
    }
}

struct FlakyFactory {
    calls: Arc<AtomicUsize>,
    fail_first: usize,
}

impl VendorFactory for FlakyFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(FlakyTts {
            calls: Arc::clone(&self.calls),
            fail_first: self.fail_first,
            pending_wait: Mutex::new(None),
        }))
    }
}

async fn make_agent(fail_first: usize) -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_tts(
        TtsVendor::Mock,
        Arc::new(FlakyFactory {
            calls: Arc::new(AtomicUsize::new(0)),
            fail_first,
        }),
    );
    let config = VoiceAgentConfig {
        stt: None,
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
        }),
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent
}

async fn next_event(events: &mut Receiver<SpeechEvent>, kind: SpeechEventKind) -> SpeechEvent {
    timeout(Duration::from_secs(30), async {
        loop {
            let event = events.recv().await.expect("event bus closed");
            if event.kind == kind {
                return event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {kind:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_reply_emits_speak_failed_and_next_reply_plays() {
    let agent = make_agent(1).await;
    let mut events = agent.subscribe_events();

    // The failing reply does not surface as an error to the host.
    agent.send_text_to_tts("first reply").await.unwrap();
    let failed = next_event(&mut events, SpeechEventKind::AgentSpeakFailed).await;
    assert!(
        failed
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("speech models are still loading"),
        "reason={:?}",
        failed.reason
    );

    // The agent keeps running: the next reply is synthesized and played.
    agent.send_text_to_tts("second reply").await.unwrap();
    next_event(&mut events, SpeechEventKind::AgentSpeakingStart).await;
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retried_start_emits_tts_wait_with_fields() {
    let agent = make_agent(0).await;
    let mut events = agent.subscribe_events();

    agent.send_text_to_tts("hello").await.unwrap();
    let wait = next_event(&mut events, SpeechEventKind::TtsWait).await;
    assert_eq!(wait.wait_ms, Some(1500));
    assert_eq!(wait.attempts, Some(7));
    assert_eq!(
        wait.reason.as_deref(),
        Some("Unavailable speech models are still loading")
    );
    agent.stop().await.unwrap();
}
