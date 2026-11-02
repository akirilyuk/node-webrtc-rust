//! `send_text_to_tts*` queues jobs in call order (ticket order), not in the order the send
//! futures happen to reach the queue.
//!
//! Earlier sends are held back before the queue by the pre-enqueue test hook (one gate per
//! ticket, released by the test in reverse order), so a later send always arrives first.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use node_webrtc_rust_speech::agent::TtsPreEnqueueHook;
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsProgressiveSink, TtsProvider, VendorFactory,
};
use node_webrtc_rust_speech::{PcmWriter, SttProvider, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};

const N: u64 = 24;
const BOUND: Duration = Duration::from_secs(20);

fn config(with_tts: bool) -> VoiceAgentConfig {
    VoiceAgentConfig {
        stt: None,
        tts: with_tts.then(|| TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
            sentence_case: Default::default(),
            transport: None,
        }),
        vad: VadConfig {
            enabled: false,
            ..VadConfig::default()
        },
        ..Default::default()
    }
}

async fn make_agent(with_tts: bool) -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
    let agent = VoiceAgent::new(config(with_tts), Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent
}

type Gates = Arc<Mutex<HashMap<u64, oneshot::Receiver<()>>>>;

/// Installs a hook that parks ticket `t` until its gate sender is fired (or dropped).
fn install_gates(
    agent: &VoiceAgent,
    tickets: impl Iterator<Item = u64>,
) -> HashMap<u64, oneshot::Sender<()>> {
    let gates: Gates = Arc::new(Mutex::new(HashMap::new()));
    let mut senders = HashMap::new();
    for t in tickets {
        let (tx, rx) = oneshot::channel();
        gates.lock().unwrap().insert(t, rx);
        senders.insert(t, tx);
    }
    let hook: TtsPreEnqueueHook = Arc::new(move |ticket| {
        let rx = gates.lock().unwrap().remove(&ticket);
        Box::pin(async move {
            if let Some(rx) = rx {
                let _ = rx.await;
            }
        })
    });
    agent.set_tts_pre_enqueue_hook(Some(hook));
    senders
}

/// Like [`tagged`] but waits for playback, so a cancelled queued job rejects the send.
fn blocking(id: &str) -> SendTextToTtsOptions {
    SendTextToTtsOptions {
        non_blocking: false,
        ..tagged(id)
    }
}

