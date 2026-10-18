//! VoiceAgent + STT actor: utterance order, late results, holds, swaps and barge-in while an
//! earlier utterance still waits for its final.
//!
//! Every test drives a scripted *deferred* provider (`GatedStt`): its outputs only come out of
//! `poll_output` when the test releases them, so the order of events is chosen by the test, never
//! by timing. Synchronisation uses the provider's `Notify`, `wait_stt_idle` and explicit
//! releases; paused time is only used to move a finalize deadline.
//!
//! `wait_stt_idle` is a barrier that waits for every outstanding finalize. It is therefore only
//! called when no finalize is unanswered; while an earlier utterance is still unanswered the
//! tests wait on the provider counters instead.
//!
//! Order contract (utterance N+1 may start before utterance N ends): start events of N+1 can come
//! before the terminal of N, and terminals are emitted in utterance order.

use std::collections::VecDeque;
use std::future::pending;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{
    FinalizeMode, SttOutput, SttProvider, SttTranscript, TtsProvider, VendorFactory,
};
use node_webrtc_rust_speech::stt_hold::BeginSttHoldOptions;
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::sync::{broadcast, Notify};
use tokio::time::timeout;

// ---- scripted provider -----------------------------------------------------------------------

struct GatedState {
    /// Outputs the test released; `poll_output` hands them out in order.
    released: VecDeque<SttOutput>,
    pushed_bytes: u64,
    finalize_requests: usize,
    stops: usize,
    never_settle_push: bool,
    never_settle_finalize: bool,
    never_settle_poll: bool,
    open_pending: bool,
    finalize_wait_ms: u64,
}

/// Shared between the test and the provider the agent owns.
struct Gated {
    state: Mutex<GatedState>,
    /// A permit is stored when nobody waits, so a change is never lost.
    changed: Notify,
}

impl Gated {
    fn new(finalize_wait_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(GatedState {
                released: VecDeque::new(),
                pushed_bytes: 0,
                finalize_requests: 0,
                stops: 0,
                never_settle_push: false,
                never_settle_finalize: false,
                never_settle_poll: false,
                open_pending: false,
                finalize_wait_ms,
            }),
            changed: Notify::new(),
        })
    }

    fn with<R>(&self, f: impl FnOnce(&mut GatedState) -> R) -> R {
        let r = f(&mut self.state.lock().unwrap());
        self.changed.notify_one();
        r
    }

    /// Makes `outputs` available to the next `poll_output` calls.
    fn release(&self, outputs: Vec<SttOutput>) {
        self.with(|s| s.released.extend(outputs));
    }

    fn finalize_requests(&self) -> usize {
        self.state.lock().unwrap().finalize_requests
    }

    fn pushed_bytes(&self) -> u64 {
        self.state.lock().unwrap().pushed_bytes
    }

    async fn wait_until(&self, what: &str, pred: impl Fn(&GatedState) -> bool) {
        let waited = timeout(Duration::from_secs(20), async {
            loop {
                if pred(&self.state.lock().unwrap()) {
                    return;
                }
                self.changed.notified().await;
            }
        })
        .await;
        assert!(waited.is_ok(), "timed out waiting for {what}");
    }

    async fn wait_finalize_requests(&self, n: usize) {
        self.wait_until(&format!("{n} finalize request(s)"), |s| {
            s.finalize_requests >= n
        })
        .await;
    }

    /// Every released output was handed to the actor (its results are emitted before any command
    /// sent after this returns is handled).
    async fn wait_consumed(&self) {
        self.wait_until("released outputs to be polled", |s| s.released.is_empty())
            .await;
    }
}

struct GatedStt {
    shared: Arc<Gated>,
}

