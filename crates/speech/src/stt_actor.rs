//! Per-agent STT actor.
//!
//! One task owns the [`SttProvider`] of an agent and is the only caller of it. The agent sends
//! ordered [`SttCommand`]s (never awaiting, never dropping audio) and receives [`SttResult`]s with
//! every transcript attributed to the utterance it belongs to. A slow vendor therefore stalls the
//! actor only, not the inbound VAD path.
//!
//! Attribution: every `EndUtterance { key }` becomes an outstanding finalize. Transcripts polled
//! while a finalize is outstanding belong to the oldest one (its `key`); the vendor's
//! [`SttOutput::FinalizeDone`] closes it. A finalize that gets no answer in time is reported as
//! `FinalizeDone { timed_out: true }` and keeps attributing late transcripts to its key until the
//! vendor's own marker arrives (which is then swallowed).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{sleep_until, timeout, Instant};

use crate::config::{SttConfig, VoiceSessionContext};
use crate::error::{SpeechError, SpeechResult};
use crate::otel;
use crate::pipeline::{FinalizeMode, SttOutput, SttProvider, SttTranscript};
use crate::registry::VendorRegistry;

/// Longest wait for `SttProvider::stop` before the actor moves on.
pub const STT_ACTOR_STOP_MS: u64 = 3_000;
/// Longest wait for `SttProvider::start`.
pub const STT_ACTOR_START_MS: u64 = 15_000;
/// Interval at which a busy actor drains provider outputs when no command arrives.
pub const STT_ACTOR_POLL_MS: u64 = 20;
/// Interval at which an idle actor (no audio, nothing outstanding) drains provider outputs. It
/// bounds how late a stray vendor result is picked up.
pub const STT_ACTOR_IDLE_POLL_MS: u64 = 1_000;
/// How long after the last audio the actor keeps polling at [`STT_ACTOR_POLL_MS`].
pub const STT_ACTOR_BUSY_WINDOW_MS: u64 = 10_000;
/// How long a timed-out finalize keeps attributing late transcripts to its key while the
/// vendor's own marker is missing; after that the entry is dropped.
pub const STT_LATE_MARKER_GRACE_MS: u64 = 60_000;

/// Bytes of 16 kHz mono s16le PCM per millisecond.
const PCM_BYTES_PER_MS: u64 = 32;
/// Backlog (ms) above which one log line is written per episode.
const BACKLOG_LOG_ABOVE_MS: u32 = 2_000;
/// Backlog (ms) below which a new episode can be logged again.
const BACKLOG_REARM_BELOW_MS: u32 = 500;

/// Command from the agent to its STT actor. Handled strictly in send order.
#[derive(Debug)]
pub enum SttCommand {
    /// Bind the session context and start the provider.
    Start {
        ctx: VoiceSessionContext,
        done: oneshot::Sender<SpeechResult<()>>,
    },
    /// Audio for the provider: 16 kHz mono s16le, any length.
    Audio(Bytes),
    /// End the current utterance; answered by one `SttResult::FinalizeDone` with this key.
    EndUtterance { key: u64 },
    /// Replace the provider once every outstanding finalize is done. Parks later commands.
    Swap {
        config: SttConfig,
        ctx: VoiceSessionContext,
        running: bool,
        done: oneshot::Sender<SpeechResult<()>>,
    },
    /// Wait until the provider can transcribe (`true`) or the timeout passes (`false`).
    WaitReady {
        timeout: Duration,
        done: oneshot::Sender<bool>,
    },
    /// Emits `SttResult::BarrierReached` once every outstanding finalize is done.
    Barrier { done: oneshot::Sender<()> },
    /// Stop the provider (bounded) and end the actor.
    Stop { done: oneshot::Sender<()> },
}

