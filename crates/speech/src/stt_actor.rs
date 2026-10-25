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
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{sleep_until, timeout, Instant};

use crate::audio_tap::UtteranceTap;
use crate::config::{SttConfig, VoiceSessionContext};
use crate::error::{SpeechError, SpeechResult};
use crate::otel::{self, SttMetricAttrs};
use crate::pipeline::{FinalizeMode, SttOutput, SttProvider, SttTranscript};
use crate::registry::VendorRegistry;

/// Longest wait for `SttProvider::stop` before the actor moves on.
pub const STT_ACTOR_STOP_MS: u64 = 3_000;
/// Longest wait for `SttProvider::start`.
pub const STT_ACTOR_START_MS: u64 = 15_000;
/// Wait after a failed reopen of a stream the vendor closed, before the next attempt.
pub const STT_REOPEN_BACKOFF_MS: u64 = 2_000;
/// Interval at which a busy actor drains provider outputs when no command arrives. An idle actor
/// (nothing outstanding, no work for [`STT_ACTOR_BUSY_WINDOW_MS`]) does not poll at all: it
/// sleeps until a command or a deadline arrives, and drains leftovers at the next command.
pub const STT_ACTOR_POLL_MS: u64 = 20;
/// How long after the last audio, finalize request or provider output the actor keeps polling at
/// [`STT_ACTOR_POLL_MS`].
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
    /// `SttProvider::emits_partials` of the active provider. `true` until a provider has been
    /// started; stored after every successful start and every applied `Swap`.
    pub emits_partials: AtomicBool,
}

impl Default for SttActorStatus {
    fn default() -> Self {
        Self {
            queued_audio_bytes: AtomicU64::new(0),
            audio_bytes_pushed: AtomicU64::new(0),
            vendor_backlog_ms: AtomicU32::new(0),
            open_pending: AtomicBool::new(false),
            outstanding_finalizes: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
            emits_partials: AtomicBool::new(true),
        }
    }
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

    /// Whether the active provider emits `SttTranscript::Partial` (`true` before the first start).
    pub fn emits_partials(&self) -> bool {
        self.status.emits_partials.load(Ordering::SeqCst)
    }
}

/// Receives one STT latency sample (ms from finalize request to the first final) per utterance.
pub type SttLatencySink = Arc<dyn Fn(f64, &SttMetricAttrs) + Send + Sync>;

/// The production sink: records `voice_stt_latency_ms`.
// The explicit call keeps the `record_*` call site visible to `otel_record_fns_have_callers_test`.
#[allow(clippy::redundant_closure)]
pub fn default_stt_latency_sink() -> SttLatencySink {
    Arc::new(|ms, attrs| otel::record_stt_latency_ms(ms, attrs))
}

/// Options for [`spawn_stt_actor`].
#[derive(Clone)]
pub struct SttActorOptions {
    /// `VadConfig::stt_open_wait_max_ms` (default 120_000).
    pub open_wait_max_ms: u64,
    /// Test-only delay of finals and finalize markers; see [`test_final_delay_ms_from_env`].
    pub test_final_delay_ms: u64,
    /// Test-only: directory for the STT input tap (see [`crate::audio_tap`]); `None` = off.
    pub audio_tap_dir: Option<&'static Path>,
    /// Test-only: agent id used in the tap file names (see [`crate::audio_tap`]).
    pub audio_tap_agent_id: u64,
    /// Initial vendor/model/language/project attributes of the latency samples.
    pub metric_attrs: SttMetricAttrs,
    /// Where latency samples go ([`default_stt_latency_sink`] in production).
    pub stt_latency_sink: SttLatencySink,
}