#[async_trait]
impl SttProvider for GatedStt {
    fn vendor_name(&self) -> &'static str {
        "gated-order"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        self.shared.with(|s| s.stops += 1);
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        if self.shared.state.lock().unwrap().never_settle_push {
            pending::<()>().await;
        }
        self.shared.with(|s| s.pushed_bytes += pcm.len() as u64);
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        Ok(None)
    }

    fn finalize_mode(&self) -> FinalizeMode {
        FinalizeMode::Deferred
    }

    async fn request_finalize(&mut self) -> SpeechResult<()> {
        if self.shared.state.lock().unwrap().never_settle_finalize {
            pending::<()>().await;
        }
        self.shared.with(|s| s.finalize_requests += 1);
        Ok(())
    }

    async fn poll_output(&mut self) -> SpeechResult<Option<SttOutput>> {
        if self.shared.state.lock().unwrap().never_settle_poll {
            pending::<()>().await;
        }
        Ok(self.shared.with(|s| s.released.pop_front()))
    }

    fn stream_open_pending(&self) -> bool {
        self.shared.state.lock().unwrap().open_pending
    }

    fn finalize_wait_ms(&self) -> u64 {
        self.shared.state.lock().unwrap().finalize_wait_ms
    }
}

/// Hands out one `GatedStt` per `create_stt`, in creation order.
struct GatedFactory {
    finalize_wait_ms: u64,
    created: Mutex<Vec<Arc<Gated>>>,
}

impl GatedFactory {
    fn new(finalize_wait_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            finalize_wait_ms,
            created: Mutex::new(Vec::new()),
        })
    }

    fn first(&self) -> Arc<Gated> {
        Arc::clone(self.created.lock().unwrap().first().expect("a provider"))
    }

    fn last(&self) -> Arc<Gated> {
        Arc::clone(self.created.lock().unwrap().last().expect("a provider"))
    }

    fn count(&self) -> usize {
        self.created.lock().unwrap().len()
    }
}