/// Result from the STT actor to the agent, in the order the actor produced it.
#[derive(Debug)]
pub enum SttResult {
    /// A transcript. `key` is the utterance whose `EndUtterance` it answers, `None` for
    /// transcripts of audio that is still being spoken.
    Transcript {
        key: Option<u64>,
        gen: u64,
        transcript: SttTranscript,
    },
    /// The utterance `key` is closed. `timed_out` when the vendor did not answer in time.
    FinalizeDone { key: u64, gen: u64, timed_out: bool },
    /// The vendor failed. `key` is the utterance it concerns, when known.
    VendorError {
        key: Option<u64>,
        gen: u64,
        message: String,
    },
    /// A `Swap` finished (applied or refused).
    SwapApplied {
        config: SttConfig,
        result: Result<(), String>,
    },
    /// A `Barrier` was reached; the agent answers `done` after handling earlier results.
    BarrierReached { done: oneshot::Sender<()> },
}

/// Live counters the agent reads without a round trip.
#[derive(Default)]
pub struct SttActorStatus {
    /// Audio sent via [`SttActorHandle::send_audio`] and not yet pushed to the provider.
    pub queued_audio_bytes: AtomicU64,
    /// Total bytes pushed to the provider (tests).
    pub audio_bytes_pushed: AtomicU64,
    /// `SttProvider::decode_backlog_ms`.
    pub vendor_backlog_ms: AtomicU32,
    /// `SttProvider::stream_open_pending`.
    pub open_pending: AtomicBool,
    /// Finalizes still waiting for the vendor (timed-out ones are not counted).
    pub outstanding_finalizes: AtomicUsize,
    /// Bumped by every applied `Swap`; starts at 0.
    pub generation: AtomicU64,
}

/// Agent-side handle to a running STT actor.
pub struct SttActorHandle {
    pub cmd: mpsc::UnboundedSender<SttCommand>,
    pub status: Arc<SttActorStatus>,
    pub join: JoinHandle<()>,
}

impl SttActorHandle {
    /// Counts the bytes in `status.queued_audio_bytes` before sending (never awaits, never drops).
    /// Returns false only when the actor has exited.
    pub fn send_audio(&self, pcm: Bytes) -> bool {
        let len = pcm.len() as u64;
        self.status
            .queued_audio_bytes
            .fetch_add(len, Ordering::SeqCst);
        if self.cmd.send(SttCommand::Audio(pcm)).is_ok() {
            true
        } else {
            saturating_sub(&self.status.queued_audio_bytes, len);
            false
        }
    }

    /// Unbounded send; returns false only when the actor has exited.
    pub fn send(&self, cmd: SttCommand) -> bool {
        self.cmd.send(cmd).is_ok()
    }

    /// Queued audio (32 bytes per ms at 16 kHz mono s16le) plus the vendor's own backlog.
    pub fn backlog_ms(&self) -> u32 {
        backlog_ms_of(&self.status)
    }
}

/// Options for [`spawn_stt_actor`].
#[derive(Debug, Clone, Copy)]
pub struct SttActorOptions {
    /// `VadConfig::stt_open_wait_max_ms` (default 120_000).
    pub open_wait_max_ms: u64,
    /// Test-only delay of finals and finalize markers; see [`test_final_delay_ms_from_env`].
    pub test_final_delay_ms: u64,
}

/// `SPEECH_STT_TEST_FINAL_DELAY_MS` (test-only), default 0. Read once.
pub fn test_final_delay_ms_from_env() -> u64 {
    static VALUE: OnceLock<u64> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("SPEECH_STT_TEST_FINAL_DELAY_MS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0)
    })
}

/// Decides when the backlog line is logged. Returns `(log_now, logged)`: logs once when the
/// backlog goes above 2 000 ms, stays quiet while it remains above 500 ms, and re-arms below it.
pub fn backlog_episode(prev_logged: bool, ms: u32) -> (bool, bool) {
    if !prev_logged && ms > BACKLOG_LOG_ABOVE_MS {
        (true, true)
    } else if prev_logged && ms < BACKLOG_REARM_BELOW_MS {
        (false, false)
    } else {
        (false, prev_logged)
    }
}

fn saturating_sub(counter: &AtomicU64, len: u64) {
    let mut current = counter.load(Ordering::SeqCst);
    loop {
        match counter.compare_exchange_weak(
            current,
            current.saturating_sub(len),
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => break,
            Err(seen) => current = seen,
        }
    }
}

