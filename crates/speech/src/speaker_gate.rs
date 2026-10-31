//! Speaker gate: learns the caller's voice from their first utterances and checks every later
//! utterance against it.
//!
//! Everything slow runs off the frame path. The agent only calls the cheap synchronous feed
//! methods ([`SpeakerGate::begin`], [`SpeakerGate::push_frame`], [`SpeakerGate::end_utterance`]);
//! PCM for enrollment and checks goes over an unbounded channel to one per-agent task that owns
//! the embedder. Results come back keyed by utterance key and are applied only to that utterance
//! (`rt.stale-event-by-key`). Every wait is bounded (`infra.unbounded-external-wait`), and a
//! timeout fails open (`unknown`): a stuck gate must never mute the real caller.
//!
//! Consecutive re-checks (`recheck_on_other`, default on): while the stored verdict is `other` and
//! the utterance keeps going, every further `check_speech_ms` of speech is scored (the latest
//! window), up to `recheck_max_windows` windows per utterance (the first check included; `2` is
//! one re-check, `0` has no cap). A re-check can only upgrade `other` to `caller`; it never runs
//! after a `caller` or `unknown` first verdict, and it never downgrades one. This covers a
//! bystander (TV, colleague) who opens the VAD before the caller and runs into the caller's words
//! without a pause. A window is cut only when the previous one has settled, never queued.
//!
//! Embeddings, the reference vector and PCM are never logged, emitted or exported. Scores appear
//! only in `VOICE_DEBUG` output, as a number.

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Notify;

use crate::config::SpeakerGateConfig;
use crate::pipeline::SpeakerEmbedder;

/// Mono s16le at 16 kHz.
const BYTES_PER_MS: usize = 32;
/// Audio kept before SpeechStart so a check includes the onset (the STT pre-roll equivalent).
const PRE_ROLL_MS: usize = 300;
/// Verdicts and "check sent" marks kept per agent.
const BOOK_CAP: usize = 32;
/// Ended utterances whose PCM is kept for enrollment until their final arrives.
const ENDED_CAP: usize = 4;
/// Pending channel messages above which a backlog warning is logged (once per episode).
const BACKLOG_WARN: usize = 8;

/// Default for [`SpeakerGateConfig::recheck_max_windows`]: windows scored per utterance, the first
/// check included (12 s of speech with the default 1.5 s check).
pub const DEFAULT_RECHECK_MAX_WINDOWS: u32 = 8;

/// Earliest speech time (ms of speech in the utterance) at which a barge-in released by the
/// `window`-th scored window (1 = first check) can fire. SDK mirror:
/// `speakerRecheckBargeEarliestMs(checkSpeechMs, window)`
/// (`packages/sdk/src/voice/defaults.ts`), which the noisy-bystander roundtrip oracle uses.
pub fn recheck_barge_earliest_ms(check_speech_ms: u32, window: u32) -> u32 {
    check_speech_ms.saturating_mul(window)
}

/// Who spoke an utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Caller,
    Other,
    Unknown,
}

impl Outcome {
    /// Wire value of `SpeechEvent::speaker`.
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Caller => "caller",
            Outcome::Other => "other",
            Outcome::Unknown => "unknown",
        }
    }
}

/// Result of one check, keyed by the utterance it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub key: u64,
    pub outcome: Outcome,
    pub timed_out: bool,
    /// Why the verdict is `Unknown` (`None` for caller/other): see [`UNKNOWN_SHORT`] and the
    /// other `UNKNOWN_*` values. Only `short` is suppressed by `suppress_short_barge_in`.
    pub reason: Option<&'static str>,
    /// True for the result of a re-check (second window or later). Only `Caller` from a re-check
    /// is stored; any other re-check result leaves the first `other` verdict in place.
    pub recheck: bool,
    /// Which window this result scored (1 = first check).
    pub window: u32,
}

/// `speakerUnknownReason` values (the final's `speaker` is `unknown`).
/// No reference voice yet.
pub const UNKNOWN_NOT_ENROLLED: &str = "not_enrolled";
/// Less speech than `min_check_speech_ms`: the utterance was not scored.
pub const UNKNOWN_SHORT: &str = "short";
/// The embedding call exceeded `verify_timeout_ms`, or the final stopped waiting for the verdict.
pub const UNKNOWN_TIMEOUT: &str = "timeout";
/// The embedding call failed.
pub const UNKNOWN_ERROR: &str = "error";
/// The embedder returned no embedding, or the score could not be computed.
pub const UNKNOWN_NO_EMBEDDING: &str = "no_embedding";
/// Gate enrolled, but no check was sent for the utterance and no verdict exists.
pub const UNKNOWN_NO_CHECK: &str = "no_check";

/// What a barge-in attempt for utterance `key` should do right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BargeDecision {
    /// Gate off, not enrolled, or the verdict already says caller/unknown.
    Proceed,
    /// The verdict is `other`.
    Suppress,
    /// Wait for the verdict; the verdict handler decides.
    Defer,
}

enum Msg {
    Enroll {
        pcm: Vec<u8>,
    },
    /// `speech_bytes` is the clip without the pre-roll. `recheck`: a window after the first, sent
    /// while the verdict is `other`. `window`: 1 for the first check.
    Check {
        key: u64,
        pcm: Vec<u8>,
        speech_bytes: usize,
        recheck: bool,
        window: u32,
    },
    Reset,
}