impl VendorFactory for GatedFactory {
    fn create_stt(&self, _config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        let shared = Gated::new(self.finalize_wait_ms);
        self.created.lock().unwrap().push(Arc::clone(&shared));
        Ok(Box::new(GatedStt { shared }))
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

fn final_out(text: &str) -> SttOutput {
    SttOutput::Transcript(SttTranscript::Final(text.into()))
}

fn partial_out(text: &str) -> SttOutput {
    SttOutput::Transcript(SttTranscript::Partial(text.into()))
}

// ---- agent setup -----------------------------------------------------------------------------

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

fn gate_vad() -> VadConfig {
    let mut vad = VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 40,
        gate_stt: true,
        stt_gate_hold_ms: 80,
        ..VadConfig::default()
    };
    vad.barge_in.enabled = false;
    vad
}

fn barge_vad(require_stt_partial: bool) -> VadConfig {
    let mut vad = gate_vad();
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = true;
    vad.barge_in.flush_tts = true;
    vad.barge_in.agent_playback_guard_ms = 0;
    vad.barge_in.require_stt_partial = require_stt_partial;
    vad
}

fn stt_config(language: &str) -> SttConfig {
    SttConfig {
        provider: SttVendor::Mock,
        model: None,
        model_path: None,
        language: Some(language.into()),
        api_key: None,
        endpoint: None,
        transport: None,
    }
}

async fn make_agent(
    factory: &Arc<GatedFactory>,
    vad: VadConfig,
    writer: Option<PcmWriter>,
) -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::clone(factory) as Arc<dyn VendorFactory>,
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
    let config = VoiceAgentConfig {
        stt: Some(stt_config("en")),
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
        vad,
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writer: PcmWriter = writer.unwrap_or_else(|| Arc::new(|_pcm, _ms| Ok(())));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent
}

/// Feeds `n` frames; every call must return promptly (the inbound path never waits for STT).
/// Does not wait for results, so it is safe while an earlier utterance is unanswered.
async fn feed(agent: &Arc<VoiceAgent>, frame: &[u8], n: usize) {
    feed_with_duration(agent, frame, n, 20).await;
}

async fn feed_with_duration(agent: &Arc<VoiceAgent>, frame: &[u8], n: usize, duration_ms: u32) {
    for _ in 0..n {
        timeout(
            Duration::from_secs(5),
            agent.process_inbound_pcm(Bytes::from(frame.to_vec()), duration_ms),
        )
        .await
        .expect("process_inbound_pcm did not return")
        .unwrap();
    }
}

/// Every STT command sent so far was handled and its results emitted. Only valid while no
/// finalize is unanswered.
async fn idle(agent: &Arc<VoiceAgent>) {
    assert!(
        agent.wait_stt_idle(Duration::from_secs(10)).await,
        "wait_stt_idle timed out (is a finalize still unanswered?)"
    );
}

/// Releases `outputs` and waits until the actor took them and the agent emitted their results.
async fn release_and_settle(agent: &Arc<VoiceAgent>, gated: &Gated, outputs: Vec<SttOutput>) {
    gated.release(outputs);
    gated.wait_consumed().await;
    idle(agent).await;
}

// ---- event log -------------------------------------------------------------------------------

struct Log {
    rx: broadcast::Receiver<SpeechEvent>,
    events: Vec<SpeechEvent>,
}

impl Log {
    fn new(agent: &Arc<VoiceAgent>) -> Self {
        Self {
            rx: agent.subscribe_events(),
            events: Vec::new(),
        }
    }

    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            self.events.push(event);
        }
    }

    async fn wait_for(&mut self, what: &str, pred: impl Fn(&[SpeechEvent]) -> bool) {
        loop {
            self.pump();
            if pred(&self.events) {
                return;
            }
            match timeout(Duration::from_secs(20), self.rx.recv()).await {
                Ok(Ok(event)) => self.events.push(event),
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
                _ => panic!("timed out waiting for {what}; events: {}", self.summary()),
            }
        }
    }

    /// One line per event: `kind id text reason`.
    fn summary(&mut self) -> String {
        self.pump();
        let lines: Vec<String> = self
            .events
            .iter()
            .enumerate()
            .map(|(i, e)| {
                format!(
                    "#{i} {:?} id={} text={} reason={}",
                    e.kind,
                    e.utterance_id.as_deref().unwrap_or("-"),
                    e.text.as_deref().unwrap_or("-"),
                    e.reason.as_deref().unwrap_or("-")
                )
            })
            .collect();
        format!("\n{}", lines.join("\n"))
    }

    fn count(&mut self, kind: SpeechEventKind) -> usize {
        self.pump();
        self.events.iter().filter(|e| e.kind == kind).count()
    }

    fn position(&mut self, pred: impl Fn(&SpeechEvent) -> bool) -> Option<usize> {
        self.pump();
        self.events.iter().position(pred)
    }

    /// The utterance id of the `n`-th `user_speaking_start` (0-based).
    fn start_id(&mut self, n: usize) -> String {
        self.pump();
        self.events
            .iter()
            .filter(|e| e.kind == SpeechEventKind::UserSpeakingStart)
            .nth(n)
            .and_then(|e| e.utterance_id.clone())
            .unwrap_or_else(|| panic!("no user_speaking_start #{n}: {:?}", self.events))
    }

    fn finals(&mut self) -> Vec<(String, String)> {
        self.pump();
        self.events
            .iter()
            .filter(|e| e.kind == SpeechEventKind::UserSpeechFinal)
            .map(|e| {
                (
                    e.text.clone().unwrap_or_default(),
                    e.utterance_id.clone().unwrap_or_default(),
                )
            })
            .collect()
    }

    /// Events that end an utterance: a final or a `user_stt_not_found`.
    fn terminals_of(&mut self, id: &str) -> Vec<SpeechEventKind> {
        self.pump();
        self.events
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    SpeechEventKind::UserSpeechFinal | SpeechEventKind::UserSttNotFound
                ) && e.utterance_id.as_deref() == Some(id)
            })
            .map(|e| e.kind)
            .collect()
    }

    fn index_of_final(&mut self, text: &str) -> usize {
        self.position(|e| {
            e.kind == SpeechEventKind::UserSpeechFinal && e.text.as_deref() == Some(text)
        })
        .unwrap_or_else(|| panic!("no final {text:?}: {:?}", self.events))
    }
}

/// Utterance A: speech, then silence until its close was handed to the provider.
async fn speak_and_close(agent: &Arc<VoiceAgent>, gated: &Gated, requests_after: usize) {
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    feed(agent, &loud, 15).await;
    feed(agent, &silent, 30).await;
    gated.wait_finalize_requests(requests_after).await;
}