fn tagged(id: &str) -> SendTextToTtsOptions {
    SendTextToTtsOptions {
        non_blocking: true,
        reply_id: Some(id.to_string()),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_sends_are_queued_and_played_in_call_order() {
    let agent = make_agent(true).await;
    let mut rx = agent.subscribe_events();
    let mut gates = install_gates(&agent, 0..N);

    // Tickets are taken here, synchronously, in call order.
    let mut handles = Vec::new();
    for i in 0..N {
        let ticket = agent.reserve_tts_send_for_reply(Some(&format!("r{i}")));
        assert_eq!(ticket.id(), i);
        let agent = Arc::clone(&agent);
        handles.push(tokio::spawn(async move {
            agent
                .send_text_to_tts_reserved(ticket, &format!("{i}"), tagged(&format!("r{i}")))
                .await
        }));
    }
    // The last call reaches the queue first, the first call last.
    for i in (0..N).rev() {
        gates.remove(&i).unwrap().send(()).unwrap();
        tokio::task::yield_now().await;
    }
    for h in handles {
        timeout(BOUND, h)
            .await
            .expect("send bounded")
            .unwrap()
            .unwrap();
    }

    let mut starts = Vec::new();
    timeout(BOUND, async {
        while starts.len() < N as usize {
            let event = rx.recv().await.unwrap();
            if event.kind == SpeechEventKind::AgentReplyStart {
                starts.push(event.reply_id.unwrap_or_default());
            }
        }
    })
    .await
    .expect("all replies start");
    let expected: Vec<String> = (0..N).map(|i| format!("r{i}")).collect();
    assert_eq!(
        starts, expected,
        "agent_reply_start order must equal call order"
    );
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queue_holds_jobs_in_ticket_order() {
    let agent = make_agent(true).await;
    // Keep the worker from dequeuing: hold the first ticket's job back, inspect the rest.
    let mut gates = install_gates(&agent, 0..N);
    let mut handles = Vec::new();
    for i in 0..N {
        let ticket = agent.reserve_tts_send_for_reply(Some(&format!("q{i}")));
        let agent = Arc::clone(&agent);
        handles.push(tokio::spawn(async move {
            agent
                .send_text_to_tts_reserved(ticket, &format!("q {i}"), tagged(&format!("q{i}")))
                .await
        }));
    }
    for i in (1..N).rev() {
        gates.remove(&i).unwrap().send(()).unwrap();
    }
    // Tickets 1.. are past their gate but must not be queued ahead of ticket 0.
    tokio::task::yield_now().await;
    assert!(
        agent.queued_tts_tickets().await.is_empty(),
        "no job may jump ahead of ticket 0"
    );
    gates.remove(&0).unwrap().send(()).unwrap();
    for h in handles {
        timeout(BOUND, h)
            .await
            .expect("send bounded")
            .unwrap()
            .unwrap();
    }
    let queued = agent.queued_tts_tickets().await;
    let mut sorted = queued.clone();
    sorted.sort_unstable();
    assert_eq!(queued, sorted, "queue must stay in ticket order");
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ticket_that_gives_up_does_not_block_later_tickets() {
    // Error path: no TTS configured, every send errors and must release its turn.
    let agent = make_agent(false).await;
    let mut handles = Vec::new();
    for i in 0..N {
        let ticket = agent.reserve_tts_send_for_reply(Some("x"));
        let agent = Arc::clone(&agent);
        handles.push(tokio::spawn(async move {
            agent
                .send_text_to_tts_reserved(ticket, &format!("x {i}"), tagged("x"))
                .await
        }));
    }
    for h in handles {
        let res = timeout(BOUND, h)
            .await
            .expect("error path bounded")
            .unwrap();
        assert!(res.unwrap_err().to_string().contains("TTS not configured"));
    }

    // Aborted send future and an unused (dropped) ticket.
    let agent = make_agent(true).await;
    let _gates = install_gates(&agent, 0..1); // ticket 0 parks forever
    let t0 = agent.reserve_tts_send_for_reply(Some("p0"));
    let t1 = agent.reserve_tts_send();
    let t2 = agent.reserve_tts_send_for_reply(Some("p2"));
    let a0 = Arc::clone(&agent);
    let parked = tokio::spawn(async move {
        a0.send_text_to_tts_reserved(t0, "parked", tagged("p0"))
            .await
    });
    tokio::task::yield_now().await;
    parked.abort();
    drop(t1);
    timeout(
        BOUND,
        agent.send_text_to_tts_reserved(t2, "after", tagged("p2")),
    )
    .await
    .expect("ticket 2 is not blocked by an aborted or dropped lower ticket")
    .unwrap();
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flush_cancels_tickets_still_on_their_way_and_keeps_later_ones() {
    let agent = make_agent(true).await;
    let mut rx = agent.subscribe_events();
    let mut gates = install_gates(&agent, 0..6);
    let mut handles = Vec::new();
    for i in 0..6u64 {
        let ticket = agent.reserve_tts_send_for_reply(Some(&format!("f{i}")));
        let agent = Arc::clone(&agent);
        handles.push(tokio::spawn(async move {
            agent
                .send_text_to_tts_reserved(ticket, &format!("f {i}"), tagged(&format!("f{i}")))
                .await
        }));
    }
    let cut = agent.begin_flush_tts();
    let later = agent.reserve_tts_send_for_reply(Some("k"));
    timeout(BOUND, agent.flush_tts_from(cut))
        .await
        .expect("flush bounded")
        .unwrap();

    // The later send is not blocked by the cancelled, still-parked tickets 0..6.
    timeout(
        BOUND,
        agent.send_text_to_tts_reserved(later, "kept", tagged("k")),
    )
    .await
    .expect("later ticket not blocked by cancelled ones")
    .unwrap();

    for i in 0..6u64 {
        gates.remove(&i).unwrap().send(()).unwrap();
    }
    for h in handles {
        let res = timeout(BOUND, h)
            .await
            .expect("cancelled send bounded")
            .unwrap();
        assert!(
            res.unwrap_err().to_string().contains("TTS cancelled"),
            "a send called before flush is cancelled"
        );
    }
    // The cut owns the reply ends of the sends it cancelled on their way: once each, in ticket
    // order (nothing was queued before them).
    let mut ends = Vec::new();
    let collected = timeout(Duration::from_millis(300), async {
        while let Ok(event) = rx.recv().await {
            if event.kind == SpeechEventKind::AgentReplyEnd {
                let id = event.reply_id.unwrap_or_default();
                if id.starts_with('f') {
                    ends.push(id);
                }
            }
        }
    })
    .await;
    assert!(
        collected.is_err(),
        "event bus stays open during the settle window"
    );
    let expected: Vec<String> = (0..6).map(|i| format!("f{i}")).collect();
    assert_eq!(
        ends, expected,
        "in-flight reply ends once each, in ticket order"
    );
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reply_id_must_match_the_ticket() {
    let agent = make_agent(true).await;
    let none = agent.reserve_tts_send();
    let err = agent
        .send_text_to_tts_reserved(none, "x", tagged("x"))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("differs from the id the ticket"),
        "{err}"
    );
    let a = agent.reserve_tts_send_for_reply(Some("a"));
    let err = agent
        .send_text_to_tts_reserved(a, "x", tagged("b"))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("differs from the id the ticket"),
        "{err}"
    );
    let a = agent.reserve_tts_send_for_reply(Some("a"));
    let err = agent
        .send_text_to_tts_reserved(a, "x", SendTextToTtsOptions::default())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("differs from the id the ticket"),
        "{err}"
    );
    // The rejected tickets gave their place up.
    let ok = agent.reserve_tts_send_for_reply(Some("ok"));
    timeout(
        BOUND,
        agent.send_text_to_tts_reserved(ok, "fine", tagged("ok")),
    )
    .await
    .expect("later ticket not blocked by rejected ones")
    .unwrap();
    agent.stop().await.unwrap();
}

/// TTS that streams a 300 ms chunk for texts starting with `long` and a 40 ms chunk otherwise.
/// The synthesis job lasts until its audio has played, so `p` holds the worker that long.
struct LongTts;

#[async_trait::async_trait]
impl TtsProvider for LongTts {
    fn vendor_name(&self) -> &'static str {
        "mock-long"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let duration_ms: u32 = if text.starts_with("long") { 300 } else { 40 };
        let chunk = TtsAudioChunk {
            pcm: Bytes::from(vec![0u8; 192 * duration_ms as usize]),
            duration_ms,
        };
        if let Some(sink) = sink.as_ref() {
            let _ = sink.send(chunk.clone());
        }
        Ok(vec![chunk])
    }
}

struct LongFactory;

impl VendorFactory for LongFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(LongTts))
    }
}

