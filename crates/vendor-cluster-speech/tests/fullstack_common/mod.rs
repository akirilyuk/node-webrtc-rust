//! Shared full-stack harness: VoiceAgent + cluster-sherpa vendor + in-process mock speech server
//! over real gRPC.
//!
//! The test body is the inbound task: it feeds one frame at a time, paced from an anchor like the
//! native inbound loop (frame n due at `anchor + n * 20 ms`, sequential awaits, never two
//! concurrent `process_inbound_pcm` calls). Every speech event is tagged with the number of
//! frames fed when the collector received it, so a test can assert "this event arrived within N
//! frames of the audio that caused it".
//!
//! Per-utterance stream mode is read once per process from the environment, so each test file
//! that includes this module is its own binary.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_cluster_speech::ClusterSherpaFactory;
use tokio::task::JoinHandle;
use tokio::time::{sleep, sleep_until, Instant as TokioInstant};

use crate::mock_server::{spawn_mock_speech, MockSpeechState};

pub const FRAME_MS: u64 = 20;
/// Longest a single inbound call may run before the harness gives up and reports it as hung.
const HUNG_CALL_BOUND: Duration = Duration::from_secs(20);

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

pub fn set_per_utterance_env() {
    // `SPEECH_STT_OPEN_WAIT_MAX_MS` must stay unset: the 120 s default is part of the scenario.
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var("CLUSTER_STT_STREAM_PER_UTTERANCE", "1");
        std::env::remove_var("SPEECH_STT_OPEN_WAIT_MAX_MS");
    }
}

/// VAD settings shared by the full-stack tests (barge-in off; a test may turn it on).
pub fn default_vad() -> VadConfig {
    let mut vad = VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 300,
        speech_pad_ms: 1500,
        gate_stt: true,
        stt_gate_hold_ms: 1500,
        ..VadConfig::default()
    };
    vad.barge_in.enabled = false;
    vad
}

/// Silence after a speech end that is long enough for the gate hold to end plus 1 s.
pub fn silence_past_gate_hold_ms(vad: &VadConfig) -> u64 {
    u64::from(vad.min_silence_duration_ms) + u64::from(vad.stt_gate_hold_ms) + 1_000
}

/// One speech event with the harness state at the time the collector received it.
#[derive(Debug, Clone)]
pub struct TaggedEvent {
    /// Frames fed (`process_inbound_pcm` calls started) when the event was received.
    pub frame: u64,
    pub at_ms: u128,
    pub kind: SpeechEventKind,
    pub text: Option<String>,
    pub utterance_id: Option<String>,
    /// `PcmWriter` calls made when the event was received.
    pub writer_calls: u64,
}

