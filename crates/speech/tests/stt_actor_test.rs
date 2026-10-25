//! Per-agent STT actor: ordering, attribution of transcripts to utterances, bounded waits.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, VoiceSessionContext};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::otel::SttMetricAttrs;
use node_webrtc_rust_speech::pipeline::{
    FinalizeMode, SttOutput, SttProvider, SttTranscript, TtsProvider, VendorFactory,
};
use node_webrtc_rust_speech::stt_actor::{
    backlog_episode, default_stt_latency_sink, spawn_stt_actor, SttActorHandle, SttActorOptions,
    SttCommand, SttResult, STT_ACTOR_BUSY_WINDOW_MS, STT_ACTOR_STOP_MS,
};
use node_webrtc_rust_speech::VendorRegistry;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::time::timeout;

#[derive(Default)]
struct Shared {
    pushed: Vec<Bytes>,
    /// Outputs the provider will hand out from `poll_output` / `poll_transcript`.
    out: VecDeque<SttOutput>,
    /// Inline providers: transcripts queued by `finalize_utterance`.
    on_finalize: Vec<SttTranscript>,
    /// One output moved into `out` by every `push_audio`.
    push_hooks: VecDeque<SttOutput>,
    finalize_requests: usize,
    started: bool,
    stopped: bool,
}

#[derive(Default)]
struct Probe {
    state: Mutex<Shared>,
    finalize_requested: Notify,
    pushed: Notify,
    gate: Notify,
    open_pending: AtomicBool,
    /// Number of `poll_output` calls.
    polls: AtomicUsize,
}

impl Probe {
    fn out(&self, item: SttOutput) {
        self.state.lock().unwrap().out.push_back(item);
    }
    fn pushed(&self) -> Vec<Bytes> {
        self.state.lock().unwrap().pushed.clone()
    }
}

struct Scripted {
    probe: Arc<Probe>,
    mode: FinalizeMode,
    wait_ms: u64,
    gate_first_push: bool,
    gated: bool,
    stop_hangs: bool,
}

fn scripted(mode: FinalizeMode) -> (Scripted, Arc<Probe>) {
    let probe = Arc::new(Probe::default());
    (
        Scripted {
            probe: Arc::clone(&probe),
            mode,
            wait_ms: 10_000,
            gate_first_push: false,
            gated: false,
            stop_hangs: false,
        },
        probe,
    )
}

#[async_trait]
impl SttProvider for Scripted {
    fn vendor_name(&self) -> &'static str {
        "scripted"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        self.probe.state.lock().unwrap().started = true;
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        self.probe.state.lock().unwrap().stopped = true;
        if self.stop_hangs {
            std::future::pending::<()>().await;
        }
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        if self.gate_first_push && !self.gated {
            self.gated = true;
            self.probe.gate.notified().await;
        }
        {
            let mut s = self.probe.state.lock().unwrap();
            s.pushed.push(pcm);
            if let Some(hook) = s.push_hooks.pop_front() {
                s.out.push_back(hook);
            }
        }
        self.probe.pushed.notify_one();
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let mut s = self.probe.state.lock().unwrap();
        match s.out.front() {
            Some(SttOutput::Transcript(_)) => match s.out.pop_front() {
                Some(SttOutput::Transcript(t)) => Ok(Some(t)),
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }

    async fn poll_output(&mut self) -> SpeechResult<Option<SttOutput>> {
        self.probe.polls.fetch_add(1, Ordering::SeqCst);
        Ok(self.probe.state.lock().unwrap().out.pop_front())
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let mut s = self.probe.state.lock().unwrap();
        let queued = std::mem::take(&mut s.on_finalize);
        for t in queued {
            s.out.push_back(SttOutput::Transcript(t));
        }
        Ok(())
    }

    async fn request_finalize(&mut self) -> SpeechResult<()> {
        self.probe.state.lock().unwrap().finalize_requests += 1;
        self.probe.finalize_requested.notify_one();
        Ok(())
    }

    fn finalize_mode(&self) -> FinalizeMode {
        self.mode
    }

    fn finalize_wait_ms(&self) -> u64 {
        self.wait_ms
    }

    fn stream_open_pending(&self) -> bool {
        self.probe.open_pending.load(Ordering::SeqCst)
    }
}

struct SwapFactory {
    next: Mutex<Option<Box<dyn SttProvider>>>,
}

impl VendorFactory for SwapFactory {
    fn create_stt(&self, _config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        self.next
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| SpeechError::Config("no provider available".into()))
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Err(SpeechError::Config("no tts".into()))
    }
}