/// Parks the synthesis worker on ticket `t` (before it takes the TTS lock) until the returned
/// sender fires; the second receiver reports that the worker is parked.
fn park_worker_on(agent: &VoiceAgent, t: u64) -> (oneshot::Sender<()>, oneshot::Receiver<()>) {
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let (parked_tx, parked_rx) = oneshot::channel::<()>();
    let slot = Arc::new(Mutex::new(Some((release_rx, parked_tx))));
    let hook: TtsPreEnqueueHook = Arc::new(move |ticket| {
        let taken = if ticket == t {
            slot.lock().unwrap().take()
        } else {
            None
        };
        Box::pin(async move {
            if let Some((release_rx, parked_tx)) = taken {
                let _ = parked_tx.send(());
                let _ = release_rx.await;
            }
        })
    });
    agent.set_tts_pre_synthesis_hook(Some(hook));
    (release_tx, parked_rx)
}

/// Event kinds and reply ids in emit order.
type SeenEvents = Arc<Mutex<Vec<(SpeechEventKind, Option<String>)>>>;

/// `p` plays first; then the worker is parked on `w` (no reply id), so `f0` stays queued;
/// `f1` is parked before the queue and `f2..f5` wait for their turn behind it. The flush cut is
/// taken, `f2..f5` (cancelled by it) are let finish, and only then does the flush emit. Every
/// reply ends exactly once, in ticket order, and the cancelled ones end after the flush's
/// `agent_speaking_end`.
async fn flush_in_flight_reply_ends(barge_in: bool) {
    let mut cfg = config(true);
    if barge_in {
        // Manual flush with barge-in enabled and `use_vad = false` runs
        // `barge_in_with_speaking_end`.
        cfg.vad.enabled = true;
        cfg.vad.barge_in.enabled = true;
        cfg.vad.barge_in.use_vad = false;
        cfg.vad.barge_in.flush_tts = true;
        cfg.vad.barge_in.agent_playback_guard_ms = 0;
    }
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(LongFactory));
    let agent = VoiceAgent::new(cfg, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| {
        tokio::task::block_in_place(|| std::thread::sleep(Duration::from_millis(2)));
        Ok(())
    });
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let mut rx = agent.subscribe_events();
    let seen: SeenEvents = Arc::default();
    let seen_c = Arc::clone(&seen);
    let collector = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            seen_c.lock().unwrap().push((event.kind, event.reply_id));
        }
    });
    // Ticket 0 = p, 1 = w (worker parks on it), 2 = f0 (queued), 3 = f1 (gated), 4..=8 = f2..f5.
    let (release_worker, worker_parked) = park_worker_on(&agent, 1);
    let mut gates = install_gates(&agent, 3..4);

    agent
        .send_text_to_tts_with_options("long reply that keeps playing", tagged("p"))
        .await
        .unwrap();
    agent
        .send_text_to_tts_with_options(
            "worker",
            SendTextToTtsOptions {
                non_blocking: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    timeout(BOUND, worker_parked)
        .await
        .expect("worker parks on w")
        .unwrap();

    let mut handles = Vec::new();
    for i in 0..6u64 {
        let reply = format!("f{i}");
        let ticket = agent.reserve_tts_send_for_reply(Some(&reply));
        assert_eq!(ticket.id(), i + 2);
        let agent = Arc::clone(&agent);
        handles.push(tokio::spawn(async move {
            agent
                .send_text_to_tts_reserved(ticket, &format!("f {i}"), blocking(&reply))
                .await
        }));
    }
    timeout(BOUND, async {
        while agent.queued_tts_tickets().await != vec![2] {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("f0 is queued behind the parked worker");

    let cut = agent.begin_flush_tts();
    // f2..f5 wake on the cut and finish before the flush emits anything.
    let mut results = Vec::new();
    for h in handles.drain(2..) {
        results.push(timeout(BOUND, h).await.expect("send bounded").unwrap());
    }
    timeout(BOUND, agent.flush_tts_from(cut))
        .await
        .expect("flush bounded")
        .unwrap();
    gates.remove(&3).unwrap().send(()).unwrap();
    let _ = release_worker.send(());
    for h in handles {
        results.push(timeout(BOUND, h).await.expect("send bounded").unwrap());
    }
    for res in results {
        let err = res.expect_err("a send called before the flush is cancelled");
        assert!(err.to_string().contains("TTS cancelled"), "{err}");
    }

    let expected: Vec<String> = std::iter::once("p".to_string())
        .chain((0..6).map(|i| format!("f{i}")))
        .collect();
    let reply_ends = |seen: &[(SpeechEventKind, Option<String>)]| -> Vec<(usize, String)> {
        seen.iter()
            .enumerate()
            .filter(|(_, (k, _))| *k == SpeechEventKind::AgentReplyEnd)
            .map(|(i, (_, id))| (i, id.clone().unwrap_or_default()))
            .collect()
    };
    timeout(BOUND, async {
        while reply_ends(&seen.lock().unwrap()).len() < expected.len() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("every reply ends: {:?}", seen.lock().unwrap()));
    // Settle window: a duplicate reply end would land here.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let seen_now = seen.lock().unwrap().clone();
    let ends = reply_ends(&seen_now);
    let ids: Vec<String> = ends.iter().map(|(_, id)| id.clone()).collect();
    assert_eq!(
        ids, expected,
        "reply ends once each, in ticket order: {seen_now:?}"
    );
    // `p`'s playback ends (`agent_speaking_end`) before any cancelled reply ends. (`w`, which
    // the worker holds in the hook at the cut, is cancelled by the flush; it has no reply id.)
    let speaking_end = seen_now
        .iter()
        .enumerate()
        .skip(ends[0].0)
        .find(|(_, (k, _))| *k == SpeechEventKind::AgentSpeakingEnd)
        .map(|(i, _)| i)
        .expect("agent_speaking_end after p ends");
    for (idx, id) in &ends[1..] {
        assert!(
            *idx > speaking_end,
            "{id} must end after the flush's agent_speaking_end: {seen_now:?}"
        );
    }
    agent.stop().await.unwrap();
    collector.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flush_emits_reply_end_for_in_flight_sends_once_after_queued_ones() {
    flush_in_flight_reply_ends(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn barge_in_flush_emits_reply_end_for_in_flight_sends_once_after_queued_ones() {
    flush_in_flight_reply_ends(true).await;
}

// ---------------------------------------------------------------------------------------------
// A job the synthesis worker has dequeued but not started (held across the pre-synthesis hook
// and the LID wait) belongs to the flush, like a queued one.
// ---------------------------------------------------------------------------------------------

/// Records every text that reached the vendor; a text starting with `park` waits in the vendor
/// call until the test releases it.
#[derive(Default)]
struct Probe {
    synthesized: Mutex<Vec<String>>,
    park: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

impl Probe {
    fn reached_vendor(&self, text: &str) -> bool {
        self.synthesized.lock().unwrap().iter().any(|t| t == text)
    }

    /// Arms the vendor park: the first `park…` text signals `entered` and waits for `release`.
    fn arm_park(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *self.park.lock().unwrap() = Some((entered_tx, release_rx));
        (entered_rx, release_tx)
    }
}

struct ProbeTts(Arc<Probe>);

#[async_trait::async_trait]
impl TtsProvider for ProbeTts {
    fn vendor_name(&self) -> &'static str {
        "mock-probe"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.0.synthesized.lock().unwrap().push(text.to_string());
        let park = if text.starts_with("park") {
            self.0.park.lock().unwrap().take()
        } else {
            None
        };
        if let Some((entered, release)) = park {
            let _ = entered.send(());
            let _ = release.await;
        }
        let chunk = TtsAudioChunk {
            pcm: Bytes::from(vec![0u8; 192 * 40]),
            duration_ms: 40,
        };
        if let Some(sink) = sink.as_ref() {
            let _ = sink.send(chunk.clone());
        }
        Ok(vec![chunk])
    }
}

struct ProbeFactory(Arc<Probe>);

impl VendorFactory for ProbeFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(ProbeTts(Arc::clone(&self.0))))
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum CutBy {
    Flush,
    /// Manual flush with barge-in enabled (`use_vad = false`): `barge_in_with_speaking_end`.
    BargeInFlush,
    Stop,
}

async fn probe_agent(cut_by: CutBy) -> (Arc<VoiceAgent>, Arc<Probe>) {
    let mut cfg = config(true);
    if cut_by == CutBy::BargeInFlush {
        cfg.vad.enabled = true;
        cfg.vad.barge_in.enabled = true;
        cfg.vad.barge_in.use_vad = false;
        cfg.vad.barge_in.flush_tts = true;
        cfg.vad.barge_in.agent_playback_guard_ms = 0;
    }
    let probe = Arc::new(Probe::default());
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(ProbeFactory(Arc::clone(&probe))));
    let agent = VoiceAgent::new(cfg, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    (agent, probe)
}

/// (kind, reply id, interrupted)
type Seen = Vec<(SpeechEventKind, Option<String>, Option<bool>)>;

/// Everything already emitted, without waiting.
fn drain_now(rx: &mut tokio::sync::broadcast::Receiver<SpeechEvent>) -> Seen {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(ev) => out.push((ev.kind, ev.reply_id, ev.interrupted)),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            | Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return out,
            Err(e) => panic!("event bus: {e}"),
        }
    }
}

/// Receives until `reply` ends (bounded); returns everything received on the way.
async fn recv_until_reply_end(
    rx: &mut tokio::sync::broadcast::Receiver<SpeechEvent>,
    reply: &str,
) -> Seen {
    let mut out = Vec::new();
    timeout(BOUND, async {
        loop {
            let ev = rx.recv().await.expect("event bus open");
            let done =
                ev.kind == SpeechEventKind::AgentReplyEnd && ev.reply_id.as_deref() == Some(reply);
            out.push((ev.kind, ev.reply_id, ev.interrupted));
            if done {
                return;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{reply} ends: {out:?}"));
    out
}

fn reply_ends(seen: &Seen) -> Vec<(String, Option<bool>)> {
    seen.iter()
        .filter(|(k, _, _)| *k == SpeechEventKind::AgentReplyEnd)
        .map(|(_, id, interrupted)| (id.clone().unwrap_or_default(), *interrupted))
        .collect()
}

fn events_for<'a>(seen: &'a Seen, reply: &'a str) -> impl Iterator<Item = &'a SpeechEventKind> {
    seen.iter()
        .filter(move |(_, id, _)| id.as_deref() == Some(reply))
        .map(|(k, _, _)| k)
}

/// The worker holds send A (ticket 0, reply `r1`) in the pre-synthesis hook while the cut runs.
/// The cut settles A itself: A rejects "TTS cancelled" before the hook is released, A never
/// reaches the vendor, and `r1` ends exactly once (interrupted), emitted by the cut.
async fn cut_cancels_job_dequeued_before_synthesis(cut_by: CutBy) {
    let (agent, probe) = probe_agent(cut_by).await;
    let mut rx = agent.subscribe_events();
    let (release_worker, worker_parked) = park_worker_on(&agent, 0);
    let a = {
        let agent = Arc::clone(&agent);
        tokio::spawn(async move {
            agent
                .send_text_to_tts_with_options("held a", blocking("r1"))
                .await
        })
    };
    timeout(BOUND, worker_parked)
        .await
        .expect("worker parks on A")
        .unwrap();

    let stop = match cut_by {
        CutBy::Stop => {
            let agent = Arc::clone(&agent);
            Some(tokio::spawn(async move { agent.stop().await }))
        }
        CutBy::Flush | CutBy::BargeInFlush => {
            timeout(BOUND, agent.flush_tts())
                .await
                .expect("flush bounded")
                .unwrap();
            None
        }
    };
    // The cut settles A while the worker still holds it in the hook.
    let res = timeout(BOUND, a)
        .await
        .expect("A is settled by the cut, not by the worker")
        .unwrap();
    let err = res.expect_err("A was called before the cut");
    assert!(err.to_string().contains("TTS cancelled"), "{err}");
    let at_cut = drain_now(&mut rx);
    assert_eq!(
        reply_ends(&at_cut),
        vec![("r1".to_string(), Some(true))],
        "the cut emits r1's interrupted reply end: {at_cut:?}"
    );

    let _ = release_worker.send(());
    let later = if let Some(stop) = stop {
        timeout(BOUND, stop)
            .await
            .expect("stop bounded")
            .unwrap()
            .unwrap();
        drain_now(&mut rx)
    } else {
        // A later, unrelated send plays after the worker let A go; anything the worker would
        // emit for A lands before it.
        timeout(
            BOUND,
            agent.send_text_to_tts_with_options("z", blocking("rz")),
        )
        .await
        .expect("z bounded")
        .unwrap();
        let seen = recv_until_reply_end(&mut rx, "rz").await;
        agent.stop().await.unwrap();
        seen
    };
    assert_eq!(
        events_for(&later, "r1").count(),
        0,
        "nothing for r1 after the cut: {later:?}"
    );
    assert!(
        !events_for(&at_cut, "r1").any(|k| *k == SpeechEventKind::AgentReplyStart),
        "r1 never starts: {at_cut:?}"
    );
    assert!(
        !probe.reached_vendor("held a"),
        "A never reaches the vendor"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flush_cancels_job_dequeued_before_synthesis() {
    cut_cancels_job_dequeued_before_synthesis(CutBy::Flush).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn barge_in_flush_cancels_job_dequeued_before_synthesis() {
    cut_cancels_job_dequeued_before_synthesis(CutBy::BargeInFlush).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_cancels_job_dequeued_before_synthesis() {
    cut_cancels_job_dequeued_before_synthesis(CutBy::Stop).await;
}

/// A (r1) is held by the worker, B (r2) and C (r3) are queued behind it. The flush ends r1, r2,
/// r3 in that order, once each, and none of them reaches the vendor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flush_held_job_reply_end_precedes_queued_ones() {
    let (agent, probe) = probe_agent(CutBy::Flush).await;
    let mut rx = agent.subscribe_events();
    let (release_worker, worker_parked) = park_worker_on(&agent, 0);
    let a = {
        let agent = Arc::clone(&agent);
        tokio::spawn(async move {
            agent
                .send_text_to_tts_with_options("held a", blocking("r1"))
                .await
        })
    };
    timeout(BOUND, worker_parked)
        .await
        .expect("worker parks on A")
        .unwrap();
    agent
        .send_text_to_tts_with_options("queued b", tagged("r2"))
        .await
        .unwrap();
    agent
        .send_text_to_tts_with_options("queued c", tagged("r3"))
        .await
        .unwrap();
    assert_eq!(agent.queued_tts_tickets().await, vec![1, 2]);

    timeout(BOUND, agent.flush_tts())
        .await
        .expect("flush bounded")
        .unwrap();
    let res = timeout(BOUND, a)
        .await
        .expect("A is settled by the flush")
        .unwrap();
    assert!(res
        .expect_err("A cancelled")
        .to_string()
        .contains("TTS cancelled"));
    let at_flush = drain_now(&mut rx);
    assert_eq!(
        reply_ends(&at_flush),
        vec![
            ("r1".to_string(), Some(true)),
            ("r2".to_string(), Some(true)),
            ("r3".to_string(), Some(true)),
        ],
        "held reply ends first, then the queued ones: {at_flush:?}"
    );

    let _ = release_worker.send(());
    timeout(
        BOUND,
        agent.send_text_to_tts_with_options("z", blocking("rz")),
    )
    .await
    .expect("z bounded")
    .unwrap();
    let later = recv_until_reply_end(&mut rx, "rz").await;
    for r in ["r1", "r2", "r3"] {
        assert_eq!(
            events_for(&later, r).count(),
            0,
            "nothing for {r}: {later:?}"
        );
    }
    for t in ["held a", "queued b", "queued c"] {
        assert!(!probe.reached_vendor(t), "{t} never reaches the vendor");
    }
    agent.stop().await.unwrap();
}

/// The cut is taken first; D (sent after it) is dequeued and held when `flush_tts_from` runs.
/// D is kept: it plays and ends normally, and its send resolves Ok.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn held_job_after_begin_flush_is_kept() {
    let (agent, probe) = probe_agent(CutBy::Flush).await;
    let mut rx = agent.subscribe_events();
    // Ticket 0 plays to the end, so the cut's `below` is 1.
    timeout(
        BOUND,
        agent.send_text_to_tts_with_options("w", blocking("r0")),
    )
    .await
    .expect("w bounded")
    .unwrap();
    recv_until_reply_end(&mut rx, "r0").await;

    let cut = agent.begin_flush_tts();
    let (release_worker, worker_parked) = park_worker_on(&agent, 1);
    let d = {
        let agent = Arc::clone(&agent);
        tokio::spawn(async move {
            agent
                .send_text_to_tts_with_options("held d", blocking("rd"))
                .await
        })
    };
    timeout(BOUND, worker_parked)
        .await
        .expect("worker parks on D")
        .unwrap();
    timeout(BOUND, agent.flush_tts_from(cut))
        .await
        .expect("flush bounded")
        .unwrap();
    let at_flush = drain_now(&mut rx);
    assert_eq!(events_for(&at_flush, "rd").count(), 0, "{at_flush:?}");

    let _ = release_worker.send(());
    timeout(BOUND, d)
        .await
        .expect("D bounded")
        .unwrap()
        .expect("D plays");
    let seen = recv_until_reply_end(&mut rx, "rd").await;
    assert_eq!(
        reply_ends(&seen),
        vec![("rd".to_string(), Some(false))],
        "D ends normally: {seen:?}"
    );
    assert!(probe.reached_vendor("held d"));
    agent.stop().await.unwrap();
}

/// The other interleaving: the worker has already taken A out of its hold and is inside the
/// vendor call when the flush runs. A is then in flight (not held); the flush cuts it through
/// the epoch, and `r1` still ends exactly once, interrupted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flush_after_worker_started_job_ends_it_once() {
    let (agent, probe) = probe_agent(CutBy::Flush).await;
    let mut rx = agent.subscribe_events();
    let (entered, release_vendor) = probe.arm_park();
    let a = {
        let agent = Arc::clone(&agent);
        tokio::spawn(async move {
            agent
                .send_text_to_tts_with_options("park a", blocking("r1"))
                .await
        })
    };
    timeout(BOUND, entered)
        .await
        .expect("A reaches the vendor")
        .unwrap();
    timeout(BOUND, agent.flush_tts())
        .await
        .expect("flush bounded")
        .unwrap();
    let _ = release_vendor.send(());
    let _ = timeout(BOUND, a).await.expect("A bounded").unwrap();
    let mut seen = recv_until_reply_end(&mut rx, "r1").await;
    timeout(
        BOUND,
        agent.send_text_to_tts_with_options("z", blocking("rz")),
    )
    .await
    .expect("z bounded")
    .unwrap();
    seen.extend(recv_until_reply_end(&mut rx, "rz").await);
    let r1_ends: Vec<_> = reply_ends(&seen)
        .into_iter()
        .filter(|(id, _)| id == "r1")
        .collect();
    assert_eq!(
        r1_ends,
        vec![("r1".to_string(), Some(true))],
        "r1 ends once, interrupted: {seen:?}"
    );
    agent.stop().await.unwrap();
}

/// The worker has taken A ("park a", r1) back and read the epoch, but the streaming job has not
/// started yet (parked in the post-take hook) when a whole flush runs. The held slot is empty,
/// so the flush settles nothing for A; the streaming job must see the epoch moved before it
/// produces anything. A never reaches the vendor and r1 ends exactly once, interrupted, from
/// the worker's discard path (the flush emits none for it).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flush_after_take_back_before_streaming_start_cancels_job() {
    assert!(
        node_webrtc_rust_speech::pipeline::tts_stream_chunks_enabled(),
        "this test covers the streaming path (VOICE_TTS_STREAM_CHUNKS unset)"
    );
    let (agent, probe) = probe_agent(CutBy::Flush).await;
    let mut rx = agent.subscribe_events();
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let (parked_tx, parked_rx) = oneshot::channel::<()>();
    let slot = Arc::new(Mutex::new(Some((release_rx, parked_tx))));
    let hook: TtsPreEnqueueHook = Arc::new(move |ticket| {
        let taken = if ticket == 0 {
            slot.lock().unwrap().take()
        } else {
            None
        };
        Box::pin(async move {
            if let Some((release_rx, parked_tx)) = taken {
                let _ = parked_tx.send(());
                let _ = release_rx.await;
            }
        })
    });
    agent.set_tts_post_take_hook(Some(hook));
    let a = {
        let agent = Arc::clone(&agent);
        tokio::spawn(async move {
            agent
                .send_text_to_tts_with_options("park a", blocking("r1"))
                .await
        })
    };
    timeout(BOUND, parked_rx)
        .await
        .expect("worker parks after taking A back")
        .unwrap();
    timeout(BOUND, agent.flush_tts())
        .await
        .expect("flush bounded")
        .unwrap();
    let at_flush = drain_now(&mut rx);
    assert_eq!(
        events_for(&at_flush, "r1").count(),
        0,
        "the flush emits nothing for a started job: {at_flush:?}"
    );

    let _ = release_tx.send(());
    let _ = timeout(BOUND, a).await.expect("A bounded").unwrap();
    timeout(
        BOUND,
        agent.send_text_to_tts_with_options("z", blocking("rz")),
    )
    .await
    .expect("z bounded")
    .unwrap();
    let later = recv_until_reply_end(&mut rx, "rz").await;
    let r1: Vec<_> = later
        .iter()
        .filter(|(_, id, _)| id.as_deref() == Some("r1"))
        .collect();
    assert_eq!(
        r1,
        vec![&(
            SpeechEventKind::AgentReplyEnd,
            Some("r1".to_string()),
            Some(true)
        )],
        "r1: one interrupted reply end and nothing else (no start, no audio): {later:?}"
    );
    assert!(
        !probe.reached_vendor("park a"),
        "A never reaches the vendor"
    );
    agent.stop().await.unwrap();
}