pub struct Harness {
    pub agent: Arc<VoiceAgent>,
    pub state: MockSpeechState,
    events: Arc<Mutex<Vec<TaggedEvent>>>,
    frames_fed: Arc<AtomicU64>,
    writer_calls: Arc<AtomicU64>,
    anchor: Instant,
    tokio_anchor: TokioInstant,
    next_frame: u64,
    call_ms: Vec<u128>,
    loud: Bytes,
    silent: Bytes,
    collector: JoinHandle<()>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Harness {
    /// Start the mock speech server and a started VoiceAgent wired to it.
    pub async fn start(state: MockSpeechState, vad: VadConfig) -> Self {
        set_per_utterance_env();
        let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();

        let mut registry = VendorRegistry::new();
        let factory = Arc::new(ClusterSherpaFactory);
        registry.register_stt(SttVendor::ClusterSherpa, factory.clone());
        registry.register_tts(TtsVendor::ClusterSherpa, factory);

        let config = VoiceAgentConfig {
            stt: Some(node_webrtc_rust_speech::SttConfig {
                provider: SttVendor::ClusterSherpa,
                model: None,
                model_path: Some("/models/sherpa/stt/en".into()),
                language: Some("en".into()),
                api_key: Some("test-token".into()),
                endpoint: Some(url.clone()),
            }),
            tts: Some(TtsConfig {
                provider: TtsVendor::ClusterSherpa,
                model: None,
                model_path: Some("/models/sherpa/tts/en".into()),
                voice: Some("amy".into()),
                api_key: Some("test-token".into()),
                endpoint: Some(url.clone()),
                sentence_case: Default::default(),
            }),
            vad,
            ..Default::default()
        };

        let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
        let writer_calls = Arc::new(AtomicU64::new(0));
        let writer_counter = Arc::clone(&writer_calls);
        let writer: PcmWriter = Arc::new(move |_pcm, _ms| {
            writer_counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
        agent.start(None).await.unwrap();
        sleep(Duration::from_millis(80)).await;

        let anchor = Instant::now();
        let events: Arc<Mutex<Vec<TaggedEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let frames_fed = Arc::new(AtomicU64::new(0));

        let mut rx = agent.subscribe_events();
        let collector_events = Arc::clone(&events);
        let collector_frames = Arc::clone(&frames_fed);
        let collector_writes = Arc::clone(&writer_calls);
        let collector = tokio::spawn(async move {
            loop {
                let event = match rx.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                collector_events.lock().unwrap().push(TaggedEvent {
                    frame: collector_frames.load(Ordering::SeqCst),
                    at_ms: anchor.elapsed().as_millis(),
                    kind: event.kind,
                    text: event.text.clone(),
                    utterance_id: event.utterance_id.clone(),
                    writer_calls: collector_writes.load(Ordering::SeqCst),
                });
            }
        });

        Self {
            agent,
            state,
            events,
            frames_fed,
            writer_calls,
            anchor,
            tokio_anchor: TokioInstant::from_std(anchor),
            next_frame: 0,
            call_ms: Vec::new(),
            loud: Bytes::from(loud_stereo_frame()),
            silent: Bytes::from(silent_stereo_frame()),
            collector,
            shutdown: Some(shutdown),
        }
    }

    /// Frames fed so far = index of the next frame to feed.
    pub fn frame_index(&self) -> u64 {
        self.next_frame
    }

    /// Milliseconds since the anchor.
    pub fn elapsed_ms(&self) -> u128 {
        self.anchor.elapsed().as_millis()
    }

    pub fn writer_calls(&self) -> u64 {
        self.writer_calls.load(Ordering::SeqCst)
    }

    /// Feed one 20 ms frame, due at `anchor + n * 20 ms`.
    pub async fn feed(&mut self, loud: bool) {
        let n = self.next_frame;
        sleep_until(self.tokio_anchor + Duration::from_millis(n * FRAME_MS)).await;
        self.frames_fed.fetch_add(1, Ordering::SeqCst);
        let frame = if loud {
            self.loud.clone()
        } else {
            self.silent.clone()
        };
        let started = Instant::now();
        let call = tokio::time::timeout(
            HUNG_CALL_BOUND,
            self.agent.process_inbound_pcm(frame, FRAME_MS as u32),
        )
        .await;
        let took = started.elapsed().as_millis();
        let Ok(result) = call else {
            panic!(
                "process_inbound_pcm did not return within {} ms (frame {n})\n{}",
                HUNG_CALL_BOUND.as_millis(),
                self.timeline()
            );
        };
        result.unwrap();
        self.call_ms.push(took);
        self.next_frame += 1;
    }

    /// Feed `ms` of audio (loud or silent) as 20 ms frames.
    pub async fn feed_ms(&mut self, loud: bool, ms: u64) {
        for _ in 0..ms / FRAME_MS {
            self.feed(loud).await;
        }
    }

    /// Snapshot of every event received so far, in arrival order.
    pub fn events(&self) -> Vec<TaggedEvent> {
        self.events.lock().unwrap().clone()
    }

    /// Longest `process_inbound_pcm` call so far.
    pub fn max_call_ms(&self) -> u128 {
        self.call_ms.iter().copied().max().unwrap_or(0)
    }

    /// 99th percentile (nearest rank) of the call durations so far.
    pub fn p99_call_ms(&self) -> u128 {
        if self.call_ms.is_empty() {
            return 0;
        }
        let mut sorted = self.call_ms.clone();
        sorted.sort_unstable();
        let rank = (sorted.len() * 99).div_ceil(100);
        sorted[rank - 1]
    }

    pub fn call_count(&self) -> usize {
        self.call_ms.len()
    }

    /// Human-readable event timeline plus call stats, for assertion messages.
    pub fn timeline(&self) -> String {
        let mut out = String::new();
        for e in self.events() {
            out.push_str(&format!(
                "  frame={:>5} t={:>6}ms writes={:>4} {:?} id={:?} text={:?}\n",
                e.frame, e.at_ms, e.writer_calls, e.kind, e.utterance_id, e.text
            ));
        }
        out.push_str(&format!(
            "calls={} max_call_ms={} p99_call_ms={}\n",
            self.call_count(),
            self.max_call_ms(),
            self.p99_call_ms()
        ));
        out
    }

    /// Stop the agent (bounded) and the mock server. Returns the events and `(max, p99)` call ms.
    pub async fn finish(mut self) -> (Vec<TaggedEvent>, u128, u128) {
        sleep(Duration::from_millis(300)).await;
        self.collector.abort();
        if tokio::time::timeout(Duration::from_secs(5), self.agent.stop())
            .await
            .is_err()
        {
            eprintln!("agent.stop() did not return within 5 s");
        }
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        (self.events(), self.max_call_ms(), self.p99_call_ms())
    }
}

/// First event of `kind` whose tag is at least `min_frame`.
pub fn first_at_or_after(
    events: &[TaggedEvent],
    kind: SpeechEventKind,
    min_frame: u64,
) -> Option<&TaggedEvent> {
    events
        .iter()
        .find(|e| e.kind == kind && e.frame >= min_frame)
}

/// Position in `events` of the first event of `kind` with this utterance id.
pub fn position_of(
    events: &[TaggedEvent],
    kind: SpeechEventKind,
    utterance_id: &str,
) -> Option<usize> {
    events
        .iter()
        .position(|e| e.kind == kind && e.utterance_id.as_deref() == Some(utterance_id))
}

/// Ids of every `user_speaking_start`, in arrival order.
pub fn speaking_start_ids(events: &[TaggedEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSpeakingStart)
        .filter_map(|e| e.utterance_id.clone())
        .collect()
}

/// `true` for the two events that end an utterance.
pub fn is_terminal(kind: SpeechEventKind) -> bool {
    matches!(
        kind,
        SpeechEventKind::UserSpeechFinal | SpeechEventKind::UserSttNotFound
    )
}