#[derive(Default)]
struct Book {
    verdicts: VecDeque<(u64, Outcome)>,
    /// Why each `Unknown` verdict is unknown (see [`Verdict::reason`]).
    reasons: VecDeque<(u64, &'static str)>,
    sent: VecDeque<u64>,
    /// Keys with a re-check window in flight (removed when its result settles).
    rechecks_in_flight: VecDeque<u64>,
    /// Highest window settled per key (first check = 1).
    windows_scored: VecDeque<(u64, u32)>,
    /// Keys whose final read its verdict: no re-check starts after that.
    closed: VecDeque<u64>,
}

struct Shared {
    enrolled: AtomicBool,
    book: Mutex<Book>,
    notify: Notify,
    backlog: AtomicUsize,
    backlog_logged: AtomicBool,
    stale: std::sync::atomic::AtomicU64,
    /// Warn lines (metadata only), also printed to stderr. Test-visible via [`SpeakerGate::warnings`].
    warnings: Mutex<Vec<String>>,
    /// Test hook: while set, the verifier stores verdicts but holds back `on_verdict`.
    parked: AtomicBool,
    park_release: Notify,
    /// Test hook: times a barge-in was held for a further window.
    holds: std::sync::atomic::AtomicU64,
}

impl Shared {
    fn warn(&self, line: String) {
        eprintln!("[speaker-gate] warn: {line}");
        let mut w = lock(&self.warnings);
        if w.len() < 64 {
            w.push(line);
        }
    }
}

struct Cur {
    key: u64,
    buf: Vec<u8>,
    /// Pre-roll bytes at the head of `buf`; they do not count as speech.
    pre: usize,
    sent: bool,
    /// Windows sent for this key (0 before the first check, 1 after it).
    windows_sent: u32,
    /// Speech bytes of the utterance, never capped (`buf` may be trimmed). A segment that joins a
    /// key whose windows already went out starts with credit for them, so its next window needs
    /// `check_speech_ms` of its own speech.
    speech_seen: usize,
    /// The final read the verdict: no further window starts.
    rechecks_closed: bool,
}

#[derive(Default)]
struct Feed {
    ring: Vec<u8>,
    cur: Option<Cur>,
    ended: VecDeque<(u64, Vec<u8>)>,
    /// Barge-ins deferred until their utterance's verdict, one per utterance key. Every check
    /// yields a verdict within the verifier's own timeout and that verdict removes its key, so
    /// it holds at most the live and closing utterances; `reset` clears it.
    pending_barge: BTreeSet<u64>,
}

/// Per-agent speaker gate (feed side plus the handle to the verifier task).
pub struct SpeakerGate {
    cfg: SpeakerGateConfig,
    tx: UnboundedSender<Msg>,
    rx: Mutex<Option<UnboundedReceiver<Msg>>>,
    shared: Arc<Shared>,
    feed: Mutex<Feed>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn push_capped<T: PartialEq>(q: &mut VecDeque<T>, v: T) {
    q.push_back(v);
    while q.len() > BOOK_CAP {
        q.pop_front();
    }
}

fn cosine(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let (mut dot, mut na, mut nb) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (x, y) in a.iter().zip(b) {
        dot += f64::from(*x) * f64::from(*y);
        na += f64::from(*x) * f64::from(*x);
        nb += f64::from(*y) * f64::from(*y);
    }
    if na <= 0.0 || nb <= 0.0 {
        return None;
    }
    Some((dot / (na.sqrt() * nb.sqrt())) as f32)
}

impl SpeakerGate {
    pub fn new(cfg: SpeakerGateConfig) -> Arc<Self> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Arc::new(Self {
            cfg,
            tx,
            rx: Mutex::new(Some(rx)),
            shared: Arc::new(Shared {
                enrolled: AtomicBool::new(false),
                book: Mutex::new(Book::default()),
                notify: Notify::new(),
                backlog: AtomicUsize::new(0),
                backlog_logged: AtomicBool::new(false),
                stale: std::sync::atomic::AtomicU64::new(0),
                warnings: Mutex::new(Vec::new()),
                parked: AtomicBool::new(false),
                park_release: Notify::new(),
                holds: std::sync::atomic::AtomicU64::new(0),
            }),
            feed: Mutex::new(Feed::default()),
        })
    }

    /// Starts the verifier task (once; later calls are no-ops). Needs a tokio runtime.
    /// `on_verdict` is called after the verdict is stored; it must not block.
    pub fn spawn(
        &self,
        embedder: Box<dyn SpeakerEmbedder>,
        on_verdict: Box<dyn Fn(Verdict) + Send + Sync>,
    ) {
        let Some(rx) = lock(&self.rx).take() else {
            return;
        };
        let cfg = self.cfg.clone();
        let shared = Arc::clone(&self.shared);
        tokio::spawn(run(rx, Arc::from(embedder), cfg, shared, on_verdict));
    }

    pub fn is_enrolled(&self) -> bool {
        self.shared.enrolled.load(Ordering::SeqCst)
    }

    /// Verdicts dropped because their utterance was gone (tests and diagnostics).
    pub fn stale_verdicts(&self) -> u64 {
        self.shared.stale.load(Ordering::Relaxed)
    }

    /// Warn lines logged so far (ids and numbers only).
    pub fn warnings(&self) -> Vec<String> {
        lock(&self.shared.warnings).clone()
    }

    pub fn note_stale_verdict(&self) {
        self.shared.stale.fetch_add(1, Ordering::Relaxed);
    }

    /// Forget the enrolled voice; the next utterances enroll again.
    pub fn reset(&self) {
        self.shared.enrolled.store(false, Ordering::SeqCst);
        {
            let mut feed = lock(&self.feed);
            feed.ended.clear();
            feed.pending_barge.clear();
        }
        self.send(Msg::Reset);
    }

    fn send(&self, msg: Msg) {
        let pending = self.shared.backlog.fetch_add(1, Ordering::SeqCst) + 1;
        if pending > BACKLOG_WARN && !self.shared.backlog_logged.swap(true, Ordering::SeqCst) {
            self.shared
                .warn(format!("speaker gate backlog: {pending} messages pending"));
        }
        let _ = self.tx.send(msg);
    }

    /// Test hook: while on, verdicts are stored (and waiters notified) but `on_verdict` waits;
    /// turning it off releases the held delivery.
    #[doc(hidden)]
    pub fn park_verdict_delivery(&self, on: bool) {
        self.shared.parked.store(on, Ordering::SeqCst);
        if !on {
            self.shared.park_release.notify_waiters();
        }
    }

    /// Test hook: how often a barge-in was held for a further window.
    #[doc(hidden)]
    pub fn hold_count(&self) -> u64 {
        self.shared.holds.load(Ordering::SeqCst)
    }