fn backlog_ms_of(status: &SttActorStatus) -> u32 {
    let queued_ms = status.queued_audio_bytes.load(Ordering::SeqCst) / PCM_BYTES_PER_MS;
    let queued_ms = u32::try_from(queued_ms).unwrap_or(u32::MAX);
    queued_ms.saturating_add(status.vendor_backlog_ms.load(Ordering::SeqCst))
}

/// Spawns the actor on the current tokio runtime. It owns `provider`, the command receiver and the
/// result sender; it exits after `Stop`, or when every command sender is dropped (the provider is
/// stopped, bounded by [`STT_ACTOR_STOP_MS`], first).
pub fn spawn_stt_actor(
    provider: Option<Box<dyn SttProvider>>,
    registry: Arc<VendorRegistry>,
    options: SttActorOptions,
) -> (SttActorHandle, mpsc::UnboundedReceiver<SttResult>) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (res_tx, res_rx) = mpsc::unbounded_channel();
    let status = Arc::new(SttActorStatus::default());
    let actor = Actor {
        provider,
        registry,
        options,
        status: Arc::clone(&status),
        results: res_tx,
        outstanding: VecDeque::new(),
        inline_late_key: None,
        gen: 0,
        delayed: VecDeque::new(),
        parked: None,
        last_audio_at: None,
        backlog_logged: false,
        last_open_pending: false,
    };
    let join = tokio::spawn(actor.run(cmd_rx));
    (
        SttActorHandle {
            cmd: cmd_tx,
            status,
            join,
        },
        res_rx,
    )
}

/// One `EndUtterance` waiting for the vendor's answer.
struct Outstanding {
    key: u64,
    gen: u64,
    requested_at: Instant,
    deadline_base_ms: u64,
    timed_out: bool,
    timed_out_at: Option<Instant>,
}

