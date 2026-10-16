//! C1 pure-stall safety net must not close a stream while VAD is in speech.
//!
//! A frozen inbound reader (CPU-starved host event loop) stops calling `process_inbound_pcm`
//! while the audio waits in the track buffer. From the agent that looks exactly like a stopped
//! sender (`pcm_idle` >= `stt_listen_timeout_ms`), and the wall-clock net closed the open STT
//! stream as `user_stt_not_found` mid-phrase, losing the words after it. Only the hard timeout
//! may bound that case while VAD is speaking; after the speech ended the net still applies.
//!
//! The scripted STT is FINAL-ONLY (never queues a partial), so C1 stays armed for the whole
//! phrase. Frames are paced in REAL time because the agent measures with `std::time::Instant`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_speech::{PcmWriter, SpeechResult, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

const FRAME_MS: u64 = 20;
const MIN_SILENCE_MS: u32 = 300;
const HOLD_MS: u32 = 1000;

const ALPHA: i16 = 6000;
const BRAVO: i16 = 11000;
const SILENCE: i16 = 0;

/// Constant-amplitude 48 kHz stereo 20 ms frame.
fn stereo_frame(level: i16) -> Bytes {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&level.to_le_bytes());
        pcm.extend_from_slice(&level.to_le_bytes());
    }
    Bytes::from(pcm)
}

fn marker_for(block: &[i16]) -> Option<&'static str> {
    let max = block.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    if max >= 9000 {
        Some("bravo")
    } else if max >= 4000 {
        Some("alpha")
    } else {
        None
    }
}

/// Final-only STT: collapsed marker words, one `Final` per `finalize_utterance`, never a partial.
#[derive(Default)]
struct FinalOnlyStt {
    words: Vec<&'static str>,
    queue: VecDeque<SttTranscript>,
}

#[async_trait::async_trait]
impl SttProvider for FinalOnlyStt {
    fn vendor_name(&self) -> &'static str {
        "scripted-final-only"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let samples: Vec<i16> = pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect();
        for block in samples.chunks(320) {
            if let Some(word) = marker_for(block) {
                if self.words.last() != Some(&word) {
                    self.words.push(word);
                }
            }
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        Ok(self.queue.pop_front())
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        if !self.words.is_empty() {
            self.queue
                .push_back(SttTranscript::Final(self.words.join(" ")));
        }
        self.words.clear();
        Ok(())
    }

    fn discard_queued_transcripts(&mut self) -> usize {
        let n = self.queue.len();
        self.queue.clear();
        n
    }
}

struct FinalOnlyFactory;

impl VendorFactory for FinalOnlyFactory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(FinalOnlyStt::default()))
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

async fn make_agent() -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(FinalOnlyFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig {
        enabled: true,
        provider: "energy".into(),
        threshold: 0.05,
        min_speech_duration_ms: 200,
        min_silence_duration_ms: MIN_SILENCE_MS,
        speech_pad_ms: 200,
        gate_stt: true,
        stt_gate_hold_ms: HOLD_MS,
        stt_listen_timeout_ms: 4000,
        stt_listen_hard_timeout_ms: 0,
        ..Default::default()
    };
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
            sentence_case: Default::default(),
        }),
        vad,
        ..Default::default()
    };

    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent
}

type Events = Arc<Mutex<Vec<(u64, SpeechEvent)>>>;

/// Collects every event with its time since `start` until the agent is dropped.
fn collect(agent: &VoiceAgent, start: std::time::Instant) -> Events {
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let mut rx = agent.subscribe_events();
    let sink = events.clone();
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let at = start.elapsed().as_millis() as u64;
            sink.lock().unwrap().push((at, event));
        }
    });
    events
}

/// Feeds `frames` frames of `level` in real time, 20 ms apart, continuing the `next_due` clock.
async fn feed(agent: &VoiceAgent, next_due: &mut std::time::Instant, level: i16, frames: u64) {
    let frame = stereo_frame(level);
    for _ in 0..frames {
        tokio::time::sleep_until(tokio::time::Instant::from_std(*next_due)).await;
        agent.process_inbound_pcm(frame.clone(), 20).await.unwrap();
        *next_due += Duration::from_millis(FRAME_MS);
    }
}

/// Reader freeze: no frames for `ms`, then the clock restarts from now (the buffered audio
/// arrives afterwards at the normal pace).
async fn freeze(next_due: &mut std::time::Instant, ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
    *next_due = std::time::Instant::now();
}

fn timeline(events: &Events) -> Vec<(u64, SpeechEventKind, Option<String>)> {
    events
        .lock()
        .unwrap()
        .iter()
        .map(|(at, e)| (*at, e.kind, e.text.clone()))
        .collect()
}

fn count(events: &Events, kind: SpeechEventKind) -> usize {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, e)| e.kind == kind)
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reader_stall_mid_phrase_does_not_close_stream() {
    let agent = make_agent().await;
    let start = std::time::Instant::now();
    let events = collect(&agent, start);
    let mut due = start;
    feed(&agent, &mut due, SILENCE, 25).await;
    feed(&agent, &mut due, ALPHA, 29).await;
    freeze(&mut due, 4300).await;
    feed(&agent, &mut due, BRAVO, 40).await;
    feed(&agent, &mut due, SILENCE, 150).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let _ = agent.stop().await;

    let msg = format!("timeline: {:?}", timeline(&events));
    assert_eq!(count(&events, SpeechEventKind::UserSttNotFound), 0, "{msg}");
    let finals: Vec<String> = events
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, e)| e.kind == SpeechEventKind::UserSpeechFinal)
        .map(|(_, e)| e.text.clone().unwrap_or_default())
        .collect();
    assert_eq!(finals, vec!["alpha bravo".to_string()], "{msg}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reader_stall_after_speech_end_still_hits_c1() {
    let agent = make_agent().await;
    let start = std::time::Instant::now();
    let events = collect(&agent, start);
    let mut due = start;
    feed(&agent, &mut due, SILENCE, 25).await;
    feed(&agent, &mut due, ALPHA, 29).await;
    feed(&agent, &mut due, SILENCE, 20).await;
    freeze(&mut due, 4500).await;
    let _ = agent.stop().await;

    let msg = format!("timeline: {:?}", timeline(&events));
    assert_eq!(count(&events, SpeechEventKind::UserSttNotFound), 1, "{msg}");
}