    /// Test hook: bytes currently buffered for the live utterance.
    #[doc(hidden)]
    pub fn live_buffer_bytes(&self) -> usize {
        lock(&self.feed).cur.as_ref().map_or(0, |c| c.buf.len())
    }

    /// Enrolled with re-checks on: the buffer rolls (pre-roll plus the latest window) once the
    /// first check went out.
    fn rolling(&self) -> bool {
        self.cfg.recheck_on_other && self.is_enrolled()
    }

    /// Cap on collected speech when the buffer does not roll.
    fn cap_bytes(&self) -> usize {
        let ms = if self.is_enrolled() {
            self.cfg.check_speech_ms
        } else {
            self.cfg.check_speech_ms.max(self.cfg.enroll_speech_ms)
        };
        ms as usize * BYTES_PER_MS
    }

    fn windows_remain(&self, windows_sent: u32) -> bool {
        self.cfg.recheck_max_windows == 0 || windows_sent < self.cfg.recheck_max_windows
    }

    fn check_bytes(&self) -> usize {
        self.cfg.check_speech_ms as usize * BYTES_PER_MS
    }

    /// SpeechStart of utterance `key`; `frame` is the onset frame (16 kHz mono s16le).
    /// A second start for the same key (brief gap) continues the same utterance.
    ///
    /// One first check per utterance key, plus consecutive re-checks while the verdict is `other`
    /// (up to `recheck_max_windows`). A segment that joins a key whose check already went out
    /// (pending or settled) does not send another first check, so a speaker who joins the
    /// utterance during the gate hold cannot relabel a `caller` or `unknown` verdict; only an
    /// `other` verdict can be re-checked.
    pub fn begin(&self, key: u64, frame: &[u8]) {
        let mut feed = lock(&self.feed);
        if feed.cur.as_ref().is_some_and(|c| c.key == key) {
            drop(feed);
            self.push_frame(frame);
            return;
        }
        if feed.cur.is_some() {
            self.end_locked(&mut feed);
        }
        let resumed = feed
            .ended
            .back()
            .is_some_and(|(k, _)| *k == key)
            .then(|| feed.ended.pop_back())
            .flatten();
        let check = self.check_bytes();
        let cur = match resumed {
            Some((k, buf)) => {
                let ws = self.windows_sent_for(k);
                Cur {
                    key: k,
                    buf,
                    pre: 0,
                    sent: true,
                    windows_sent: ws,
                    speech_seen: ws as usize * check,
                    rechecks_closed: self.recheck_closed(k),
                }
            }
            None => {
                let buf = std::mem::take(&mut feed.ring);
                let pre = buf.len();
                let sent = self.check_sent(key);
                let ws = if sent { self.windows_sent_for(key) } else { 0 };
                Cur {
                    key,
                    pre,
                    buf,
                    sent,
                    windows_sent: ws,
                    // A segment that joins a key whose windows already went out scores its own
                    // next `check_speech_ms`; otherwise the window follows the first check.
                    speech_seen: ws as usize * check,
                    rechecks_closed: self.recheck_closed(key),
                }
            }
        };
        feed.cur = Some(cur);
        drop(feed);
        self.push_frame(frame);
    }

    /// One 16 kHz mono s16le frame. Inside an utterance it is collected; otherwise it feeds the
    /// pre-roll ring.
    pub fn push_frame(&self, frame: &[u8]) {
        let cap = self.cap_bytes();
        let check = self.check_bytes();
        let rolling = self.rolling();
        let keep = PRE_ROLL_MS * BYTES_PER_MS + check;
        let mut to_send = None;
        {
            let mut feed = lock(&self.feed);
            match feed.cur.as_mut() {
                Some(cur) => {
                    let speech = cur.buf.len() - cur.pre;
                    if rolling {
                        cur.buf.extend_from_slice(frame);
                    } else if speech < cap {
                        let room = cap - speech;
                        cur.buf.extend_from_slice(&frame[..frame.len().min(room)]);
                    }
                    cur.speech_seen += frame.len();
                    if !cur.sent && cur.buf.len() - cur.pre >= check {
                        cur.sent = true;
                        cur.windows_sent = 1;
                        to_send = Some((
                            cur.key,
                            cur.buf[..cur.pre + check].to_vec(),
                            check,
                            false,
                            1,
                        ));
                    } else if self.cfg.recheck_on_other
                        && cur.sent
                        && !cur.rechecks_closed
                        && self.windows_remain(cur.windows_sent)
                        && cur.speech_seen >= (cur.windows_sent as usize + 1) * check
                        && self.is_enrolled()
                        && self.recheck_due(cur.key)
                    {
                        cur.windows_sent += 1;
                        // Marked under the feed lock, so a final or barge decision never sees the
                        // window full but the re-check unsent.
                        push_capped(&mut lock(&self.shared.book).rechecks_in_flight, cur.key);
                        let from = cur.buf.len().saturating_sub(keep);
                        let pcm = cur.buf[from..].to_vec();
                        to_send = Some((cur.key, pcm, check, true, cur.windows_sent));
                    }
                    if rolling && cur.sent && cur.buf.len() > keep {
                        let n = cur.buf.len() - keep;
                        cur.buf.drain(..n);
                        cur.pre = 0;
                    }
                }
                None => {
                    feed.ring.extend_from_slice(frame);
                    let max = PRE_ROLL_MS * BYTES_PER_MS;
                    if feed.ring.len() > max {
                        let drop_n = feed.ring.len() - max;
                        feed.ring.drain(..drop_n);
                    }
                }
            }
        }
        if let Some((key, pcm, speech_bytes, recheck, window)) = to_send {
            if recheck {
                eprintln!(
                    "[speaker-gate] info: speaker gate re-check sent key={key} window={window}"
                );
            } else {
                self.mark_sent(key);
            }
            self.send(Msg::Check {
                key,
                pcm,
                speech_bytes,
                recheck,
                window,
            });
        }
    }

    fn mark_sent(&self, key: u64) {
        push_capped(&mut lock(&self.shared.book).sent, key);
    }