struct Actor {
    provider: Option<Box<dyn SttProvider>>,
    registry: Arc<VendorRegistry>,
    options: SttActorOptions,
    status: Arc<SttActorStatus>,
    results: mpsc::UnboundedSender<SttResult>,
    outstanding: VecDeque<Outstanding>,
    /// Key of the last Inline finalize; transcripts still queued in the provider when the next
    /// audio arrives belong to it.
    inline_late_key: Option<u64>,
    gen: u64,
    /// Test-only FIFO of results held back by `test_final_delay_ms`.
    delayed: VecDeque<(Instant, SttResult)>,
    /// A `Swap` or `Barrier` waiting for the outstanding finalizes to finish.
    parked: Option<SttCommand>,
    /// When the last `Audio` command was handled; keeps the actor busy for a while after it.
    last_audio_at: Option<Instant>,
    backlog_logged: bool,
    /// `open_pending()` as seen by the last `observe_open_transition` call.
    last_open_pending: bool,
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<SttCommand>) {
        let mut next_poll = Instant::now();
        loop {
            let now = Instant::now();
            let due = self.delayed.front().map(|(at, _)| *at);
            let deadline = self.next_deadline();
            let can_read = self.parked.is_none();
            tokio::select! {
                biased;
                _ = sleep_until(due.unwrap_or(now)), if due.is_some() => {
                    self.release_due();
                }
                _ = sleep_until(deadline.unwrap_or(now)), if deadline.is_some() => {
                    self.expire_deadlines();
                }
                cmd = rx.recv(), if can_read => {
                    match cmd {
                        None => {
                            self.stop_provider().await;
                            break;
                        }
                        Some(cmd) => {
                            let was_idle = !self.busy();
                            if !self.handle_command(cmd).await {
                                break;
                            }
                            if was_idle {
                                // A burst of audio is polled at the busy cadence at once.
                                next_poll = Instant::now() + Duration::from_millis(STT_ACTOR_POLL_MS);
                            }
                        }
                    }
                }
                // Idle agents poll once per second so a late vendor result is still drained;
                // busy agents poll every 20 ms.
                _ = sleep_until(next_poll) => {
                    self.drain_outputs(self.inline_late_key).await;
                    self.check_backlog(true);
                    let period = if self.busy() { STT_ACTOR_POLL_MS } else { STT_ACTOR_IDLE_POLL_MS };
                    next_poll = Instant::now() + Duration::from_millis(period);
                }
            }
            self.run_parked().await;
            self.publish_status();
        }
    }

    fn vendor_name(&self) -> &'static str {
        self.provider
            .as_ref()
            .map(|p| p.vendor_name())
            .unwrap_or("none")
    }

    /// True while something can still produce output soon: a finalize, a late result, parked or
    /// delayed work, recent audio, a pending stream open or a vendor decode backlog.
    fn busy(&self) -> bool {
        let recent_audio = self
            .last_audio_at
            .is_some_and(|at| at.elapsed() < Duration::from_millis(STT_ACTOR_BUSY_WINDOW_MS));
        let vendor_backlog = self
            .provider
            .as_ref()
            .is_some_and(|p| p.decode_backlog_ms() > 0);
        !self.outstanding.is_empty()
            || self.inline_late_key.is_some()
            || self.parked.is_some()
            || !self.delayed.is_empty()
            || recent_audio
            || self.open_pending()
            || vendor_backlog
    }

    fn open_pending(&self) -> bool {
        self.provider
            .as_ref()
            .is_some_and(|p| p.stream_open_pending())
    }

    fn publish_status(&mut self) {
        self.observe_open_transition();
        let backlog = self
            .provider
            .as_ref()
            .map(|p| p.decode_backlog_ms())
            .unwrap_or(0);
        self.status
            .vendor_backlog_ms
            .store(backlog, Ordering::SeqCst);
        self.status
            .open_pending
            .store(self.open_pending(), Ordering::SeqCst);
        let waiting = self.outstanding.iter().filter(|o| !o.timed_out).count();
        self.status
            .outstanding_finalizes
            .store(waiting, Ordering::SeqCst);
    }

    // ---- results -------------------------------------------------------------------------

    fn emit(&mut self, result: SttResult) {
        let delay = self.options.test_final_delay_ms;
        if delay == 0 {
            let _ = self.results.send(result);
            return;
        }
        let holds = matches!(
            result,
            SttResult::Transcript {
                transcript: SttTranscript::Final(_),
                ..
            } | SttResult::FinalizeDone { .. }
        );
        let own = Instant::now() + Duration::from_millis(if holds { delay } else { 0 });
        let at = match self.delayed.back() {
            Some((prev, _)) if *prev > own => *prev,
            _ => own,
        };
        self.delayed.push_back((at, result));
    }

    fn release_due(&mut self) {
        let now = Instant::now();
        while self.delayed.front().is_some_and(|(at, _)| *at <= now) {
            if let Some((_, result)) = self.delayed.pop_front() {
                let _ = self.results.send(result);
            }
        }
    }

    fn front_key_gen(&self) -> (Option<u64>, u64) {
        match self.outstanding.front() {
            Some(o) => (Some(o.key), o.gen),
            None => (None, self.gen),
        }
    }

    fn emit_transcript(&mut self, transcript: SttTranscript, fallback_key: Option<u64>) {
        let (key, gen) = self.front_key_gen();
        self.emit(SttResult::Transcript {
            key: key.or(fallback_key),
            gen,
            transcript,
        });
    }

    fn emit_vendor_error(&mut self, key: Option<u64>, message: String) {
        let gen = self.gen;
        self.emit(SttResult::VendorError { key, gen, message });
    }

    // ---- outputs -------------------------------------------------------------------------

    /// Polls the provider until it has nothing more and attributes the outputs. `fallback_key`
    /// is used for transcripts when no finalize is outstanding.
    async fn drain_outputs(&mut self, fallback_key: Option<u64>) {
        let mut items = Vec::new();
        let mut error = None;
        if let Some(provider) = self.provider.as_mut() {
            loop {
                match provider.poll_output().await {
                    Ok(Some(item)) => items.push(item),
                    Ok(None) => break,
                    Err(e) => {
                        error = Some(e.to_string());
                        break;
                    }
                }
            }
        }
        for item in items {
            match item {
                SttOutput::Transcript(t) => self.emit_transcript(t, fallback_key),
                SttOutput::FinalizeDone => {
                    if let Some(done) = self.outstanding.pop_front() {
                        if !done.timed_out {
                            self.emit(SttResult::FinalizeDone {
                                key: done.key,
                                gen: done.gen,
                                timed_out: false,
                            });
                        }
                    }
                }
            }
        }
        if let Some(message) = error {
            let (key, _) = self.front_key_gen();
            self.emit_vendor_error(key, message);
        }
    }

    // ---- deadlines -----------------------------------------------------------------------

    /// Restarts the finalize wait of every outstanding entry when a pending stream open completes.
    ///
    /// Catalog `rt.stt-terminal-before-late-final` / `infra.unbounded-external-wait`. While the
    /// open is refused or slow, the wait is bounded by `open_wait_max_ms`. After the open
    /// succeeds the vendor gets its normal finalize wait (recomputed with the audio queued now)
    /// counted from that moment. Without this, the base deadline (counted from the request)
    /// is long past when the flip happens, and every queued finalize expires before the vendor
    /// can answer the Finalize that was waiting behind the open.
    ///
    /// Called before every expiry decision so a flip is always seen first.
    fn observe_open_transition(&mut self) {
        let now_pending = self.open_pending();
        if self.last_open_pending && !now_pending {
            if let Some(base) = self.provider.as_ref().map(|p| p.finalize_wait_ms()) {
                let now = Instant::now();
                for entry in self.outstanding.iter_mut().filter(|o| !o.timed_out) {
                    entry.requested_at = now;
                    entry.deadline_base_ms = base;
                }
            }
        }
        self.last_open_pending = now_pending;
    }

    fn deadline_of(&self, o: &Outstanding) -> Instant {
        let base = o.requested_at + Duration::from_millis(o.deadline_base_ms);
        if self.open_pending() {
            base.max(o.requested_at + Duration::from_millis(self.options.open_wait_max_ms))
        } else {
            base
        }
    }

    fn grace_expiry_of_front(&self) -> Option<Instant> {
        self.outstanding
            .front()
            .and_then(|o| o.timed_out_at)
            .map(|at| at + Duration::from_millis(STT_LATE_MARKER_GRACE_MS))
    }

    fn next_deadline(&mut self) -> Option<Instant> {
        self.observe_open_transition();
        let finalize = self
            .outstanding
            .iter()
            .find(|o| !o.timed_out)
            .map(|o| self.deadline_of(o));
        match (finalize, self.grace_expiry_of_front()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn expire_deadlines(&mut self) {
        self.observe_open_transition();
        let now = Instant::now();
        // A timed-out front entry whose marker never arrived stops attributing transcripts.
        while self.grace_expiry_of_front().is_some_and(|at| at <= now) {
            if let Some(dropped) = self.outstanding.pop_front() {
                eprintln!(
                    "[speech] stt finalize marker for {} never arrived within {} ms after its timeout (vendor={}); attribution moves on",
                    dropped.key,
                    STT_LATE_MARKER_GRACE_MS,
                    self.vendor_name()
                );
            }
        }
        while let Some(idx) = self.outstanding.iter().position(|o| !o.timed_out) {
            if self.deadline_of(&self.outstanding[idx]) > now {
                break;
            }
            let entry = &mut self.outstanding[idx];
            entry.timed_out = true;
            entry.timed_out_at = Some(now);
            let (key, gen, waited) = (entry.key, entry.gen, entry.requested_at.elapsed());
            eprintln!(
                "[speech] stt final for {key} not received within {} ms (vendor={}); closing the utterance",
                waited.as_millis(),
                self.vendor_name()
            );
            self.emit(SttResult::FinalizeDone {
                key,
                gen,
                timed_out: true,
            });
        }
    }

    // ---- backlog -------------------------------------------------------------------------

    fn check_backlog(&mut self, record_gauge: bool) {
        let vendor_ms = self
            .provider
            .as_ref()
            .map(|p| p.decode_backlog_ms())
            .unwrap_or(0);
        self.status
            .vendor_backlog_ms
            .store(vendor_ms, Ordering::SeqCst);
        let ms = backlog_ms_of(&self.status);
        let (log_now, logged) = backlog_episode(self.backlog_logged, ms);
        self.backlog_logged = logged;
        if log_now {
            eprintln!(
                "[speech] stt actor behind by {ms} ms of audio (vendor={}); queued, not dropped",
                self.vendor_name()
            );
        }
        if record_gauge {
            otel::record_voice_stt_actor_backlog_ms(u64::from(ms), self.vendor_name());
        }
    }

    // ---- provider lifecycle --------------------------------------------------------------

    async fn stop_provider(&mut self) {
        if let Some(provider) = self.provider.as_mut() {
            stop_bounded(provider.as_mut()).await;
        }
    }

    // ---- commands ------------------------------------------------------------------------

    /// Returns false when the actor must exit.
    async fn handle_command(&mut self, cmd: SttCommand) -> bool {
        match cmd {
            SttCommand::Start { ctx, done } => {
                let result = match self.provider.as_mut() {
                    Some(provider) => {
                        provider.bind_session_context(&ctx);
                        start_bounded(provider.as_mut()).await
                    }
                    None => Ok(()),
                };
                let _ = done.send(result);
            }
            SttCommand::Audio(pcm) => self.on_audio(pcm).await,
            SttCommand::EndUtterance { key } => self.on_end_utterance(key).await,
            SttCommand::WaitReady { timeout, done } => {
                let ready = match self.provider.as_mut() {
                    Some(provider) => provider.wait_ready(timeout).await.unwrap_or(false),
                    None => true,
                };
                let _ = done.send(ready);
            }
            cmd @ (SttCommand::Swap { .. } | SttCommand::Barrier { .. }) => {
                self.parked = Some(cmd);
            }
            SttCommand::Stop { done } => {
                self.stop_provider().await;
                let _ = done.send(());
                return false;
            }
        }
        true
    }

    async fn on_audio(&mut self, pcm: Bytes) {
        let len = pcm.len() as u64;
        self.last_audio_at = Some(Instant::now());
        if let Some(key) = self.inline_late_key.take() {
            self.drain_outputs(Some(key)).await;
        }
        let mut errors = Vec::new();
        let mut transcripts = Vec::new();
        if let Some(provider) = self.provider.as_mut() {
            match provider.finalize_mode() {
                FinalizeMode::Inline => {
                    if let Err(e) = provider.push_and_poll(pcm, &mut transcripts).await {
                        errors.push(e.to_string());
                    }
                }
                FinalizeMode::Deferred => {
                    if let Err(e) = provider.push_audio(pcm).await {
                        errors.push(e.to_string());
                    }
                }
            }
        }
        saturating_sub(&self.status.queued_audio_bytes, len);
        self.status
            .audio_bytes_pushed
            .fetch_add(len, Ordering::SeqCst);
        for t in transcripts {
            self.emit_transcript(t, None);
        }
        for message in errors {
            self.emit_vendor_error(None, message);
        }
        if self
            .provider
            .as_ref()
            .is_some_and(|p| p.finalize_mode() == FinalizeMode::Deferred)
        {
            self.drain_outputs(None).await;
        }
        self.check_backlog(false);
    }

    async fn on_end_utterance(&mut self, key: u64) {
        let gen = self.gen;
        let Some(provider) = self.provider.as_mut() else {
            self.emit(SttResult::FinalizeDone {
                key,
                gen,
                timed_out: false,
            });
            return;
        };
        let wait_ms = provider.finalize_wait_ms();
        let entry = Outstanding {
            key,
            gen,
            requested_at: Instant::now(),
            deadline_base_ms: wait_ms,
            timed_out: false,
            timed_out_at: None,
        };
        match provider.finalize_mode() {
            FinalizeMode::Deferred => {
                let result = provider.request_finalize().await;
                self.outstanding.push_back(entry);
                match result {
                    Ok(()) => self.drain_outputs(None).await,
                    Err(e) => {
                        self.outstanding.pop_back();
                        self.emit_vendor_error(Some(key), e.to_string());
                        self.emit(SttResult::FinalizeDone {
                            key,
                            gen,
                            timed_out: false,
                        });
                    }
                }
            }
            FinalizeMode::Inline => {
                let vendor = provider.vendor_name();
                let result = timeout(
                    Duration::from_millis(wait_ms),
                    provider.finalize_utterance(),
                )
                .await;
                self.outstanding.push_back(entry);
                match result {
                    Ok(Ok(())) => {
                        self.drain_outputs(None).await;
                        self.outstanding.pop_back();
                        self.emit(SttResult::FinalizeDone {
                            key,
                            gen,
                            timed_out: false,
                        });
                    }
                    Ok(Err(e)) => {
                        self.outstanding.pop_back();
                        self.emit_vendor_error(Some(key), e.to_string());
                        self.emit(SttResult::FinalizeDone {
                            key,
                            gen,
                            timed_out: false,
                        });
                    }
                    Err(_) => {
                        eprintln!(
                            "[speech] stt final for {key} not received within {wait_ms} ms (vendor={vendor}); closing the utterance"
                        );
                        self.outstanding.pop_back();
                        self.emit(SttResult::FinalizeDone {
                            key,
                            gen,
                            timed_out: true,
                        });
                    }
                }
                self.inline_late_key = Some(key);
            }
        }
    }

    // ---- parked commands -----------------------------------------------------------------

    async fn run_parked(&mut self) {
        if self.parked.is_none() || self.outstanding.iter().any(|o| !o.timed_out) {
            return;
        }
        match self.parked.take() {
            Some(SttCommand::Barrier { done }) => self.emit(SttResult::BarrierReached { done }),
            Some(SttCommand::Swap {
                config,
                ctx,
                running,
                done,
            }) => self.run_swap(config, ctx, running, done).await,
            _ => {}
        }
    }

    async fn run_swap(
        &mut self,
        config: SttConfig,
        ctx: VoiceSessionContext,
        running: bool,
        done: oneshot::Sender<SpeechResult<()>>,
    ) {
        let mut new = match self.registry.create_stt(&config) {
            Ok(p) => p,
            Err(e) => {
                self.refuse_swap(config, e, done);
                return;
            }
        };
        new.bind_session_context(&ctx);
        if running {
            if let Err(e) = start_bounded(new.as_mut()).await {
                self.refuse_swap(config, e, done);
                return;
            }
        }
        if let Some(old) = self.provider.as_mut() {
            stop_bounded(old.as_mut()).await;
        }
        self.provider = Some(new);
        self.gen += 1;
        self.status.generation.store(self.gen, Ordering::SeqCst);
        self.outstanding.clear();
        self.inline_late_key = None;
        self.emit(SttResult::SwapApplied {
            config,
            result: Ok(()),
        });
        let _ = done.send(Ok(()));
    }

    fn refuse_swap(
        &mut self,
        config: SttConfig,
        error: SpeechError,
        done: oneshot::Sender<SpeechResult<()>>,
    ) {
        self.emit(SttResult::SwapApplied {
            config,
            result: Err(error.to_string()),
        });
        let _ = done.send(Err(error));
    }
}

async fn start_bounded(provider: &mut dyn SttProvider) -> SpeechResult<()> {
    match timeout(Duration::from_millis(STT_ACTOR_START_MS), provider.start()).await {
        Ok(result) => result,
        Err(_) => Err(SpeechError::Stt("stt start timed out".into())),
    }
}

async fn stop_bounded(provider: &mut dyn SttProvider) {
    match timeout(Duration::from_millis(STT_ACTOR_STOP_MS), provider.stop()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("[speech] stt provider stop failed: {e}"),
        Err(_) => eprintln!("[speech] stt provider stop did not finish within 3000 ms"),
    }
}
