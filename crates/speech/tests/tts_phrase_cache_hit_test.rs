//! A reply served from the runner phrase cache emits exactly one `tts_phrase_cache_hit`
//! (`chars`, no text); the vendor is asked for the audio only once. Every TTS vendor is cached
//! by default, so the mock vendor used here goes through the cache.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, TtsConfig, TtsVendor, VoiceAgentConfig};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsProgressiveSink, TtsProvider, VendorFactory,
};
use node_webrtc_rust_speech::{PcmWriter, SttProvider, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::sync::broadcast::Receiver;
use tokio::time::timeout;

struct CountingTts {
    calls: Arc<AtomicUsize>,
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
impl TtsProvider for CountingTts {
    fn vendor_name(&self) -> &'static str {
        "mock-counting"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        _text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let chunk = sine_chunk(100);
        if let Some(sink) = sink {
            let _ = sink.send(chunk.clone());
        }
        Ok(vec![chunk])
    }
}

struct CountingFactory {
    calls: Arc<AtomicUsize>,
}

impl VendorFactory for CountingFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(CountingTts {
            calls: Arc::clone(&self.calls),
        }))
    }
}

async fn make_agent(calls: Arc<AtomicUsize>) -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_tts(TtsVendor::Mock, Arc::new(CountingFactory { calls }));
    let config = VoiceAgentConfig {
        stt: None,
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
            sentence_case: Default::default(),
            transport: None,
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
async fn second_identical_reply_emits_one_cache_hit_with_chars() {
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = make_agent(Arc::clone(&calls)).await;
    let mut events = agent.subscribe_events();
    // Multi-byte characters: `chars` counts Unicode scalars, not bytes.
    let text = "Grüße aus dem Phrasen-Cache, hallo";

    agent.send_text_to_tts(text).await.unwrap();
    next_event(&mut events, SpeechEventKind::AgentSpeakingEnd).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    agent.send_text_to_tts(text).await.unwrap();
    let hit = next_event(&mut events, SpeechEventKind::TtsPhraseCacheHit).await;
    assert_eq!(hit.chars, Some(text.chars().count() as u32));
    assert!(hit.text.is_none());
    next_event(&mut events, SpeechEventKind::AgentSpeakingEnd).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "second reply came from the cache"
    );
    // The first reply was a miss, the second a hit: exactly one hit event in total.
    while let Ok(event) = events.try_recv() {
        assert_ne!(event.kind, SpeechEventKind::TtsPhraseCacheHit);
    }
    agent.stop().await.unwrap();
}