    /// SpeechEnd: send what exists if no check went out yet.
    pub fn end_utterance(&self) {
        let mut feed = lock(&self.feed);
        self.end_locked(&mut feed);
    }

    fn end_locked(&self, feed: &mut Feed) {
        let Some(cur) = feed.cur.take() else {
            return;
        };
        if !cur.sent {
            self.mark_sent(cur.key);
            let speech = (cur.buf.len() - cur.pre).min(self.check_bytes());
            self.send(Msg::Check {
                key: cur.key,
                pcm: cur.buf[..cur.pre + speech].to_vec(),
                speech_bytes: speech,
                recheck: false,
                window: 1,
            });
        } else if self.cfg.recheck_on_other
            && self.outcome_for(cur.key) == Some(Outcome::Other)
            && !self.recheck_in_flight(cur.key)
        {
            // Ended with no re-check window in flight: the `other` verdict stands and the barge-in
            // held for a further window is dropped. A window in flight keeps it (the final awaits
            // that result).
            feed.pending_barge.remove(&cur.key);
        }
        if !self.is_enrolled() {
            // Kept for enrollment: voiced PCM only, without the pre-roll.
            let mut voiced = cur.buf;
            voiced.drain(..cur.pre);
            feed.ended.push_back((cur.key, voiced));
            while feed.ended.len() > ENDED_CAP {
                feed.ended.pop_front();
            }
        }
    }

    /// The stored verdict for `key`, if the check finished.
    pub fn outcome_for(&self, key: u64) -> Option<Outcome> {
        lock(&self.shared.book)
            .verdicts
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, o)| *o)
    }

    fn check_sent(&self, key: u64) -> bool {
        lock(&self.shared.book).sent.contains(&key)
    }

    fn recheck_closed(&self, key: u64) -> bool {
        lock(&self.shared.book).closed.contains(&key)
    }

    /// Windows sent for `key` so far: the first check plus every re-check settled or in flight.
    fn windows_sent_for(&self, key: u64) -> u32 {
        let book = lock(&self.shared.book);
        let scored = book
            .windows_scored
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(0, |(_, w)| *w);
        let flying = u32::from(book.rechecks_in_flight.contains(&key));
        (scored + flying).max(u32::from(book.sent.contains(&key)))
    }

    /// The stored verdict of `key` is `other`, its final has not read it, and no re-check is in
    /// flight.
    fn recheck_due(&self, key: u64) -> bool {
        let book = lock(&self.shared.book);
        book.verdicts
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, o)| *o)
            == Some(Outcome::Other)
            && !book.closed.contains(&key)
            && !book.rechecks_in_flight.contains(&key)
    }

    fn recheck_in_flight(&self, key: u64) -> bool {
        lock(&self.shared.book).rechecks_in_flight.contains(&key)
    }

    /// A re-check of `key` is in flight, or can still start (the utterance is live, its cap not
    /// reached, its final not read). Caller holds the feed lock.
    fn recheck_open_locked(&self, feed: &Feed, key: u64) -> bool {
        if !self.cfg.recheck_on_other {
            return false;
        }
        if self.recheck_in_flight(key) {
            return true;
        }
        feed.cur.as_ref().is_some_and(|c| {
            c.key == key && c.sent && !c.rechecks_closed && self.windows_remain(c.windows_sent)
        }) && !self.recheck_closed(key)
    }

    /// After a first `other` verdict whose barge-in was pending: keep it pending when a re-check
    /// can still decide it (atomically with SpeechEnd, which drops it).
    pub fn hold_for_recheck(&self, key: u64) -> bool {
        let mut feed = lock(&self.feed);
        if self.recheck_open_locked(&feed, key) {
            feed.pending_barge.insert(key);
            self.shared.holds.fetch_add(1, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// The final of `key` is reading its verdict: no re-check starts after this (an in-flight one
    /// is still awaited).
    pub fn close_recheck(&self, key: u64) {
        let mut feed = lock(&self.feed);
        if let Some(cur) = feed.cur.as_mut().filter(|c| c.key == key) {
            cur.rechecks_closed = true;
        }
        let mut book = lock(&self.shared.book);
        if !book.closed.contains(&key) {
            push_capped(&mut book.closed, key);
        }
    }

    /// True when a re-check of `key` settled (`speakerRechecked` on the final).
    pub fn rechecked(&self, key: u64) -> bool {
        self.windows_scored(key) >= 2
    }

    /// Windows of `key` scored so far (first check = 1; 0 when none settled).
    pub fn windows_scored(&self, key: u64) -> u32 {
        lock(&self.shared.book)
            .windows_scored
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(0, |(_, w)| *w)
    }

    /// Decide a barge-in attempt for the live utterance `key`.
    pub fn barge_decision(&self, key: u64) -> BargeDecision {
        if !self.is_enrolled() {
            return BargeDecision::Proceed;
        }
        match self.outcome_for(key) {
            Some(Outcome::Other) => {
                if self.recheck_open_locked(&lock(&self.feed), key) {
                    BargeDecision::Defer
                } else {
                    BargeDecision::Suppress
                }
            }
            Some(Outcome::Unknown)
                if self.cfg.suppress_short_barge_in
                    && self.stored_unknown_reason(key) == Some(UNKNOWN_SHORT) =>
            {
                BargeDecision::Suppress
            }
            Some(_) => BargeDecision::Proceed,
            None => BargeDecision::Defer,
        }
    }

    /// True when `verdict` vetoes the barge-in of its utterance: `other`, or (opt-in
    /// `suppress_short_barge_in`) `unknown` because the speech was below `min_check_speech_ms`.
    /// Every other `unknown` fails open.
    /// A re-check releases the barge-in only when it says `caller`; any other re-check result
    /// (including `unknown`) keeps the first `other` verdict's veto.
    pub fn vetoes_barge(&self, verdict: &Verdict) -> bool {
        if verdict.recheck {
            return verdict.outcome != Outcome::Caller;
        }
        verdict.outcome == Outcome::Other
            || (self.cfg.suppress_short_barge_in
                && verdict.outcome == Outcome::Unknown
                && verdict.reason == Some(UNKNOWN_SHORT))
    }

    fn stored_unknown_reason(&self, key: u64) -> Option<&'static str> {
        lock(&self.shared.book)
            .reasons
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, r)| *r)
    }

    /// `speakerUnknownReason` for the final of `key` whose outcome is `Unknown`.
    pub fn unknown_reason(&self, key: u64) -> &'static str {
        if let Some(r) = self.stored_unknown_reason(key) {
            return r;
        }
        if !self.is_enrolled() {
            UNKNOWN_NOT_ENROLLED
        } else if self.check_sent(key) {
            // The final stopped waiting before the verdict arrived.
            UNKNOWN_TIMEOUT
        } else {
            UNKNOWN_NO_CHECK
        }
    }

    pub fn set_pending_barge(&self, key: u64) {
        lock(&self.feed).pending_barge.insert(key);
    }

    /// True (and cleared) when a barge-in for `key` waits for its verdict.
    pub fn take_pending_barge(&self, key: u64) -> bool {
        lock(&self.feed).pending_barge.remove(&key)
    }

    /// The verdict a final of `key` can carry right now: the stored verdict, or `Unknown` when no
    /// check was sent. `None` while a sent check or re-check has not settled (use
    /// [`Self::await_outcome`] off the frame path).
    pub fn settled_outcome(&self, key: u64) -> Option<Outcome> {
        if !self.check_sent(key) {
            return Some(self.outcome_for(key).unwrap_or(Outcome::Unknown));
        }
        if self.recheck_in_flight(key) {
            return None;
        }
        self.outcome_for(key)
    }

    /// Verdict for the final of `key`: waits at most `verify_timeout_ms` for a check (or re-check)
    /// that was sent but has not settled, then the stored verdict or `Unknown`.
    pub async fn await_outcome(&self, key: u64) -> Outcome {
        if !self.check_sent(key) {
            return self.outcome_for(key).unwrap_or(Outcome::Unknown);
        }
        let deadline = Duration::from_millis(u64::from(self.cfg.verify_timeout_ms));
        let started = tokio::time::Instant::now();
        loop {
            let notified = self.shared.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(o) = self.outcome_for(key) {
                if !self.recheck_in_flight(key) {
                    return o;
                }
            }
            let left = deadline.saturating_sub(started.elapsed());
            if left.is_zero() || tokio::time::timeout(left, notified).await.is_err() {
                return self.outcome_for(key).unwrap_or(Outcome::Unknown);
            }
        }
    }

    /// After a non-empty final whose verdict is not `other`: hand that utterance's voiced PCM
    /// (without the pre-roll) to enrollment (until enrolled).
    pub fn enroll_utterance(&self, key: u64) {
        if self.is_enrolled() {
            return;
        }
        let pcm = {
            let mut feed = lock(&self.feed);
            if let Some(cur) = feed.cur.as_ref().filter(|c| c.key == key) {
                Some(cur.buf[cur.pre..].to_vec())
            } else {
                feed.ended
                    .iter()
                    .position(|(k, _)| *k == key)
                    .and_then(|i| feed.ended.remove(i))
                    .map(|(_, b)| b)
            }
        };
        if let Some(pcm) = pcm.filter(|p| !p.is_empty()) {
            self.send(Msg::Enroll { pcm });
        }
    }
}

