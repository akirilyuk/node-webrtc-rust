//! `TtsProvider::playout_prebuffer_ms`: for a vendor that streams in bursts with idle gaps the
//! agent holds the reply until enough audio is buffered (or the synthesis ended), so playback
//! does not run dry between bursts. Prebuffer 0 plays on the first chunk as before.
//!
//! Deterministic: paused tokio time, the writer stamps writes with the virtual clock.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttConfig, TtsConfig, TtsVendor, VoiceAgentConfig,
};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsProgressiveSink, TtsProvider, VendorFactory,
};
use node_webrtc_rust_speech::{PcmWriter, SttProvider, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::time::Instant;

#[derive(Clone, Copy)]
enum Step {
    Chunk(u32),
    Pause(u64),
}

struct BurstTts {
    prebuffer_ms: u32,
    steps: Vec<Step>,
}

fn chunk(duration_ms: u32) -> TtsAudioChunk {
    TtsAudioChunk {
        pcm: Bytes::from(vec![0x20u8; 192 * duration_ms as usize]),
        duration_ms,
    }
}

#[async_trait]
impl TtsProvider for BurstTts {
    fn vendor_name(&self) -> &'static str {
        "mock-burst"
    }

    fn playout_prebuffer_ms(&self) -> u32 {
        self.prebuffer_ms
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        _text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let mut all = Vec::new();
        for step in &self.steps {
            match *step {
                Step::Pause(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                Step::Chunk(ms) => {
                    let c = chunk(ms);
                    if let Some(sink) = &sink {
                        let _ = sink.send(c.clone());
                    }
                    all.push(c);
                }
            }
        }
        Ok(all)
    }
}

struct BurstFactory {
    prebuffer_ms: u32,
    steps: Vec<Step>,
}

impl VendorFactory for BurstFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(BurstTts {
            prebuffer_ms: self.prebuffer_ms,
            steps: self.steps.clone(),
        }))
    }
}

type Writes = Arc<Mutex<Vec<(Instant, usize)>>>;

async fn make_agent(prebuffer_ms: u32, steps: Vec<Step>) -> (Arc<VoiceAgent>, Writes) {
    let mut registry = VendorRegistry::new();
    registry.register_tts(
        TtsVendor::Mock,
        Arc::new(BurstFactory {
            prebuffer_ms,
            steps,
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
            transport: None,
            sentence_case: Default::default(),
        }),
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writes: Writes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&writes);
    let writer: PcmWriter = Arc::new(move |pcm, _ms| {
        sink.lock().unwrap().push((Instant::now(), pcm.len()));
        Ok(())
    });
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    (agent, writes)
}

fn tagged(id: &str) -> SendTextToTtsOptions {
    SendTextToTtsOptions {
        non_blocking: true,
        reply_id: Some(id.to_string()),
        ..Default::default()
    }
}

fn record(agent: &VoiceAgent) -> Arc<Mutex<Vec<SpeechEvent>>> {
    let mut rx = agent.subscribe_events();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            sink.lock().unwrap().push(event);
        }
    });
    events
}

fn ms(d: std::time::Duration) -> u128 {
    d.as_millis()
}

/// All audio was played; the drain may pad the last partial frame (at most two 20 ms frames).
fn assert_played(total: usize, audio_ms: usize) {
    let expected = 192 * audio_ms;
    assert!(
        (expected..=expected + 2 * 3840).contains(&total),
        "played {total} bytes for {expected} bytes of audio"
    );
}

const BURSTY: [Step; 5] = [
    Step::Chunk(40),
    Step::Pause(500),
    Step::Chunk(170),
    Step::Chunk(170),
    Step::Chunk(170),
];

#[tokio::test(start_paused = true)]
async fn prebuffer_holds_playback_until_enough_audio_and_leaves_no_gap() {
    let (agent, writes) = make_agent(250, BURSTY.to_vec()).await;
    let t0 = Instant::now();
    agent
        .send_text_to_tts_with_options("burst one", tagged("r"))
        .await
        .unwrap();
    agent.wait_tts_playback_idle().await.unwrap();

    let writes = writes.lock().unwrap().clone();
    assert!(!writes.is_empty());
    // 250 ms is buffered only with the second burst (at 500 ms): nothing is played before.
    assert!(
        ms(writes[0].0 - t0) >= 500,
        "playback started at {} ms",
        ms(writes[0].0 - t0)
    );
    let total: usize = writes.iter().map(|w| w.1).sum();
    assert_played(total, 550);
    // No underrun inside the reply: writes follow each other at the frame pace.
    for pair in writes.windows(2) {
        assert!(
            ms(pair[1].0 - pair[0].0) <= 60,
            "gap of {} ms inside the reply",
            ms(pair[1].0 - pair[0].0)
        );
    }
    agent.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn prebuffer_zero_plays_on_the_first_chunk_as_before() {
    let (agent, writes) = make_agent(0, BURSTY.to_vec()).await;
    let t0 = Instant::now();
    agent
        .send_text_to_tts_with_options("burst zero", tagged("r"))
        .await
        .unwrap();
    agent.wait_tts_playback_idle().await.unwrap();

    let writes = writes.lock().unwrap().clone();
    assert!(
        ms(writes[0].0 - t0) < 100,
        "playback started at {} ms",
        ms(writes[0].0 - t0)
    );
    let total: usize = writes.iter().map(|w| w.1).sum();
    assert_played(total, 550);
    // The 500 ms vendor pause is audible as a gap, which is what the prebuffer prevents.
    let widest = writes
        .windows(2)
        .map(|p| ms(p[1].0 - p[0].0))
        .max()
        .unwrap();
    assert!(widest >= 400, "widest gap {widest} ms");
    agent.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn reply_shorter_than_the_prebuffer_plays_at_end_of_stream() {
    let steps = vec![Step::Chunk(40), Step::Pause(300), Step::Chunk(60)];
    let (agent, writes) = make_agent(250, steps).await;
    let t0 = Instant::now();
    agent
        .send_text_to_tts_with_options("short one", tagged("r"))
        .await
        .unwrap();
    agent.wait_tts_playback_idle().await.unwrap();

    let writes = writes.lock().unwrap().clone();
    let first = ms(writes[0].0 - t0);
    assert!(
        (300..400).contains(&first),
        "playback started at {first} ms"
    );
    let total: usize = writes.iter().map(|w| w.1).sum();
    assert_played(total, 100);
    agent.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn barge_in_during_the_prebuffer_wait_plays_nothing() {
    let steps = vec![Step::Chunk(40), Step::Pause(500), Step::Chunk(200)];
    let (agent, writes) = make_agent(250, steps).await;
    let events = record(&agent);
    agent
        .send_text_to_tts_with_options("cut one", tagged("c"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    agent.flush_tts().await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert!(writes.lock().unwrap().is_empty(), "audio was played");
    // Same events as a reply cut off before its first frame: one end, nothing played, no start.
    let reply: Vec<_> = events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                SpeechEventKind::AgentReplyStart
                    | SpeechEventKind::AgentReplyEnd
                    | SpeechEventKind::AgentSpeakingStart
            )
        })
        .cloned()
        .collect();
    assert_eq!(reply.len(), 1, "{reply:?}");
    assert_eq!(reply[0].kind, SpeechEventKind::AgentReplyEnd);
    assert_eq!(reply[0].played_ms, Some(0));
    assert_eq!(reply[0].interrupted, Some(true));
    agent.stop().await.unwrap();
}