fn swap_registry(next: Option<Box<dyn SttProvider>>) -> Arc<VendorRegistry> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(SwapFactory {
            next: Mutex::new(next),
        }),
    );
    Arc::new(registry)
}

fn stt_config() -> SttConfig {
    SttConfig {
        provider: SttVendor::Mock,
        model: None,
        model_path: None,
        language: None,
        api_key: None,
        endpoint: None,
        transport: None,
    }
}

fn options() -> SttActorOptions {
    SttActorOptions {
        open_wait_max_ms: 120_000,
        test_final_delay_ms: 0,
        audio_tap_dir: None,
        audio_tap_agent_id: 0,
        metric_attrs: SttMetricAttrs::default(),
        stt_latency_sink: default_stt_latency_sink(),
    }
}

type Samples = Arc<Mutex<Vec<(f64, SttMetricAttrs)>>>;

/// Options whose latency samples land in the returned list.
fn recording_options() -> (SttActorOptions, Samples) {
    let samples: Samples = Arc::new(Mutex::new(Vec::new()));
    let sink_samples = Arc::clone(&samples);
    let mut o = options();
    o.metric_attrs = SttMetricAttrs {
        stt_vendor: "scripted".into(),
        stt_model: "m1".into(),
        stt_language: "en".into(),
        project_id: String::new(),
    };
    o.stt_latency_sink = Arc::new(move |ms, attrs| {
        sink_samples.lock().unwrap().push((ms, attrs.clone()));
    });
    (o, samples)
}

async fn start_with_project(handle: &SttActorHandle, project: &str) {
    let (done_tx, done_rx) = oneshot::channel();
    assert!(handle.send(SttCommand::Start {
        ctx: VoiceSessionContext {
            project_id: Some(project.into()),
            ..Default::default()
        },
        done: done_tx,
    }));
    done_rx.await.unwrap().unwrap();
}

fn spawn(
    provider: Scripted,
    options: SttActorOptions,
) -> (SttActorHandle, mpsc::UnboundedReceiver<SttResult>) {
    spawn_stt_actor(
        Some(Box::new(provider)),
        Arc::new(VendorRegistry::new()),
        options,
    )
}

async fn next_result(rx: &mut mpsc::UnboundedReceiver<SttResult>, ms: u64) -> SttResult {
    timeout(Duration::from_millis(ms), rx.recv())
        .await
        .expect("no result in time")
        .expect("result channel closed")
}

async fn assert_quiet(rx: &mut mpsc::UnboundedReceiver<SttResult>, ms: u64) {
    if let Ok(got) = timeout(Duration::from_millis(ms), rx.recv()).await {
        panic!("unexpected result: {got:?}");
    }
}

fn partial(s: &str) -> SttOutput {
    SttOutput::Transcript(SttTranscript::Partial(s.into()))
}

fn final_out(s: &str) -> SttOutput {
    SttOutput::Transcript(SttTranscript::Final(s.into()))
}

#[track_caller]
fn expect_transcript(result: SttResult, key: Option<u64>, gen: u64, expected: SttTranscript) {
    match result {
        SttResult::Transcript {
            key: k,
            gen: g,
            transcript,
        } => {
            assert_eq!((k, g, transcript), (key, gen, expected));
        }
        other => panic!("expected transcript, got {other:?}"),
    }
}