async fn embed_bounded(
    embedder: &Arc<dyn SpeakerEmbedder>,
    pcm: Vec<u8>,
    bound: Duration,
) -> Result<Option<Vec<f32>>, EmbedFail> {
    match tokio::time::timeout(bound, embedder.embed(Bytes::from(pcm), 16_000)).await {
        Err(_) => Err(EmbedFail::Timeout),
        Ok(Err(_)) => Err(EmbedFail::Error),
        Ok(Ok(v)) => Ok(v),
    }
}

enum EmbedFail {
    Timeout,
    Error,
}

async fn run(
    mut rx: UnboundedReceiver<Msg>,
    embedder: Arc<dyn SpeakerEmbedder>,
    cfg: SpeakerGateConfig,
    shared: Arc<Shared>,
    on_verdict: Box<dyn Fn(Verdict) + Send + Sync>,
) {
    let verify = Duration::from_millis(u64::from(cfg.verify_timeout_ms));
    let enroll_bytes = cfg.enroll_speech_ms as usize * BYTES_PER_MS;
    let min_check_bytes = cfg.min_check_speech_ms as usize * BYTES_PER_MS;
    let mut reference: Option<Vec<f32>> = None;
    let mut enroll_pcm: Vec<u8> = Vec::new();

    while let Some(msg) = rx.recv().await {
        let pending = shared
            .backlog
            .fetch_sub(1, Ordering::SeqCst)
            .saturating_sub(1);
        if pending == 0 {
            shared.backlog_logged.store(false, Ordering::SeqCst);
        }
        match msg {
            Msg::Reset => {
                reference = None;
                enroll_pcm.clear();
                shared.enrolled.store(false, Ordering::SeqCst);
            }
            Msg::Enroll { pcm } => {
                if reference.is_some() {
                    continue;
                }
                enroll_pcm.extend_from_slice(&pcm);
                if enroll_pcm.len() < enroll_bytes {
                    continue;
                }
                match embed_bounded(&embedder, enroll_pcm.clone(), verify * 4).await {
                    Ok(Some(v)) => {
                        reference = Some(v);
                        enroll_pcm.clear();
                        shared.enrolled.store(true, Ordering::SeqCst);
                        crate::agent::voice_debug("speaker gate enrolled");
                    }
                    Ok(None) => {
                        enroll_pcm.clear();
                    }
                    Err(EmbedFail::Timeout) => {
                        shared.warn("speaker gate enroll timeout".into());
                        // Keep collecting, bounded.
                        let max = enroll_bytes * 2;
                        if enroll_pcm.len() > max {
                            let n = enroll_pcm.len() - max;
                            enroll_pcm.drain(..n);
                        }
                    }
                    Err(EmbedFail::Error) => {
                        shared.warn("speaker gate enroll failed".into());
                        enroll_pcm.clear();
                    }
                }
            }
            Msg::Check {
                key,
                pcm,
                speech_bytes,
                recheck,
                window,
            } => {
                let started = Instant::now();
                let (outcome, timed_out, reason) = match reference.as_ref() {
                    None => (Outcome::Unknown, false, Some(UNKNOWN_NOT_ENROLLED)),
                    Some(_) if speech_bytes < min_check_bytes => {
                        (Outcome::Unknown, false, Some(UNKNOWN_SHORT))
                    }
                    Some(reference) => match embed_bounded(&embedder, pcm, verify).await {
                        Ok(Some(v)) => match cosine(reference, &v) {
                            Some(score) => {
                                crate::agent::voice_debug(format!(
                                    "speaker gate score key={key} score={score:.3} recheck={recheck}"
                                ));
                                if score >= cfg.threshold {
                                    (Outcome::Caller, false, None)
                                } else {
                                    (Outcome::Other, false, None)
                                }
                            }
                            None => (Outcome::Unknown, false, Some(UNKNOWN_NO_EMBEDDING)),
                        },
                        Ok(None) => (Outcome::Unknown, false, Some(UNKNOWN_NO_EMBEDDING)),
                        Err(EmbedFail::Timeout) => {
                            let elapsed_ms = started.elapsed().as_millis();
                            shared.warn(format!(
                                "speaker gate verify timeout key={key} elapsed_ms={elapsed_ms}"
                            ));
                            (Outcome::Unknown, true, Some(UNKNOWN_TIMEOUT))
                        }
                        Err(EmbedFail::Error) => (Outcome::Unknown, false, Some(UNKNOWN_ERROR)),
                    },
                };
                {
                    let mut book = lock(&shared.book);
                    if recheck {
                        // Upgrade only: `other` -> `caller`. Anything else keeps `other`.
                        if outcome == Outcome::Caller {
                            push_capped(&mut book.verdicts, (key, outcome));
                        }
                        book.rechecks_in_flight.retain(|k| *k != key);
                    } else {
                        push_capped(&mut book.verdicts, (key, outcome));
                        if let Some(r) = reason {
                            push_capped(&mut book.reasons, (key, r));
                        }
                    }
                    if let Some(e) = book.windows_scored.iter_mut().find(|(k, _)| *k == key) {
                        e.1 = e.1.max(window);
                    } else {
                        push_capped(&mut book.windows_scored, (key, window));
                    }
                }
                if recheck {
                    eprintln!(
                        "[speaker-gate] info: speaker gate re-check key={key} window={window} outcome={}",
                        outcome.as_str()
                    );
                }
                shared.notify.notify_waiters();
                // Test hook: hold the delivery (the verdict is stored and waiters are notified).
                loop {
                    let released = shared.park_release.notified();
                    tokio::pin!(released);
                    released.as_mut().enable();
                    if !shared.parked.load(Ordering::SeqCst) {
                        break;
                    }
                    released.await;
                }
                on_verdict(Verdict {
                    key,
                    outcome,
                    timed_out,
                    reason,
                    recheck,
                    window,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrollment_pcm_excludes_the_pre_roll() {
        let gate = SpeakerGate::new(SpeakerGateConfig {
            enabled: true,
            model_path: Some("m.onnx".into()),
            ..SpeakerGateConfig::default()
        });
        // 300 ms of pre-roll (value 1), then 100 ms of speech (value 2).
        for _ in 0..15 {
            gate.push_frame(&[1_u8; 640]);
        }
        gate.begin(7, &[2_u8; 640]);
        for _ in 0..4 {
            gate.push_frame(&[2_u8; 640]);
        }
        gate.end_utterance();
        let mut rx = lock(&gate.rx).take().unwrap();
        gate.enroll_utterance(7);
        let mut enrolled = None;
        while let Ok(msg) = rx.try_recv() {
            if let Msg::Enroll { pcm } = msg {
                enrolled = Some(pcm);
            }
        }
        let pcm = enrolled.expect("enroll message");
        assert_eq!(pcm.len(), 5 * 640);
        assert!(pcm.iter().all(|b| *b == 2), "pre-roll bytes in enrollment");
    }

    // ---- re-check (recheck_on_other) ----------------------------------------------------------

    const FRAME: usize = 640; // 20 ms of 16 kHz mono s16le

    fn enrolled_gate(recheck: bool) -> Arc<SpeakerGate> {
        enrolled_gate_cap(recheck, 2)
    }

    fn enrolled_gate_cap(recheck: bool, cap: u32) -> Arc<SpeakerGate> {
        let gate = SpeakerGate::new(SpeakerGateConfig {
            enabled: true,
            model_path: Some("m.onnx".into()),
            check_speech_ms: 200,
            min_check_speech_ms: 100,
            recheck_on_other: recheck,
            recheck_max_windows: cap,
            ..SpeakerGateConfig::default()
        });
        gate.shared.enrolled.store(true, Ordering::SeqCst);
        gate
    }

    fn store(gate: &SpeakerGate, key: u64, outcome: Outcome) {
        push_capped(&mut lock(&gate.shared.book).verdicts, (key, outcome));
    }

    /// (key, pcm bytes, speech bytes, recheck) of every check sent so far.
    fn checks(rx: &mut UnboundedReceiver<Msg>) -> Vec<(u64, usize, usize, bool)> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let Msg::Check {
                key,
                pcm,
                speech_bytes,
                recheck,
                ..
            } = msg
            {
                out.push((key, pcm.len(), speech_bytes, recheck));
            }
        }
        out
    }

    /// Pre-roll, then `begin` plus `speech_frames - 1` frames (the onset frame counts as speech).
    fn start(gate: &SpeakerGate, key: u64, speech_frames: usize) {
        for _ in 0..15 {
            gate.push_frame(&[1_u8; FRAME]);
        }
        gate.begin(key, &[2_u8; FRAME]);
        for _ in 1..speech_frames {
            gate.push_frame(&[2_u8; FRAME]);
        }
    }

    /// The frame count is the clock: the re-check goes out exactly when the collected speech
    /// reaches `recheck_barge_earliest_ms(check_speech_ms)`, never a frame earlier.
    #[test]
    fn recheck_goes_out_at_the_named_bound() {
        assert_eq!(recheck_barge_earliest_ms(1500, 2), 3000);
        assert_eq!(
            recheck_barge_earliest_ms(SpeakerGateConfig::default().check_speech_ms, 2),
            3000
        );
        assert!(SpeakerGateConfig::default().recheck_on_other);
        let gate = enrolled_gate(true);
        let mut rx = lock(&gate.rx).take().unwrap();
        start(&gate, 1, 10);
        assert_eq!(checks(&mut rx), vec![(1, 300 * 32 + 6400, 6400, false)]);
        store(&gate, 1, Outcome::Other);
        let bound_frames = (recheck_barge_earliest_ms(200, 2) / 20) as usize;
        for _ in 10..bound_frames - 1 {
            gate.push_frame(&[3_u8; FRAME]);
        }
        assert!(checks(&mut rx).is_empty(), "no re-check before the bound");
        assert_eq!(gate.barge_decision(1), BargeDecision::Defer);
        gate.push_frame(&[3_u8; FRAME]);
        // Second window (6400 bytes) with 300 ms of pre-roll taken from the first window.
        assert_eq!(checks(&mut rx), vec![(1, 300 * 32 + 6400, 6400, true)]);
        assert_eq!(
            gate.settled_outcome(1),
            None,
            "final waits for the re-check"
        );
        for _ in 0..40 {
            gate.push_frame(&[3_u8; FRAME]);
        }
        gate.end_utterance();
        assert!(checks(&mut rx).is_empty(), "at most one re-check per key");
    }

    #[test]
    fn earliest_barge_ms_is_window_times_check() {
        for w in 1..=8_u32 {
            assert_eq!(recheck_barge_earliest_ms(1500, w), 1500 * w);
        }
    }

    #[test]
    fn recheck_max_windows_validation() {
        let cfg = |on: bool, cap: u32| SpeakerGateConfig {
            enabled: true,
            model_path: Some("m.onnx".into()),
            recheck_on_other: on,
            recheck_max_windows: cap,
            ..SpeakerGateConfig::default()
        };
        let e = cfg(true, 1).validate().unwrap_err();
        assert!(e.contains("recheckMaxWindows"), "{e}");
        for cap in [0, 2, 8] {
            assert!(cfg(true, cap).validate().is_ok(), "cap {cap}");
        }
        assert!(cfg(false, 1).validate().is_ok(), "one check, re-check off");
        assert_eq!(
            SpeakerGateConfig::default().recheck_max_windows,
            DEFAULT_RECHECK_MAX_WINDOWS
        );
    }

    /// Each further window is cut at its own boundary (frame count is the clock), never while the
    /// previous one is in flight, and never past the cap. The clip is pre-roll plus the latest
    /// window.
    #[test]
    fn windows_are_cut_at_each_boundary_up_to_the_cap() {
        let gate = enrolled_gate_cap(true, 4);
        let mut rx = lock(&gate.rx).take().unwrap();
        start(&gate, 1, 10);
        assert_eq!(checks(&mut rx).len(), 1);
        store(&gate, 1, Outcome::Other);
        let mut cut_at = Vec::new();
        for f in 10..80_usize {
            gate.push_frame(&[3_u8; FRAME]);
            for c in checks(&mut rx) {
                assert_eq!(c, (1, 300 * 32 + 6400, 6400, true));
                cut_at.push(f + 1);
            }
            if f == 22 {
                // Window 2 still in flight at its successor's boundary: nothing is queued.
                assert_eq!(cut_at, vec![20]);
                lock(&gate.shared.book).rechecks_in_flight.clear();
            }
            if cut_at.len() < 3 && f > 22 && !lock(&gate.shared.book).rechecks_in_flight.is_empty()
            {
                lock(&gate.shared.book).rechecks_in_flight.clear();
            }
            if cut_at.len() == 3 {
                lock(&gate.shared.book).rechecks_in_flight.clear();
            }
        }
        // Window 2 at 20 frames; window 3 at 30 (the first boundary after window 2 settled at 23);
        // window 4 at 40; the cap of 4 stops the rest.
        assert_eq!(cut_at, vec![20, 30, 40]);
    }

    #[test]
    fn long_other_utterance_keeps_the_buffer_bounded() {
        let gate = enrolled_gate_cap(true, 0);
        let mut rx = lock(&gate.rx).take().unwrap();
        start(&gate, 1, 10);
        store(&gate, 1, Outcome::Other);
        let bound = (300 + 200) * 32 + FRAME;
        let mut windows = 1;
        // 150 windows of 200 ms.
        for _ in 0..1500 {
            gate.push_frame(&[3_u8; FRAME]);
            assert!(gate.live_buffer_bytes() <= bound, "buffer grew");
            let n = checks(&mut rx).len();
            if n > 0 {
                windows += n;
                lock(&gate.shared.book).rechecks_in_flight.clear();
            }
        }
        assert!(windows > 100, "no cap: windows keep being cut ({windows})");
    }

    #[test]
    fn caller_or_unknown_first_verdict_is_never_rechecked() {
        for outcome in [Outcome::Caller, Outcome::Unknown] {
            let gate = enrolled_gate(true);
            let mut rx = lock(&gate.rx).take().unwrap();
            start(&gate, 1, 10);
            store(&gate, 1, outcome);
            for _ in 0..40 {
                gate.push_frame(&[3_u8; FRAME]);
            }
            gate.end_utterance();
            let sent = checks(&mut rx);
            assert_eq!(sent.len(), 1, "{outcome:?}: exactly one check");
            assert!(!sent[0].3);
            assert_eq!(gate.settled_outcome(1), Some(outcome));
        }
    }

    #[test]
    fn recheck_off_is_one_check_with_the_old_cap() {
        let gate = enrolled_gate(false);
        let mut rx = lock(&gate.rx).take().unwrap();
        start(&gate, 1, 10);
        store(&gate, 1, Outcome::Other);
        for _ in 0..40 {
            gate.push_frame(&[3_u8; FRAME]);
        }
        {
            let feed = lock(&gate.feed);
            let cur = feed.cur.as_ref().unwrap();
            assert_eq!(cur.buf.len() - cur.pre, 6400, "buffer capped at one window");
        }
        assert_eq!(gate.barge_decision(1), BargeDecision::Suppress);
        gate.end_utterance();
        assert_eq!(checks(&mut rx).len(), 1);
    }

    #[test]
    fn utterance_ending_before_the_window_keeps_other_and_drops_the_held_barge() {
        let gate = enrolled_gate(true);
        let mut rx = lock(&gate.rx).take().unwrap();
        start(&gate, 1, 10);
        store(&gate, 1, Outcome::Other);
        gate.set_pending_barge(1);
        assert!(gate.take_pending_barge(1));
        assert!(
            gate.hold_for_recheck(1),
            "live utterance: held for the re-check"
        );
        for _ in 0..5 {
            gate.push_frame(&[3_u8; FRAME]);
        }
        gate.end_utterance();
        assert_eq!(checks(&mut rx).len(), 1, "no re-check");
        assert!(
            !gate.take_pending_barge(1),
            "held barge-in dropped at SpeechEnd"
        );
        assert_eq!(gate.barge_decision(1), BargeDecision::Suppress);
        assert_eq!(gate.settled_outcome(1), Some(Outcome::Other));
        assert!(!gate.rechecked(1));
    }

    #[test]
    fn a_final_that_read_its_verdict_blocks_a_later_recheck() {
        let gate = enrolled_gate(true);
        let mut rx = lock(&gate.rx).take().unwrap();
        start(&gate, 1, 10);
        store(&gate, 1, Outcome::Other);
        gate.close_recheck(1);
        for _ in 0..20 {
            gate.push_frame(&[3_u8; FRAME]);
        }
        assert_eq!(checks(&mut rx).len(), 1);
        assert_eq!(gate.settled_outcome(1), Some(Outcome::Other));
        assert!(!gate.hold_for_recheck(1));
    }

    /// One scripted embedding: optionally held until the notify fires.
    type Step = (Option<Arc<Notify>>, Vec<f32>);

    struct SeqEmbedder {
        queue: Mutex<VecDeque<Step>>,
    }

    #[async_trait::async_trait]
    impl SpeakerEmbedder for SeqEmbedder {
        async fn embed(
            &self,
            _pcm: Bytes,
            _rate: u32,
        ) -> crate::error::SpeechResult<Option<Vec<f32>>> {
            let next = lock(&self.queue).pop_front();
            let Some((wait, v)) = next else {
                return Ok(None);
            };
            if let Some(n) = wait {
                n.notified().await;
            }
            Ok(Some(v))
        }
    }

    /// A re-check result for utterance A that lands after A's final read its verdict and B is
    /// live is applied to A's key only: B's verdict and B's held barge-in are untouched, and the
    /// result never downgrades anything (`rt.stale-event-by-key`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_recheck_result_never_touches_the_next_utterance() {
        let caller = vec![1.0_f32, 0.0];
        let other = vec![0.0_f32, 1.0];
        let release = Arc::new(Notify::new());
        let gate = SpeakerGate::new(SpeakerGateConfig {
            enabled: true,
            model_path: Some("m.onnx".into()),
            enroll_speech_ms: 100,
            check_speech_ms: 200,
            min_check_speech_ms: 100,
            verify_timeout_ms: 5000,
            ..SpeakerGateConfig::default()
        });
        let verdicts: Arc<Mutex<Vec<Verdict>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&verdicts);
        gate.spawn(
            Box::new(SeqEmbedder {
                queue: Mutex::new(VecDeque::from(vec![
                    (None, caller.clone()),                       // enrollment
                    (None, other.clone()),                        // A: first check
                    (Some(Arc::clone(&release)), caller.clone()), // A: re-check, held
                    (None, caller.clone()),                       // B: first check
                ])),
            }),
            Box::new(move |v| lock(&sink).push(v)),
        );
        gate.send(Msg::Enroll {
            pcm: vec![0_u8; 100 * BYTES_PER_MS],
        });
        let wait = |n: usize| {
            let verdicts = Arc::clone(&verdicts);
            async move {
                for _ in 0..500 {
                    if lock(&verdicts).len() >= n {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                panic!("verdict {n} never arrived");
            }
        };
        for _ in 0..500 {
            if gate.is_enrolled() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(gate.is_enrolled());
        start(&gate, 1, 10);
        wait(1).await;
        assert_eq!(gate.outcome_for(1), Some(Outcome::Other));
        for _ in 0..10 {
            gate.push_frame(&[3_u8; FRAME]);
        }
        assert_eq!(gate.settled_outcome(1), None, "A's re-check in flight");
        // A's final stops waiting (its bound passed) and reads `other`; B starts and is held.
        gate.close_recheck(1);
        gate.end_utterance();
        start(&gate, 2, 10);
        gate.set_pending_barge(2);
        release.notify_one();
        wait(3).await;
        let got = lock(&verdicts).clone();
        assert_eq!(
            got.iter()
                .map(|v| (v.key, v.outcome, v.recheck))
                .collect::<Vec<_>>(),
            vec![
                (1, Outcome::Other, false),
                (1, Outcome::Caller, true),
                (2, Outcome::Caller, false),
            ]
        );
        assert!(!gate.take_pending_barge(1), "nothing held for A");
        assert!(
            gate.take_pending_barge(2),
            "B's held barge-in untouched by A's result"
        );
        assert_eq!(gate.outcome_for(2), Some(Outcome::Caller));
        assert!(gate.rechecked(1));
        assert!(!gate.rechecked(2));
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]).unwrap() - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).unwrap().abs() < 1e-6);
        assert!(cosine(&[0.0, 0.0], &[1.0, 0.0]).is_none());
        assert!(cosine(&[1.0], &[1.0, 2.0]).is_none());
    }
}