impl std::fmt::Debug for SttActorOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SttActorOptions")
            .field("open_wait_max_ms", &self.open_wait_max_ms)
            .field("test_final_delay_ms", &self.test_final_delay_ms)
            .field("audio_tap_dir", &self.audio_tap_dir)
            .field("audio_tap_agent_id", &self.audio_tap_agent_id)
            .field("metric_attrs", &self.metric_attrs)
            .field("stt_latency_sink", &"<sink>")
            .finish()
    }
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
    let tap = options
        .audio_tap_dir
        .map(|d| UtteranceTap::new(d, options.audio_tap_agent_id, "stt-in", 16_000, 1));
    let metric_attrs = options.metric_attrs.clone();
    let actor = Actor {
        provider,
        registry,
        options,
        metric_attrs,
        tap,
        status: Arc::clone(&status),
        results: res_tx,
        outstanding: VecDeque::new(),
        inline_late_key: None,
        gen: 0,
        delayed: VecDeque::new(),
        parked: None,
        last_work_at: None,
        backlog_logged: false,
        reopen_not_before: None,
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
    /// When the finalize was requested; unlike `requested_at` it is never reset.
    finalize_requested_at: Instant,
    /// Set once the latency sample of this utterance has been sent.
    latency_recorded: bool,
    deadline_base_ms: u64,
    timed_out: bool,
    timed_out_at: Option<Instant>,
}