// ---- tests -----------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_final_of_a_lands_on_a_after_b_started() {
    let factory = GatedFactory::new(60_000);
    let agent = make_agent(&factory, gate_vad(), None).await;
    let gated = factory.first();
    let mut log = Log::new(&agent);
    let loud = loud_stereo_frame();

    speak_and_close(&agent, &gated, 1).await;
    assert_eq!(gated.finalize_requests(), 1);

    // B starts while A still waits for its final.
    feed(&agent, &loud, 15).await;
    assert_eq!(
        log.count(SpeechEventKind::UserSpeakingStart),
        2,
        "B must start while A is unanswered: {}",
        log.summary()
    );
    let (a_id, b_id) = (log.start_id(0), log.start_id(1));
    assert_ne!(a_id, b_id);

    release_and_settle(
        &agent,
        &gated,
        vec![final_out("alpha"), SttOutput::FinalizeDone],
    )
    .await;

    assert_eq!(
        log.finals(),
        vec![("alpha".to_string(), a_id)],
        "the late final belongs to A, not to B (B id {b_id}): {}",
        log.summary()
    );
    assert!(
        log.terminals_of(&b_id).is_empty(),
        "B must not get a terminal from A's final: {}",
        log.summary()
    );
    agent.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn late_final_after_a_timed_out_is_ignored_and_counted() {
    let factory = GatedFactory::new(1_000);
    let agent = make_agent(&factory, gate_vad(), None).await;
    let gated = factory.first();
    let mut log = Log::new(&agent);
    let loud = loud_stereo_frame();

    speak_and_close(&agent, &gated, 1).await;
    let a_id = log.start_id(0);

    // The vendor does not answer in time: A ends with its fallback terminal.
    tokio::time::advance(Duration::from_millis(1_001)).await;
    idle(&agent).await;
    assert_eq!(
        log.terminals_of(&a_id),
        vec![SpeechEventKind::UserSttNotFound],
        "A ends without a transcript: {}",
        log.summary()
    );

    // B starts, then the vendor's answer for A finally shows up.
    feed(&agent, &loud, 15).await;
    let b_id = log.start_id(1);
    release_and_settle(
        &agent,
        &gated,
        vec![final_out("late"), SttOutput::FinalizeDone],
    )
    .await;

    log.pump();
    assert!(
        !log.events
            .iter()
            .any(|e| e.text.as_deref().is_some_and(|t| t.contains("late"))),
        "a late result of a terminal utterance must not surface: {}",
        log.summary()
    );
    assert_eq!(
        log.terminals_of(&a_id).len(),
        1,
        "A has exactly one terminal"
    );
    assert!(log.terminals_of(&b_id).is_empty());
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b_speaking_start_precedes_a_final_and_terminals_in_order() {
    let factory = GatedFactory::new(60_000);
    let agent = make_agent(&factory, gate_vad(), None).await;
    let gated = factory.first();
    let mut log = Log::new(&agent);
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    speak_and_close(&agent, &gated, 1).await;
    // B starts; its partial is not released.
    feed(&agent, &loud, 15).await;
    release_and_settle(
        &agent,
        &gated,
        vec![final_out("alpha"), SttOutput::FinalizeDone],
    )
    .await;

    // B closes and gets its own answer.
    feed(&agent, &silent, 30).await;
    gated.wait_finalize_requests(2).await;
    release_and_settle(
        &agent,
        &gated,
        vec![final_out("bravo"), SttOutput::FinalizeDone],
    )
    .await;

    let (a_id, b_id) = (log.start_id(0), log.start_id(1));
    let b_start = log
        .position(|e| {
            e.kind == SpeechEventKind::UserSpeakingStart
                && e.utterance_id.as_deref() == Some(b_id.as_str())
        })
        .unwrap();
    let a_final = log.index_of_final("alpha");
    let b_final = log.index_of_final("bravo");
    assert!(
        b_start < a_final && a_final < b_final,
        "order must be B start ({b_start}) < A final ({a_final}) < B final ({b_final}): {}",
        log.summary()
    );
    assert_eq!(
        log.finals(),
        vec![("alpha".to_string(), a_id), ("bravo".to_string(), b_id)]
    );
    agent.stop().await.unwrap();
}

/// C1 compares media time and wall time (`std::time::Instant`), so this test runs in real time
/// with a short listen timeout. The two waits below only let the wall clock reach that timeout;
/// they do not order anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c1_of_b_held_while_a_closing() {
    const LISTEN_MS: u32 = 200;
    let factory = GatedFactory::new(60_000);
    let mut vad = gate_vad();
    vad.stt_listen_timeout_ms = LISTEN_MS;
    let agent = make_agent(&factory, vad, None).await;
    let gated = factory.first();
    let mut log = Log::new(&agent);
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    let wall_wait = || tokio::time::sleep(Duration::from_millis(u64::from(LISTEN_MS) + 100));

    speak_and_close(&agent, &gated, 1).await;
    let a_id = log.start_id(0);

    // B speaks for longer than its listen window (media and wall) with no partial.
    feed(&agent, &loud, 5).await;
    wall_wait().await;
    feed_with_duration(&agent, &loud, 1, LISTEN_MS + 100).await;
    wall_wait().await;
    feed_with_duration(&agent, &loud, 1, LISTEN_MS + 100).await;
    let b_id = log.start_id(1);
    assert_eq!(
        log.count(SpeechEventKind::UserSttNotFound),
        0,
        "B's listen window must not run while A is unanswered: {}",
        log.summary()
    );

    // No `wait_stt_idle` here: once A is answered, B's listen window may fire C1 from the
    // wall-clock ticker at any moment and send B's boundary finalize. A barrier queued behind
    // that unanswered finalize would wait for it (it timed out under CPU load). Wait for A's
    // final event instead.
    gated.release(vec![final_out("alpha"), SttOutput::FinalizeDone]);
    gated.wait_consumed().await;
    log.wait_for("A's user_speech_final", |events| {
        events
            .iter()
            .any(|e| e.kind == SpeechEventKind::UserSpeechFinal)
    })
    .await;
    assert_eq!(log.finals(), vec![("alpha".to_string(), a_id)]);

    // Now B's listen window runs normally.
    wall_wait().await;
    feed_with_duration(&agent, &loud, 1, LISTEN_MS + 100).await;
    log.wait_for("B's user_stt_not_found", |events| {
        events.iter().any(|e| {
            e.kind == SpeechEventKind::UserSttNotFound
                && e.utterance_id.as_deref() == Some(b_id.as_str())
        })
    })
    .await;

    // The rest of B's close is answered by the vendor; B still has exactly one terminal.
    feed(&agent, &silent, 30).await;
    // Three finalize requests in total: A, B's C1 boundary (B is still speaking, so B stays
    // live) and B's gate close in the 30 silent frames (media time, so always reached). Wait
    // for all three before answering: reading a count the actor is still raising under CPU
    // load left the last request unanswered.
    gated.wait_finalize_requests(3).await;
    // A's marker was released above; answer B's two requests.
    release_and_settle(
        &agent,
        &gated,
        vec![SttOutput::FinalizeDone, SttOutput::FinalizeDone],
    )
    .await;
    assert_eq!(gated.finalize_requests(), 3, "{}", log.summary());
    assert_eq!(
        log.terminals_of(&b_id),
        vec![SpeechEventKind::UserSttNotFound],
        "B ends with exactly one terminal: {}",
        log.summary()
    );
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hold_begun_while_a_closing_ignores_old_model_final() {
    let factory = GatedFactory::new(60_000);
    let agent = make_agent(&factory, gate_vad(), None).await;
    let gated = factory.first();
    let mut log = Log::new(&agent);
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    speak_and_close(&agent, &gated, 1).await;
    agent
        .begin_stt_hold(BeginSttHoldOptions::default())
        .await
        .unwrap();

    release_and_settle(
        &agent,
        &gated,
        vec![final_out("old"), SttOutput::FinalizeDone],
    )
    .await;
    log.pump();
    assert!(
        !log.events.iter().any(|e| e.text.as_deref() == Some("old")),
        "the old model's final must be ignored during a hold: {}",
        log.summary()
    );

    // The hold is cancelled and the agent keeps working.
    agent.cancel_stt_hold().await.unwrap();
    feed(&agent, &loud, 15).await;
    feed(&agent, &silent, 30).await;
    gated.wait_finalize_requests(2).await;
    release_and_settle(
        &agent,
        &gated,
        vec![final_out("bravo"), SttOutput::FinalizeDone],
    )
    .await;
    let b_id = log.start_id(1);
    assert_eq!(
        log.finals(),
        vec![("bravo".to_string(), b_id)],
        "{}",
        log.summary()
    );
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swap_while_a_closing_applies_after_a_final() {
    let factory = GatedFactory::new(60_000);
    let agent = make_agent(&factory, gate_vad(), None).await;
    let old = factory.first();
    let mut log = Log::new(&agent);
    let loud = loud_stereo_frame();

    speak_and_close(&agent, &old, 1).await;

    // The swap waits for A's finalize.
    let swapping = {
        let agent = Arc::clone(&agent);
        tokio::spawn(async move { agent.update_stt_config(stt_config("de")).await })
    };
    old.release(vec![final_out("alpha"), SttOutput::FinalizeDone]);
    timeout(Duration::from_secs(10), swapping)
        .await
        .expect("update_stt_config did not finish")
        .unwrap()
        .unwrap();
    idle(&agent).await;

    let a_id = log.start_id(0);
    let final_at = log.index_of_final("alpha");
    let updated_at = log
        .position(|e| e.kind == SpeechEventKind::SttConfigUpdated)
        .expect("stt_config_updated");
    assert!(
        final_at < updated_at,
        "the swap applies after A's final: {}",
        log.summary()
    );
    assert_eq!(log.finals(), vec![("alpha".to_string(), a_id)]);

    // The next utterance goes to the new provider only.
    assert_eq!(factory.count(), 2, "one provider per create_stt");
    let new = factory.last();
    let old_bytes = old.pushed_bytes();
    assert_eq!(new.pushed_bytes(), 0);
    feed(&agent, &loud, 15).await;
    idle(&agent).await;
    assert!(
        new.pushed_bytes() > 0,
        "the next utterance's audio must reach the new provider"
    );
    assert_eq!(
        old.pushed_bytes(),
        old_bytes,
        "the old provider must not get audio after the swap"
    );
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provider_that_never_settles_cannot_block_inbound() {
    let factory = GatedFactory::new(60_000);
    let agent = make_agent(&factory, gate_vad(), None).await;
    let gated = factory.first();
    gated.with(|s| {
        s.never_settle_push = true;
        s.never_settle_finalize = true;
        s.never_settle_poll = true;
    });
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    let mut max_call_ms = 0_u128;
    for i in 0..500 {
        let frame = match i {
            0..=149 | 350..=499 => &loud,
            _ => &silent,
        };
        let started = Instant::now();
        timeout(
            Duration::from_secs(1),
            agent.process_inbound_pcm(Bytes::from(frame.clone()), 20),
        )
        .await
        .unwrap_or_else(|_| panic!("process_inbound_pcm call {i} waited for the stuck provider"))
        .unwrap();
        max_call_ms = max_call_ms.max(started.elapsed().as_millis());
    }
    assert!(
        max_call_ms < 1_000,
        "slowest process_inbound_pcm call took {max_call_ms} ms"
    );
    println!("max process_inbound_pcm call: {max_call_ms} ms over 500 frames");

    timeout(Duration::from_secs(10), agent.stop())
        .await
        .expect("agent.stop() did not return within 10 s with a stuck provider")
        .unwrap();
}

/// Writer that counts its calls and takes a little time per frame, so a long reply is still
/// playing when the test interrupts it.
fn counting_writer(calls: &Arc<AtomicUsize>) -> PcmWriter {
    let calls = Arc::clone(calls);
    Arc::new(move |_pcm, _ms| {
        calls.fetch_add(1, Ordering::SeqCst);
        tokio::task::block_in_place(|| std::thread::sleep(Duration::from_millis(8)));
        Ok(())
    })
}

fn long_reply() -> String {
    "interrupt this mock playback please ".repeat(8)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vad_barge_in_fires_while_a_final_pending() {
    let factory = GatedFactory::new(60_000);
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = make_agent(&factory, barge_vad(false), Some(counting_writer(&calls))).await;
    let gated = factory.first();
    let mut log = Log::new(&agent);
    let loud = loud_stereo_frame();

    speak_and_close(&agent, &gated, 1).await;
    let a_id = log.start_id(0);

    // The agent starts a long reply while A is unanswered.
    let speaking = {
        let agent = Arc::clone(&agent);
        let text = long_reply();
        tokio::spawn(async move { agent.send_text_to_tts(&text).await })
    };
    log.wait_for("agent_speaking_start", |events| {
        events
            .iter()
            .any(|e| e.kind == SpeechEventKind::AgentSpeakingStart)
    })
    .await;
    assert_eq!(log.count(SpeechEventKind::BargeIn), 0);
    let vad_before = log.count(SpeechEventKind::VadTriggered);

    // B speaks: the barge-in fires in the call that processes B's SpeechStart.
    let mut barge_call = None;
    let mut vad_call = None;
    for call in 0..20 {
        feed(&agent, &loud, 1).await;
        if vad_call.is_none() && log.count(SpeechEventKind::VadTriggered) > vad_before {
            vad_call = Some(call);
        }
        if barge_call.is_none() && log.count(SpeechEventKind::BargeIn) > 0 {
            barge_call = Some(call);
        }
        if vad_call.is_some() {
            break;
        }
    }
    assert!(vad_call.is_some(), "B's SpeechStart: {}", log.summary());
    assert_eq!(
        barge_call,
        vad_call,
        "barge_in must be emitted by the call that handles B's SpeechStart: {}",
        log.summary()
    );
    let writes_at_barge = calls.load(Ordering::SeqCst);

    log.wait_for("agent_speaking_end", |events| {
        events
            .iter()
            .any(|e| e.kind == SpeechEventKind::AgentSpeakingEnd)
    })
    .await;
    timeout(Duration::from_secs(10), speaking)
        .await
        .expect("send_text_to_tts did not return")
        .unwrap()
        .unwrap();
    let writes_after = calls.load(Ordering::SeqCst);
    assert!(
        writes_after <= writes_at_barge + 2,
        "playback must stop at the barge-in: {writes_at_barge} writes at barge, {writes_after} after"
    );

    // A's answer still lands on A.
    release_and_settle(
        &agent,
        &gated,
        vec![final_out("alpha"), SttOutput::FinalizeDone],
    )
    .await;
    assert_eq!(log.finals(), vec![("alpha".to_string(), a_id)]);
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stt_gated_barge_in_fires_on_b_partial_after_a_final() {
    let factory = GatedFactory::new(60_000);
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = make_agent(&factory, barge_vad(true), Some(counting_writer(&calls))).await;
    let gated = factory.first();
    let mut log = Log::new(&agent);
    let loud = loud_stereo_frame();

    speak_and_close(&agent, &gated, 1).await;
    let a_id = log.start_id(0);

    let speaking = {
        let agent = Arc::clone(&agent);
        let text = long_reply();
        tokio::spawn(async move { agent.send_text_to_tts(&text).await })
    };
    log.wait_for("agent_speaking_start", |events| {
        events
            .iter()
            .any(|e| e.kind == SpeechEventKind::AgentSpeakingStart)
    })
    .await;

    // B speaks while A is unanswered: no partial of B yet, so no barge-in.
    feed(&agent, &loud, 15).await;
    let b_id = log.start_id(1);
    assert_eq!(
        log.count(SpeechEventKind::BargeIn),
        0,
        "no barge-in without a partial of B: {}",
        log.summary()
    );

    // A's answer arrives; it is A's and does not interrupt the reply. (One word: below the
    // default `min_stt_partial_tokens` of 2, so it does not qualify for a barge-in by itself.)
    release_and_settle(
        &agent,
        &gated,
        vec![final_out("alpha"), SttOutput::FinalizeDone],
    )
    .await;
    assert_eq!(log.finals(), vec![("alpha".to_string(), a_id)]);
    assert_eq!(
        log.count(SpeechEventKind::BargeIn),
        0,
        "A's final must not interrupt the reply: {}",
        log.summary()
    );

    // B's first partial (keyless, two words: qualifies) is applied by the result pump alone: no
    // frame is fed.
    release_and_settle(&agent, &gated, vec![partial_out("bravo charlie")]).await;
    let partial_at = log
        .position(|e| {
            e.kind == SpeechEventKind::UserSpeechPartial
                && e.text.as_deref() == Some("bravo charlie")
                && e.utterance_id.as_deref() == Some(b_id.as_str())
        })
        .unwrap_or_else(|| panic!("B's partial: {}", log.summary()));
    let barge_at = log
        .position(|e| e.kind == SpeechEventKind::BargeIn)
        .unwrap_or_else(|| panic!("barge_in after B's partial: {}", log.summary()));
    assert!(
        partial_at < barge_at,
        "barge_in follows B's partial: {}",
        log.summary()
    );

    timeout(Duration::from_secs(10), speaking)
        .await
        .expect("send_text_to_tts did not return")
        .unwrap()
        .unwrap();
    agent.stop().await.unwrap();
}

/// Only the live utterance can interrupt the reply. A's words (three, so they qualify at the
/// default `min_stt_partial_tokens`) were spoken before the reply started; their late final must
/// not cut it off. B's first partial does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_utterance_final_never_triggers_stt_barge_in() {
    let factory = GatedFactory::new(60_000);
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = make_agent(&factory, barge_vad(true), Some(counting_writer(&calls))).await;
    let gated = factory.first();
    let mut log = Log::new(&agent);
    let loud = loud_stereo_frame();

    speak_and_close(&agent, &gated, 1).await;
    let a_id = log.start_id(0);

    let speaking = {
        let agent = Arc::clone(&agent);
        let text = long_reply();
        tokio::spawn(async move { agent.send_text_to_tts(&text).await })
    };
    log.wait_for("agent_speaking_start", |events| {
        events
            .iter()
            .any(|e| e.kind == SpeechEventKind::AgentSpeakingStart)
    })
    .await;

    feed(&agent, &loud, 15).await;
    let b_id = log.start_id(1);

    release_and_settle(
        &agent,
        &gated,
        vec![final_out("alpha bravo charlie"), SttOutput::FinalizeDone],
    )
    .await;
    assert_eq!(
        log.finals(),
        vec![("alpha bravo charlie".to_string(), a_id)],
        "{}",
        log.summary()
    );
    assert_eq!(
        log.count(SpeechEventKind::BargeIn),
        0,
        "a closing utterance's final must not interrupt the reply: {}",
        log.summary()
    );
    assert_eq!(
        log.count(SpeechEventKind::AgentSpeakingEnd),
        0,
        "the reply is still playing: {}",
        log.summary()
    );

    release_and_settle(&agent, &gated, vec![partial_out("delta echo")]).await;
    let partial_at = log
        .position(|e| {
            e.kind == SpeechEventKind::UserSpeechPartial
                && e.text.as_deref() == Some("delta echo")
                && e.utterance_id.as_deref() == Some(b_id.as_str())
        })
        .unwrap_or_else(|| panic!("B's partial: {}", log.summary()));
    let barge_at = log
        .position(|e| e.kind == SpeechEventKind::BargeIn)
        .unwrap_or_else(|| panic!("barge_in after B's partial: {}", log.summary()));
    assert!(
        partial_at < barge_at,
        "barge_in follows B's partial: {}",
        log.summary()
    );

    timeout(Duration::from_secs(10), speaking)
        .await
        .expect("send_text_to_tts did not return")
        .unwrap()
        .unwrap();
    agent.stop().await.unwrap();
}