#[track_caller]
fn expect_done(result: SttResult, key: u64, gen: u64, timed_out: bool) {
    match result {
        SttResult::FinalizeDone {
            key: k,
            gen: g,
            timed_out: t,
        } => assert_eq!((k, g, t), (key, gen, timed_out)),
        other => panic!("expected FinalizeDone, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn inline_results_attributed_to_end_utterance_key() {
    let (provider, probe) = scripted(FinalizeMode::Inline);
    {
        let mut s = probe.state.lock().unwrap();
        s.push_hooks.push_back(partial("p"));
        s.on_finalize.push(SttTranscript::Final("a".into()));
    }
    let (handle, mut rx) = spawn(provider, options());
    assert!(handle.send_audio(Bytes::from_static(&[1, 2, 3, 4])));
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));

    expect_transcript(
        next_result(&mut rx, 1000).await,
        None,
        0,
        SttTranscript::Partial("p".into()),
    );
    expect_transcript(
        next_result(&mut rx, 1000).await,
        Some(1),
        0,
        SttTranscript::Final("a".into()),
    );
    expect_done(next_result(&mut rx, 1000).await, 1, 0, false);
}

#[tokio::test(start_paused = true)]
async fn deferred_transcripts_before_marker_go_to_front_key() {
    let (provider, probe) = scripted(FinalizeMode::Deferred);
    let (handle, mut rx) = spawn(provider, options());
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    probe.finalize_requested.notified().await;
    probe.out(final_out("a"));
    probe.out(SttOutput::FinalizeDone);
    probe.out(partial("b"));

    expect_transcript(
        next_result(&mut rx, 1000).await,
        Some(1),
        0,
        SttTranscript::Final("a".into()),
    );
    expect_done(next_result(&mut rx, 1000).await, 1, 0, false);
    expect_transcript(
        next_result(&mut rx, 1000).await,
        None,
        0,
        SttTranscript::Partial("b".into()),
    );
}

#[tokio::test(start_paused = true)]
async fn deferred_timeout_emits_timed_out_and_swallows_late_marker() {
    let (mut provider, probe) = scripted(FinalizeMode::Deferred);
    provider.wait_ms = 1_000;
    let (handle, mut rx) = spawn(provider, options());
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    probe.finalize_requested.notified().await;

    tokio::time::advance(Duration::from_millis(1_001)).await;
    expect_done(next_result(&mut rx, 1000).await, 1, 0, true);

    probe.out(final_out("late"));
    probe.out(SttOutput::FinalizeDone);
    expect_transcript(
        next_result(&mut rx, 1000).await,
        Some(1),
        0,
        SttTranscript::Final("late".into()),
    );
    assert_quiet(&mut rx, 100).await;
}

#[tokio::test(start_paused = true)]
async fn open_pending_extends_deadline_up_to_open_wait_max() {
    let (mut provider, probe) = scripted(FinalizeMode::Deferred);
    provider.wait_ms = 1_000;
    probe.open_pending.store(true, Ordering::SeqCst);
    let (handle, mut rx) = spawn(
        provider,
        SttActorOptions {
            open_wait_max_ms: 5_000,
            ..options()
        },
    );
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    probe.finalize_requested.notified().await;

    assert_quiet(&mut rx, 1_001).await;
    assert_quiet(&mut rx, 3_800).await; // t = 4 801 ms
    expect_done(next_result(&mut rx, 1_000).await, 1, 0, true);
}

/// While a stream open is refused the wait is bounded by `open_wait_max_ms`. When the open
/// completes the vendor gets its normal finalize wait from that moment, not from the request:
/// the Finalize that waited behind the open must be able to answer.
#[tokio::test(start_paused = true)]
async fn finalize_wait_restarts_when_open_completes() {
    let (mut provider, probe) = scripted(FinalizeMode::Deferred);
    provider.wait_ms = 1_000;
    probe.open_pending.store(true, Ordering::SeqCst);
    let (handle, mut rx) = spawn(provider, options());
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    probe.finalize_requested.notified().await;

    // 30 s with the open refused: far past the 1 s finalize wait, still no expiry.
    assert_quiet(&mut rx, 30_000).await;

    // The open completes. The vendor answers before the restarted wait ends (900 ms, not 999 ms:
    // the actor notices the flip and polls the provider on its 20 ms tick).
    probe.open_pending.store(false, Ordering::SeqCst);
    assert_quiet(&mut rx, 900).await;
    probe.out(final_out("x"));
    probe.out(SttOutput::FinalizeDone);
    expect_transcript(
        next_result(&mut rx, 1_000).await,
        Some(1),
        0,
        SttTranscript::Final("x".into()),
    );
    expect_done(next_result(&mut rx, 1_000).await, 1, 0, false);
}

