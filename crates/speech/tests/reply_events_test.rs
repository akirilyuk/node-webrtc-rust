//! Per-reply events: `agent_reply_start` / `agent_reply_end` for replies sent with a `reply_id`,
//! and the reply id on `tts_phrase_cache_hit` / `tts_wait` / `agent_speak_failed`.
//!
//! The mock TTS follows a script of chunks and pauses. Playback is paced in real time, so the
//! `played_ms` assertions use windows, never exact values.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttConfig, TtsConfig, TtsVendor, VoiceAgentConfig,
};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsOpenWait, TtsProgressiveSink, TtsProvider, VendorFactory,
};
use node_webrtc_rust_speech::{PcmWriter, SttProvider, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

#[derive(Clone)]
enum Step {
    /// Send one chunk of this many ms of audio.
    Chunk(u32),
    /// Pause before the next step (a vendor that is slow between chunks).
    Pause(u64),
}

#[derive(Clone)]
struct Script {
    steps: Vec<Step>,
    /// Fail after the steps ran (or at once when there are none).
    fail: bool,
    open_wait: bool,
}

impl Script {
    fn chunks(steps: Vec<Step>) -> Self {
        Self {
            steps,
            fail: false,
            open_wait: false,
        }
    }
}

/// Picks the script by the prefix of the text, so one agent can play different replies.
struct ScriptTts {
    scripts: Arc<Vec<(&'static str, Script)>>,
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
impl TtsProvider for ScriptTts {
    fn vendor_name(&self) -> &'static str {
        "mock-script"
    }

    fn take_open_wait(&self) -> Option<TtsOpenWait> {
        self.pending_wait.lock().unwrap().take()
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let script = self
            .scripts
            .iter()
            .find(|(prefix, _)| text.starts_with(prefix))
            .map(|(_, script)| script.clone())
            .expect("no script for text");
        if script.open_wait {
            *self.pending_wait.lock().unwrap() = Some(TtsOpenWait {
                wait_ms: 10,
                attempts: 2,
                reason: "Unavailable loading".into(),
            });
        }
        let mut all = Vec::new();
        for step in &script.steps {
            match step {
                Step::Pause(ms) => tokio::time::sleep(Duration::from_millis(*ms)).await,
                Step::Chunk(ms) => {
                    let chunk = sine_chunk(*ms);
                    if let Some(sink) = &sink {
                        let _ = sink.send(chunk.clone());
                    }
                    all.push(chunk);
                }
            }
        }
        if script.fail {
            return Err(SpeechError::Vendor {
                vendor: "mock-script".into(),
                message: "boom".into(),
            });
        }
        Ok(all)
    }
}

struct ScriptFactory {
    scripts: Arc<Vec<(&'static str, Script)>>,
}

impl VendorFactory for ScriptFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(ScriptTts {
            scripts: Arc::clone(&self.scripts),
            pending_wait: Mutex::new(None),
        }))
    }
}