struct Actor {
    provider: Option<Box<dyn SttProvider>>,
    registry: Arc<VendorRegistry>,
    options: SttActorOptions,
    /// Attributes of the latency samples; follows `Start` (project) and applied swaps.
    metric_attrs: SttMetricAttrs,
    /// Test-only copy of the audio pushed to the provider (see [`crate::audio_tap`]).
    tap: Option<UtteranceTap>,
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
    /// When the last `Audio` or `EndUtterance` command was handled, or the provider last produced
    /// an output; keeps the actor busy for a while after it.
    last_work_at: Option<Instant>,
    backlog_logged: bool,
    /// After a failed reopen of a dead stream: no new attempt before this time.
    reopen_not_before: Option<Instant>,
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
            let busy = self.busy();
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
                            self.flush_tap("stop");
                            self.stop_provider().await;
                            break;
                        }
                        Some(cmd) => {
                            if !busy
                                && self.last_work_at.is_some()
                                && matches!(
                                    cmd,
                                    SttCommand::Audio(_) | SttCommand::EndUtterance { .. }
                                )
                            {
                                // Leftovers from the quiet period are attributed before new
                                // audio or a new finalize. A fresh actor has nothing to
                                // attribute, and the other commands produce no transcripts.
                                self.drain_outputs(self.inline_late_key).await;
                            }
                            if !self.handle_command(cmd).await {
                                break;
                            }
                            next_poll = Instant::now() + Duration::from_millis(STT_ACTOR_POLL_MS);
                        }
                    }
                }
                // Busy actors poll every 20 ms. An idle actor has no timer at all: it sleeps until
                // a command or a deadline arrives.
                _ = sleep_until(next_poll), if busy => {
                    self.drain_outputs(self.inline_late_key).await;
                    self.check_backlog(true);
                    next_poll = Instant::now() + Duration::from_millis(STT_ACTOR_POLL_MS);
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

    /// True while something can still produce output soon: a finalize, parked or delayed work,
    /// a pending stream open, a vendor decode backlog, or audio, a finalize request or an output
    /// within the last [`STT_ACTOR_BUSY_WINDOW_MS`]. An actor that is not busy does not poll.
    fn busy(&self) -> bool {
        let recent_work = self
            .last_work_at
            .is_some_and(|at| at.elapsed() < Duration::from_millis(STT_ACTOR_BUSY_WINDOW_MS));
        let vendor_backlog = self
            .provider
            .as_ref()
            .is_some_and(|p| p.decode_backlog_ms() > 0);
        !self.outstanding.is_empty()
            || self.parked.is_some()
            || !self.delayed.is_empty()
            || recent_work
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
        if matches!(transcript, SttTranscript::Final(_)) {
            let mut sample = None;
            if let Some(front) = self.outstanding.front_mut() {
                if !front.latency_recorded {
                    front.latency_recorded = true;
                    sample = Some(front.finalize_requested_at.elapsed().as_secs_f64() * 1000.0);
                }
            }
            if let Some(ms) = sample {
                (self.options.stt_latency_sink)(ms, &self.metric_attrs);
            }
        }
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
        if !items.is_empty() {
            self.last_work_at = Some(Instant::now());
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
                if let Some(id) = ctx.project_id.as_deref().filter(|id| !id.is_empty()) {
                    self.metric_attrs.project_id = id.to_string();
                }
                let result = match self.provider.as_mut() {
                    Some(provider) => {
                        provider.bind_session_context(&ctx);
                        let started = start_bounded(provider.as_mut()).await;
                        if started.is_ok() {
                            self.status
                                .emits_partials
                                .store(provider.emits_partials(), Ordering::SeqCst);
                        }
                        started
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
                self.flush_tap("stop");
                self.stop_provider().await;
                let _ = done.send(());
                return false;
            }
        }
        true
    }

    /// The vendor closes a stream that got no audio for a while (the gate is closed between
    /// utterances). Reopens it before the first audio of the next utterance goes out, with the
    /// open marked pending so the agent's no-partial window (C1) waits for it.
    /// One failed attempt blocks the next for [`STT_REOPEN_BACKOFF_MS`]; the audio of that gap is
    /// lost and the failure is reported once.
    async fn reopen_dead_stream(&mut self) {
        let now = Instant::now();
        let dead = self.provider.as_ref().is_some_and(|p| !p.stream_alive());
        // A finalize still outstanding means the stream died mid-utterance: its markers and the
        // loss error are queued in the provider, and `start` would clear them.
        if !dead
            || !self.outstanding.is_empty()
            || self.reopen_not_before.is_some_and(|at| at > now)
        {
            return;
        }
        // Whatever the dead stream still holds (an idle close queues nothing) is delivered first.
        self.drain_outputs(None).await;
        let Some(provider) = self.provider.as_mut() else {
            return;
        };
        let vendor = provider.vendor_name();
        self.status.open_pending.store(true, Ordering::SeqCst);
        let started_at = Instant::now();
        let result = start_bounded(provider.as_mut()).await;
        match result {
            Ok(()) => {
                self.reopen_not_before = None;
                eprintln!(
                    "[speech] stt stream closed by the vendor while idle; reopened in {} ms (vendor={vendor})",
                    started_at.elapsed().as_millis()
                );
            }
            Err(e) => {
                self.reopen_not_before =
                    Some(Instant::now() + Duration::from_millis(STT_REOPEN_BACKOFF_MS));
                eprintln!(
                    "[speech] stt stream closed by the vendor while idle; reopen failed after {} ms (vendor={vendor})",
                    started_at.elapsed().as_millis()
                );
                self.emit_vendor_error(None, e.to_string());
            }
        }
        self.publish_status();
    }

    fn flush_tap(&mut self, label: &str) {
        if let Some(tap) = self.tap.as_mut() {
            tap.flush(label);
        }
    }

    async fn on_audio(&mut self, pcm: Bytes) {
        let len = pcm.len() as u64;
        if let Some(tap) = self.tap.as_mut() {
            tap.push(&pcm);
        }
        self.last_work_at = Some(Instant::now());
        self.reopen_dead_stream().await;
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
        self.flush_tap(&format!("utt{key}"));
        self.last_work_at = Some(Instant::now());
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
        let now = Instant::now();
        let entry = Outstanding {
            key,
            gen,
            requested_at: now,
            finalize_requested_at: now,
            latency_recorded: false,
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
        self.status
            .emits_partials
            .store(new.emits_partials(), Ordering::SeqCst);
        let project_id = ctx
            .project_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .unwrap_or(&self.metric_attrs.project_id)
            .to_string();
        self.metric_attrs = SttMetricAttrs::from_stt_config(&config, &project_id);
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