/// Same flip, but the vendor never answers: the entry expires one finalize wait after the open
/// completed (plus at most one actor tick to notice the flip), not earlier.
#[tokio::test(start_paused = true)]
async fn finalize_wait_after_open_completes_still_expires() {
    let (mut provider, probe) = scripted(FinalizeMode::Deferred);
    provider.wait_ms = 1_000;
    probe.open_pending.store(true, Ordering::SeqCst);
    let (handle, mut rx) = spawn(provider, options());
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    probe.finalize_requested.notified().await;

    assert_quiet(&mut rx, 30_000).await;
    probe.open_pending.store(false, Ordering::SeqCst);
    assert_quiet(&mut rx, 990).await;
    expect_done(next_result(&mut rx, 100).await, 1, 0, true);
}

#[tokio::test(start_paused = true)]
async fn swap_waits_for_outstanding_then_applies_in_order() {
    let (old, old_probe) = scripted(FinalizeMode::Deferred);
    let (new, new_probe) = scripted(FinalizeMode::Inline);
    new_probe
        .state
        .lock()
        .unwrap()
        .push_hooks
        .push_back(partial("n"));
    let registry = swap_registry(Some(Box::new(new)));
    let (handle, mut rx) = spawn_stt_actor(Some(Box::new(old)), registry, options());

    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    old_probe.finalize_requested.notified().await;
    let (done_tx, done_rx) = oneshot::channel();
    assert!(handle.send(SttCommand::Swap {
        config: stt_config(),
        ctx: VoiceSessionContext::default(),
        running: true,
        done: done_tx,
    }));
    let x = Bytes::from_static(b"XXXX");
    assert!(handle.send_audio(x.clone()));

    // The swap is parked behind the outstanding finalize; audio behind the swap is not read.
    assert_quiet(&mut rx, 300).await;
    assert!(old_probe.pushed().is_empty());
    assert!(new_probe.pushed().is_empty());

    old_probe.out(SttOutput::FinalizeDone);
    expect_done(next_result(&mut rx, 1000).await, 1, 0, false);
    match next_result(&mut rx, 1000).await {
        SttResult::SwapApplied { config, result } => {
            assert_eq!(config, stt_config());
            assert_eq!(result, Ok(()));
        }
        other => panic!("expected SwapApplied, got {other:?}"),
    }
    assert!(done_rx.await.unwrap().is_ok());
    expect_transcript(
        next_result(&mut rx, 1000).await,
        None,
        1,
        SttTranscript::Partial("n".into()),
    );
    assert_eq!(new_probe.pushed(), vec![x]);
    assert!(old_probe.pushed().is_empty());
    let new_state = new_probe.state.lock().unwrap();
    assert!(new_state.started);
    assert!(old_probe.state.lock().unwrap().stopped);
    assert_eq!(handle.status.generation.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn swap_failure_keeps_old_provider() {
    let (old, old_probe) = scripted(FinalizeMode::Inline);
    let registry = swap_registry(None);
    let (handle, mut rx) = spawn_stt_actor(Some(Box::new(old)), registry, options());

    let (done_tx, done_rx) = oneshot::channel();
    assert!(handle.send(SttCommand::Swap {
        config: stt_config(),
        ctx: VoiceSessionContext::default(),
        running: true,
        done: done_tx,
    }));
    match next_result(&mut rx, 1000).await {
        SttResult::SwapApplied { result, .. } => assert!(result.is_err()),
        other => panic!("expected SwapApplied, got {other:?}"),
    }
    assert!(done_rx.await.unwrap().is_err());

    let pcm = Bytes::from_static(b"keep");
    assert!(handle.send_audio(pcm.clone()));
    let (b_tx, _b_rx) = oneshot::channel();
    assert!(handle.send(SttCommand::Barrier { done: b_tx }));
    assert!(matches!(
        next_result(&mut rx, 1000).await,
        SttResult::BarrierReached { .. }
    ));
    assert_eq!(old_probe.pushed(), vec![pcm]);
    assert_eq!(handle.status.generation.load(Ordering::SeqCst), 0);
    assert!(!old_probe.state.lock().unwrap().stopped);
}

#[tokio::test(start_paused = true)]
async fn barrier_answers_after_outstanding_done() {
    let (provider, probe) = scripted(FinalizeMode::Deferred);
    let (handle, mut rx) = spawn(provider, options());
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    probe.finalize_requested.notified().await;
    let (done_tx, done_rx) = oneshot::channel();
    assert!(handle.send(SttCommand::Barrier { done: done_tx }));

    assert_quiet(&mut rx, 300).await;
    probe.out(SttOutput::FinalizeDone);
    expect_done(next_result(&mut rx, 1000).await, 1, 0, false);
    match next_result(&mut rx, 1000).await {
        SttResult::BarrierReached { done } => done.send(()).unwrap(),
        other => panic!("expected BarrierReached, got {other:?}"),
    }
    done_rx.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn stop_bounded_when_provider_stop_hangs() {
    let (mut provider, probe) = scripted(FinalizeMode::Inline);
    provider.stop_hangs = true;
    let (handle, _rx) = spawn(provider, options());
    let (done_tx, mut done_rx) = oneshot::channel();
    assert!(handle.send(SttCommand::Stop { done: done_tx }));

    assert!(
        timeout(Duration::from_millis(STT_ACTOR_STOP_MS - 100), &mut done_rx)
            .await
            .is_err(),
        "Stop answered before the bound"
    );
    timeout(Duration::from_millis(500), &mut done_rx)
        .await
        .expect("Stop not answered at the bound")
        .unwrap();
    timeout(Duration::from_millis(500), handle.join)
        .await
        .expect("actor did not exit")
        .unwrap();
    assert!(probe.state.lock().unwrap().stopped);
}

#[tokio::test(start_paused = true)]
async fn actor_exits_when_sender_dropped() {
    let (provider, probe) = scripted(FinalizeMode::Inline);
    let (handle, _rx) = spawn(provider, options());
    let SttActorHandle { cmd, status, join } = handle;
    drop(cmd);
    drop(status);
    timeout(Duration::from_millis(5_000), join)
        .await
        .expect("actor did not exit")
        .unwrap();
    assert!(probe.state.lock().unwrap().stopped);
}

#[tokio::test(start_paused = true)]
async fn audio_never_dropped_under_10k_commands_while_provider_blocked() {
    const CHUNKS: u32 = 10_000;
    let (mut provider, probe) = scripted(FinalizeMode::Deferred);
    provider.gate_first_push = true;
    let (handle, mut rx) = spawn(provider, options());

    let mut expected = Vec::new();
    for i in 0..CHUNKS {
        let chunk = Bytes::copy_from_slice(&i.to_le_bytes());
        expected.push(chunk.clone());
        assert!(handle.send_audio(chunk));
    }
    let total = u64::from(CHUNKS) * 4;
    assert_eq!(
        handle.status.queued_audio_bytes.load(Ordering::SeqCst),
        total
    );
    assert_eq!(handle.backlog_ms(), (total / 32) as u32);

    probe.gate.notify_one();
    let (b_tx, _b_rx) = oneshot::channel();
    assert!(handle.send(SttCommand::Barrier { done: b_tx }));
    assert!(matches!(
        next_result(&mut rx, 5_000).await,
        SttResult::BarrierReached { .. }
    ));

    assert_eq!(probe.pushed(), expected);
    assert_eq!(handle.status.queued_audio_bytes.load(Ordering::SeqCst), 0);
    assert_eq!(
        handle.status.audio_bytes_pushed.load(Ordering::SeqCst),
        total
    );
}

#[test]
fn backlog_log_once_per_episode() {
    // Below the threshold: quiet.
    assert_eq!(backlog_episode(false, 0), (false, false));
    assert_eq!(backlog_episode(false, 2_000), (false, false));
    // Crossing 2 000 ms logs once.
    assert_eq!(backlog_episode(false, 2_001), (true, true));
    // Staying above 500 ms: no more lines.
    assert_eq!(backlog_episode(true, 9_000), (false, true));
    assert_eq!(backlog_episode(true, 1_500), (false, true));
    assert_eq!(backlog_episode(true, 500), (false, true));
    // Dropping below 500 ms re-arms; the next crossing logs again.
    assert_eq!(backlog_episode(true, 499), (false, false));
    assert_eq!(backlog_episode(false, 2_500), (true, true));
}

#[tokio::test(start_paused = true)]
async fn test_final_delay_holds_final_and_marker_but_keeps_accepting_audio() {
    let (provider, probe) = scripted(FinalizeMode::Deferred);
    let (handle, mut rx) = spawn(
        provider,
        SttActorOptions {
            test_final_delay_ms: 3_000,
            ..options()
        },
    );
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    probe.finalize_requested.notified().await;
    probe.out(final_out("a"));
    probe.out(SttOutput::FinalizeDone);
    probe
        .state
        .lock()
        .unwrap()
        .push_hooks
        .push_back(partial("b"));

    let b = Bytes::from_static(b"BBBB");
    assert!(handle.send_audio(b.clone()));
    probe.pushed.notified().await;
    // The provider got the audio at once; nothing is released before the delay.
    assert_eq!(probe.pushed(), vec![b]);
    assert_quiet(&mut rx, 2_900).await;

    expect_transcript(
        next_result(&mut rx, 500).await,
        Some(1),
        0,
        SttTranscript::Final("a".into()),
    );
    expect_done(next_result(&mut rx, 500).await, 1, 0, false);
    expect_transcript(
        next_result(&mut rx, 500).await,
        None,
        0,
        SttTranscript::Partial("b".into()),
    );
}

#[tokio::test(start_paused = true)]
async fn inline_late_transcript_drained_by_tick_keeps_previous_key() {
    let (provider, probe) = scripted(FinalizeMode::Inline);
    let (handle, mut rx) = spawn(provider, options());
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    expect_done(next_result(&mut rx, 1000).await, 1, 0, false);

    probe.out(final_out("late"));
    tokio::time::advance(Duration::from_millis(20)).await;
    expect_transcript(
        next_result(&mut rx, 1000).await,
        Some(1),
        0,
        SttTranscript::Final("late".into()),
    );
}

#[tokio::test(start_paused = true)]
async fn lost_marker_after_timeout_is_dropped_after_grace() {
    let (mut provider, probe) = scripted(FinalizeMode::Deferred);
    provider.wait_ms = 1_000;
    let (handle, mut rx) = spawn(provider, options());
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    probe.finalize_requested.notified().await;

    tokio::time::advance(Duration::from_millis(1_001)).await;
    expect_done(next_result(&mut rx, 1000).await, 1, 0, true);

    tokio::time::advance(Duration::from_millis(60_000)).await;
    assert_quiet(&mut rx, 100).await;
    probe.out(partial("next"));
    // An idle actor does not poll; the released partial is drained at the next command.
    assert!(handle.send_audio(Bytes::from_static(&[1, 2, 3, 4])));
    expect_transcript(
        next_result(&mut rx, 1000).await,
        None,
        0,
        SttTranscript::Partial("next".into()),
    );
}

/// Advances paused time in small steps so the actor re-arms its poll deadline between steps.
async fn run_for(ms: u64) {
    for _ in 0..(ms / 10) {
        tokio::time::advance(Duration::from_millis(10)).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn idle_actor_does_not_poll() {
    let (provider, probe) = scripted(FinalizeMode::Deferred);
    let (handle, _rx) = spawn(provider, options());
    let polls = || probe.polls.load(Ordering::SeqCst);

    tokio::task::yield_now().await;
    run_for(5_000).await;
    assert!(polls() <= 1, "idle polls in 5 s: {}", polls());

    let before_audio = polls();
    assert!(handle.send_audio(Bytes::from_static(&[1, 2, 3, 4])));
    run_for(1_000).await;
    let busy = polls() - before_audio;
    assert!(busy >= 40, "busy polls in 1 s: {busy}");

    run_for(STT_ACTOR_BUSY_WINDOW_MS + 5_000).await;
    let before_tail = polls();
    run_for(5_000).await;
    let tail = polls() - before_tail;
    assert_eq!(tail, 0, "idle polls in the last 5 s: {tail}");
}

#[tokio::test(start_paused = true)]
async fn late_result_while_idle_is_drained_at_the_next_command() {
    let (provider, probe) = scripted(FinalizeMode::Deferred);
    let (handle, mut rx) = spawn(provider, options());
    assert!(handle.send_audio(Bytes::from_static(&[1, 2, 3, 4])));
    run_for(STT_ACTOR_BUSY_WINDOW_MS + 2_000).await;

    probe.out(partial("late"));
    run_for(5_000).await;
    assert!(rx.try_recv().is_err(), "no result is emitted while idle");

    assert!(handle.send_audio(Bytes::from_static(&[1, 2, 3, 4])));
    expect_transcript(
        next_result(&mut rx, 1_000).await,
        None,
        0,
        SttTranscript::Partial("late".into()),
    );
}

#[tokio::test(start_paused = true)]
async fn start_does_not_poll_before_first_audio() {
    let (provider, probe) = scripted(FinalizeMode::Deferred);
    probe.out(partial("queued"));
    let (handle, mut rx) = spawn(provider, options());
    let (done, started) = oneshot::channel();
    assert!(handle.send(SttCommand::Start {
        ctx: VoiceSessionContext::default(),
        done,
    }));
    started.await.unwrap().unwrap();

    run_for(2_000).await;
    assert!(
        rx.try_recv().is_err(),
        "no transcript before the first audio"
    );
    assert_eq!(probe.polls.load(Ordering::SeqCst), 0);

    assert!(handle.send_audio(Bytes::from_static(&[1, 2, 3, 4])));
    expect_transcript(
        next_result(&mut rx, 1_000).await,
        None,
        0,
        SttTranscript::Partial("queued".into()),
    );
}

#[tokio::test(start_paused = true)]
async fn inline_agent_goes_idle_after_its_last_turn() {
    let (provider, probe) = scripted(FinalizeMode::Inline);
    let (handle, mut rx) = spawn(provider, options());
    let polls = || probe.polls.load(Ordering::SeqCst);

    assert!(handle.send_audio(Bytes::from_static(&[1, 2, 3, 4])));
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    expect_done(next_result(&mut rx, 1_000).await, 1, 0, false);

    run_for(STT_ACTOR_BUSY_WINDOW_MS + 5_000).await;
    let before_tail = polls();
    run_for(5_000).await;
    let tail = polls() - before_tail;
    assert_eq!(tail, 0, "idle polls in the last 5 s: {tail}");
}

#[tokio::test(start_paused = true)]
async fn stt_latency_recorded_once_per_utterance_from_finalize_request() {
    let (provider, probe) = scripted(FinalizeMode::Deferred);
    let (opts, samples) = recording_options();
    let (handle, mut rx) = spawn(provider, opts);
    start_with_project(&handle, "proj-1").await;
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    probe.finalize_requested.notified().await;
    tokio::time::advance(Duration::from_millis(250)).await;
    probe.out(final_out("a"));
    probe.out(final_out("b"));
    probe.out(SttOutput::FinalizeDone);

    expect_transcript(
        next_result(&mut rx, 1000).await,
        Some(1),
        0,
        SttTranscript::Final("a".into()),
    );
    expect_transcript(
        next_result(&mut rx, 1000).await,
        Some(1),
        0,
        SttTranscript::Final("b".into()),
    );
    expect_done(next_result(&mut rx, 1000).await, 1, 0, false);

    let got = samples.lock().unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    let (ms, attrs) = &got[0];
    assert!(*ms >= 250.0 && *ms < 1000.0, "ms={ms}");
    assert_eq!(attrs.stt_vendor, "scripted");
    assert_eq!(attrs.stt_model, "m1");
    assert_eq!(attrs.stt_language, "en");
    assert_eq!(attrs.project_id, "proj-1");
}

#[tokio::test(start_paused = true)]
async fn stt_latency_late_final_counts_from_its_own_request() {
    let (provider, probe) = scripted(FinalizeMode::Deferred);
    let (opts, samples) = recording_options();
    let (handle, mut rx) = spawn(provider, opts);
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    probe.finalize_requested.notified().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    assert!(handle.send_audio(Bytes::from_static(&[1, 2, 3, 4])));
    assert!(handle.send(SttCommand::EndUtterance { key: 2 }));
    probe.finalize_requested.notified().await;
    tokio::time::advance(Duration::from_millis(500)).await;
    // The final of utterance 1 arrives after utterance 2 was requested.
    probe.out(final_out("one"));
    expect_transcript(
        next_result(&mut rx, 1000).await,
        Some(1),
        0,
        SttTranscript::Final("one".into()),
    );
    let got = samples.lock().unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].0 >= 600.0 && got[0].0 < 1500.0, "ms={}", got[0].0);
}

#[tokio::test(start_paused = true)]
async fn stt_latency_not_recorded_without_final() {
    let (mut provider, probe) = scripted(FinalizeMode::Deferred);
    provider.wait_ms = 1_000;
    let (opts, samples) = recording_options();
    let (handle, mut rx) = spawn(provider, opts);
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    probe.finalize_requested.notified().await;
    probe.out(SttOutput::FinalizeDone);
    expect_done(next_result(&mut rx, 1000).await, 1, 0, false);

    assert!(handle.send(SttCommand::EndUtterance { key: 2 }));
    probe.finalize_requested.notified().await;
    tokio::time::advance(Duration::from_millis(1_001)).await;
    expect_done(next_result(&mut rx, 1000).await, 2, 0, true);
    assert!(samples.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn stt_latency_attrs_follow_swap() {
    let (old, _old_probe) = scripted(FinalizeMode::Deferred);
    let (new, new_probe) = scripted(FinalizeMode::Inline);
    new_probe
        .state
        .lock()
        .unwrap()
        .on_finalize
        .push(SttTranscript::Final("hallo".into()));
    let registry = swap_registry(Some(Box::new(new)));
    let (opts, samples) = recording_options();
    let (handle, mut rx) = spawn_stt_actor(Some(Box::new(old)), registry, opts);
    start_with_project(&handle, "proj-1").await;

    let mut config = stt_config();
    config.language = Some("de".into());
    let (done_tx, done_rx) = oneshot::channel();
    assert!(handle.send(SttCommand::Swap {
        config,
        ctx: VoiceSessionContext::default(),
        running: true,
        done: done_tx,
    }));
    done_rx.await.unwrap().unwrap();
    assert!(matches!(
        next_result(&mut rx, 1000).await,
        SttResult::SwapApplied { result: Ok(()), .. }
    ));
    assert!(handle.send_audio(Bytes::from_static(&[1, 2, 3, 4])));
    assert!(handle.send(SttCommand::EndUtterance { key: 1 }));
    expect_transcript(
        next_result(&mut rx, 1000).await,
        Some(1),
        1,
        SttTranscript::Final("hallo".into()),
    );
    let got = samples.lock().unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].1.stt_language, "de");
    assert_eq!(got[0].1.stt_vendor, "mock");
    assert_eq!(got[0].1.project_id, "proj-1");
}