async fn make_agent(scripts: Vec<(&'static str, Script)>) -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_tts(
        TtsVendor::Mock,
        Arc::new(ScriptFactory {
            scripts: Arc::new(scripts),
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
            sentence_case: Default::default(),
        }),
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent
}

/// Collects every event of an agent.
struct Recorder {
    events: Arc<Mutex<Vec<(Instant, SpeechEvent)>>>,
}

impl Recorder {
    fn new(agent: &VoiceAgent) -> Self {
        let mut rx = agent.subscribe_events();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                sink.lock().unwrap().push((Instant::now(), event));
            }
        });
        Self { events }
    }

    fn snapshot(&self) -> Vec<(Instant, SpeechEvent)> {
        self.events.lock().unwrap().clone()
    }

    /// Reply events as `(kind, reply id)` in emission order.
    fn reply_events(&self) -> Vec<SpeechEvent> {
        self.snapshot()
            .into_iter()
            .map(|(_, e)| e)
            .filter(|e| {
                matches!(
                    e.kind,
                    SpeechEventKind::AgentReplyStart | SpeechEventKind::AgentReplyEnd
                )
            })
            .collect()
    }

    async fn wait_until(&self, what: &str, cond: impl Fn(&[(Instant, SpeechEvent)]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if cond(&self.snapshot()) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; saw {:?}",
                self.snapshot()
                    .iter()
                    .map(|(_, e)| (e.kind, e.reply_id.clone()))
                    .collect::<Vec<_>>()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn wait_for_ends(&self, count: usize) {
        self.wait_until("agent_reply_end", |events| {
            events
                .iter()
                .filter(|(_, e)| e.kind == SpeechEventKind::AgentReplyEnd)
                .count()
                >= count
        })
        .await;
    }
}

fn tagged(reply_id: &str) -> SendTextToTtsOptions {
    SendTextToTtsOptions {
        non_blocking: true,
        reply_id: Some(reply_id.to_string()),
        ..Default::default()
    }
}

fn shape(event: &SpeechEvent) -> (SpeechEventKind, String) {
    (event.kind, event.reply_id.clone().unwrap_or_default())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_queued_replies_each_get_start_and_end() {
    let agent = make_agent(vec![
        ("alpha", Script::chunks(vec![Step::Chunk(200)])),
        ("bravo", Script::chunks(vec![Step::Chunk(200)])),
    ])
    .await;
    let rec = Recorder::new(&agent);

    agent
        .send_text_to_tts_with_options("alpha one", tagged("a"))
        .await
        .unwrap();
    agent
        .send_text_to_tts_with_options("bravo one", tagged("b"))
        .await
        .unwrap();
    rec.wait_for_ends(2).await;
    agent.wait_tts_playback_idle().await.unwrap();

    let events = rec.reply_events();
    let order: Vec<_> = events.iter().map(shape).collect();
    assert_eq!(
        order,
        vec![
            (SpeechEventKind::AgentReplyStart, "a".to_string()),
            (SpeechEventKind::AgentReplyEnd, "a".to_string()),
            (SpeechEventKind::AgentReplyStart, "b".to_string()),
            (SpeechEventKind::AgentReplyEnd, "b".to_string()),
        ]
    );
    for end in events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::AgentReplyEnd)
    {
        let played = end.played_ms.expect("played_ms");
        assert!((160..=240).contains(&played), "played_ms={played}");
        assert_eq!(end.interrupted, Some(false));
    }
    for start in events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::AgentReplyStart)
    {
        assert!(start.first_chunk_ms.is_some());
    }
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn barge_in_mid_reply_ends_current_and_queued_as_interrupted() {
    let agent = make_agent(vec![
        ("alpha", Script::chunks(vec![Step::Chunk(2000)])),
        ("bravo", Script::chunks(vec![Step::Chunk(2000)])),
    ])
    .await;
    let rec = Recorder::new(&agent);

    agent
        .send_text_to_tts_with_options("alpha two", tagged("a"))
        .await
        .unwrap();
    agent
        .send_text_to_tts_with_options("bravo two", tagged("b"))
        .await
        .unwrap();
    rec.wait_until("reply a started", |events| {
        events
            .iter()
            .any(|(_, e)| e.kind == SpeechEventKind::AgentReplyStart)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    agent.flush_tts().await.unwrap();
    rec.wait_for_ends(2).await;
    // Nothing else may follow for a or b.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let events = rec.reply_events();
    let order: Vec<_> = events.iter().map(shape).collect();
    assert_eq!(
        order,
        vec![
            (SpeechEventKind::AgentReplyStart, "a".to_string()),
            (SpeechEventKind::AgentReplyEnd, "a".to_string()),
            (SpeechEventKind::AgentReplyEnd, "b".to_string()),
        ]
    );
    let end_a = &events[1];
    let played_a = end_a.played_ms.expect("played_ms a");
    assert!((150..=400).contains(&played_a), "played_ms a={played_a}");
    assert_eq!(end_a.interrupted, Some(true));
    let end_b = &events[2];
    assert_eq!(end_b.played_ms, Some(0));
    assert_eq!(end_b.interrupted, Some(true));
    agent.stop().await.unwrap();
}

/// A reply flushed while its audio is still being synthesized never reaches the buffer, so no
/// start is emitted, but it still ends once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flush_while_synthesizing_ends_the_reply_with_nothing_played() {
    let agent = make_agent(vec![(
        "slow",
        Script::chunks(vec![Step::Pause(400), Step::Chunk(200)]),
    )])
    .await;
    let rec = Recorder::new(&agent);

    agent
        .send_text_to_tts_with_options("slow reply", tagged("s"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    agent.flush_tts().await.unwrap();
    rec.wait_for_ends(1).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let events = rec.reply_events();
    let order: Vec<_> = events.iter().map(shape).collect();
    assert_eq!(
        order,
        vec![(SpeechEventKind::AgentReplyEnd, "s".to_string())]
    );
    assert_eq!(events[0].played_ms, Some(0));
    assert_eq!(events[0].interrupted, Some(true));
    agent.stop().await.unwrap();
}

/// The vendor pauses 300 ms between two chunks: the buffer runs dry, but the reply is not over.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn progressive_underrun_does_not_end_early() {
    let agent = make_agent(vec![(
        "gappy",
        Script::chunks(vec![Step::Chunk(200), Step::Pause(300), Step::Chunk(200)]),
    )])
    .await;
    let rec = Recorder::new(&agent);

    agent
        .send_text_to_tts_with_options("gappy reply", tagged("g"))
        .await
        .unwrap();
    rec.wait_for_ends(1).await;
    agent.wait_tts_playback_idle().await.unwrap();

    let all = rec.snapshot();
    let start_at = all
        .iter()
        .find(|(_, e)| e.kind == SpeechEventKind::AgentReplyStart)
        .map(|(at, _)| *at)
        .expect("start");
    let (end_at, end) = all
        .iter()
        .find(|(_, e)| e.kind == SpeechEventKind::AgentReplyEnd)
        .expect("end");
    let events = rec.reply_events();
    assert_eq!(events.len(), 2, "one start and one end: {events:?}");
    let played = end.played_ms.expect("played_ms");
    assert!((360..=440).contains(&played), "played_ms={played}");
    assert_eq!(end.interrupted, Some(false));
    // First chunk plays 200 ms, the vendor pauses 300 ms, the second plays 200 ms.
    let elapsed = end_at.duration_since(start_at);
    assert!(
        elapsed >= Duration::from_millis(450),
        "reply ended after only {elapsed:?}"
    );
    agent.stop().await.unwrap();
}

/// The agent wraps the vendor in the process-wide phrase cache, so the same text twice gives a
/// real cache hit on the second reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_hit_and_tts_wait_carry_reply_id() {
    let mut script = Script::chunks(vec![Step::Chunk(100)]);
    script.open_wait = true;
    let agent = make_agent(vec![("cached", script)]).await;
    let rec = Recorder::new(&agent);
    let text = "cached phrase for the reply id test";

    agent
        .send_text_to_tts_with_options(text, tagged("c1"))
        .await
        .unwrap();
    rec.wait_for_ends(1).await;
    agent.wait_tts_playback_idle().await.unwrap();
    agent
        .send_text_to_tts_with_options(text, tagged("c2"))
        .await
        .unwrap();
    rec.wait_for_ends(2).await;
    agent.wait_tts_playback_idle().await.unwrap();

    let all: Vec<_> = rec.snapshot().into_iter().map(|(_, e)| e).collect();
    let hits: Vec<_> = all
        .iter()
        .filter(|e| e.kind == SpeechEventKind::TtsPhraseCacheHit)
        .collect();
    assert_eq!(hits.len(), 1, "only the second reply is a cache hit");
    assert_eq!(hits[0].reply_id.as_deref(), Some("c2"));
    assert_eq!(hits[0].chars, Some(text.chars().count() as u32));
    let waits: Vec<_> = all
        .iter()
        .filter(|e| e.kind == SpeechEventKind::TtsWait)
        .collect();
    assert!(!waits.is_empty());
    assert_eq!(waits[0].reply_id.as_deref(), Some("c1"));
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_synthesis_carries_reply_id_and_has_no_reply_end() {
    let mut script = Script::chunks(vec![]);
    script.fail = true;
    let agent = make_agent(vec![("broken", script)]).await;
    let rec = Recorder::new(&agent);

    agent
        .send_text_to_tts_with_options("broken reply", tagged("f1"))
        .await
        .unwrap();
    rec.wait_until("agent_speak_failed", |events| {
        events
            .iter()
            .any(|(_, e)| e.kind == SpeechEventKind::AgentSpeakFailed)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let all: Vec<_> = rec.snapshot().into_iter().map(|(_, e)| e).collect();
    let failed = all
        .iter()
        .find(|e| e.kind == SpeechEventKind::AgentSpeakFailed)
        .unwrap();
    assert_eq!(failed.reply_id.as_deref(), Some("f1"));
    assert!(
        rec.reply_events().is_empty(),
        "no audio, so no reply events"
    );
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untagged_reply_emits_no_reply_events() {
    let agent = make_agent(vec![("plain", Script::chunks(vec![Step::Chunk(200)]))]).await;
    let rec = Recorder::new(&agent);

    agent.send_text_to_tts("plain reply").await.unwrap();
    agent.wait_tts_playback_idle().await.unwrap();
    rec.wait_until("agent_speaking_end", |events| {
        events
            .iter()
            .any(|(_, e)| e.kind == SpeechEventKind::AgentSpeakingEnd)
    })
    .await;
    agent.flush_tts().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(rec.reply_events().is_empty());
    assert!(rec.snapshot().iter().all(|(_, e)| e.reply_id.is_none()));
    agent.stop().await.unwrap();
}

/// `stop()` ends the reply that is playing and the ones still queued, once each.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_ends_playing_and_queued_replies_once() {
    let agent = make_agent(vec![
        ("alpha", Script::chunks(vec![Step::Chunk(2000)])),
        ("bravo", Script::chunks(vec![Step::Chunk(2000)])),
    ])
    .await;
    let rec = Recorder::new(&agent);

    agent
        .send_text_to_tts_with_options("alpha three", tagged("a"))
        .await
        .unwrap();
    agent
        .send_text_to_tts_with_options("bravo three", tagged("b"))
        .await
        .unwrap();
    rec.wait_until("reply a started", |events| {
        events
            .iter()
            .any(|(_, e)| e.kind == SpeechEventKind::AgentReplyStart)
    })
    .await;
    let _ = agent.stop().await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let order: Vec<_> = rec.reply_events().iter().map(shape).collect();
    assert_eq!(
        order,
        vec![
            (SpeechEventKind::AgentReplyStart, "a".to_string()),
            (SpeechEventKind::AgentReplyEnd, "a".to_string()),
            (SpeechEventKind::AgentReplyEnd, "b".to_string()),
        ]
    );
}
