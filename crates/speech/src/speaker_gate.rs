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

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Notify;

use crate::config::{SpeakerGateBargeIn, SpeakerGateConfig, SpeakerMismatchAction};
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
/// check included (18 s of speech with the default 1.5 s check).
/// SDK mirror: `SPEAKER_GATE_RECHECK_MAX_WINDOWS` (`packages/sdk/src/voice/defaults.ts`); both are
/// asserted to be 12 in their own tests.
pub const DEFAULT_RECHECK_MAX_WINDOWS: u32 = 12;

/// Consecutive windows of the opposite label that confirm a `caller` -> `other` speaker change
/// ([`SpeakerGateConfig::split_on_speaker_change`]). Starts at 2; tuned with evidence later.
pub const SPEAKER_CHANGE_CONFIRM_WINDOWS: u32 = 2;

/// Windows of `caller` that confirm an `other` -> `caller` change: one. That is the window the
/// re-check upgrade used, so barge latency and false-accept exposure are unchanged.
pub const OTHER_TO_CALLER_CONFIRM_WINDOWS: u32 = 1;

/// `other` -> `caller` split point, stage 1: a quiet run of at least this long in the backward
/// range decides the split (the end of the longest run). Constants of stage 1 and 2 come from
/// `crates/vendor-sherpa-onnx/examples/SPLIT_POINT_BENCH.md` (decision h pick).
pub const SPLIT_QUIET_RUN_MIN_MS: u32 = 500;
/// Stage 2: sub-window length embedded to localize the caller's onset.
pub const SPLIT_STAGE2_SUB_MS: u32 = 1000;
/// Stage 2: hop between sub-windows.
pub const SPLIT_STAGE2_HOP_MS: u32 = 125;
/// Stage 2: the localized point is refined to the lowest-energy frame within this many ms.
pub const SPLIT_STAGE2_REFINE_MS: u32 = 150;
/// Frames of power smoothing in the stage 1 quiet-run search (100 ms).
const SPLIT_SMOOTH_FRAMES: usize = 5;

/// The split point is searched within this many ms either side of the first opposite window's
/// start.
pub const SPLIT_SEARCH_MS: u32 = 300;

/// Frame length (ms) of the split-point energy search.
const SPLIT_FRAME_MS: usize = 20;

/// Adaptive reference ([`SpeakerGateConfig::adapt_reference`], only with
/// `split_on_speaker_change`): a scored window is a candidate only when its score is at least
/// `threshold + ADAPT_MARGIN`. Values from `crates/vendor-sherpa-onnx/examples/ADAPT_REFERENCE_BENCH.md`.
pub const ADAPT_MARGIN: f32 = 0.1;
/// EMA weight of one committed utterance (the mean of its candidates) in the new reference.
pub const ADAPT_ALPHA: f32 = 0.1;
/// An update whose result scores below this cosine against the enrolled embedding (the anchor)
/// is rejected; the reference stays as it was.
pub const ANCHOR_MIN_COSINE: f32 = 0.8;
/// Candidates kept per utterance key (the rest of the utterance still counts for the
/// all-`caller` rule).
const ADAPT_MAX_CANDIDATES: usize = DEFAULT_RECHECK_MAX_WINDOWS as usize;

/// Result of one adaptive-reference update ([`adapt_step`]).
#[derive(Debug, Clone, PartialEq)]
pub enum AdaptStep {
    /// The new (unit-length) reference and its cosine to the anchor.
    Commit {
        reference: Vec<f32>,
        cos_to_anchor: f32,
    },
    /// The update would fall below [`ANCHOR_MIN_COSINE`]; the reference is unchanged.
    RejectAnchor { cos_to_anchor: f32 },
    /// No usable candidate (empty, zero norm or a dimension mismatch).
    Skip,
}

fn normalized(v: &[f32]) -> Option<Vec<f32>> {
    let n = v
        .iter()
        .map(|x| f64::from(*x) * f64::from(*x))
        .sum::<f64>()
        .sqrt();
    if n <= f64::EPSILON || !n.is_finite() {
        return None;
    }
    Some(v.iter().map(|x| (f64::from(*x) / n) as f32).collect())
}

/// One adaptive-reference update: `normalize((1 - ADAPT_ALPHA) * r + ADAPT_ALPHA * c)` with `r`
/// the unit reference and `c` the unit mean of the unit candidates; rejected when the result's
/// cosine to `anchor` is below [`ANCHOR_MIN_COSINE`]. Shared with the adaptation bench.
pub fn adapt_step(reference: &[f32], anchor: &[f32], candidates: &[Vec<f32>]) -> AdaptStep {
    let Some(r) = normalized(reference) else {
        return AdaptStep::Skip;
    };
    let mut sum = vec![0.0_f32; r.len()];
    let mut n = 0_usize;
    for c in candidates {
        if c.len() != r.len() {
            return AdaptStep::Skip;
        }
        if let Some(c) = normalized(c) {
            for (s, x) in sum.iter_mut().zip(c) {
                *s += x;
            }
            n += 1;
        }
    }
    let Some(c) = (n > 0).then(|| normalized(&sum)).flatten() else {
        return AdaptStep::Skip;
    };
    let mixed: Vec<f32> = r
        .iter()
        .zip(&c)
        .map(|(a, b)| (1.0 - ADAPT_ALPHA) * a + ADAPT_ALPHA * b)
        .collect();
    let Some(new) = normalized(&mixed) else {
        return AdaptStep::Skip;
    };
    let Some(cos_to_anchor) = cosine(&new, anchor) else {
        return AdaptStep::Skip;
    };
    if cos_to_anchor < ANCHOR_MIN_COSINE {
        return AdaptStep::RejectAnchor { cos_to_anchor };
    }
    AdaptStep::Commit {
        reference: new,
        cos_to_anchor,
    }
}

/// Adaptive-reference counters (no embedding values): see [`SpeakerGate::adapt_stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AdaptStats {
    /// Utterances whose candidates were folded into the reference.
    pub commits: u64,
    /// Updates rejected by the anchor bound.
    pub rejected_anchor: u64,
    /// Settled utterances that committed nothing (split, a non-`caller` window, no candidate).
    pub skipped: u64,
    /// Cosine of the current reference to the enrolled anchor (`None` before enrollment).
    pub cos_to_anchor: Option<f32>,
}

/// Per-key adaptive-reference state on the verifier task.
#[derive(Default)]
struct AdaptKey {
    candidates: Vec<Vec<f32>>,
    /// A scored window of the key was not `caller` (or was not scored).
    dirty: bool,
}

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

/// A confirmed change of speaker inside one utterance (only with
/// [`SpeakerGateConfig::split_on_speaker_change`]). Times are media ms since the key's onset frame
/// (gap frames included; pre-roll before the onset excluded). The split point depends only on the
/// audio (and, for the stage 2 sub-window embeddings of an `other` -> `caller` change, on the
/// audio only: they score fixed spans), never on when an embedding finished. Both directions are
/// reported: `from` and `to` are `Caller` or `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeakerChange {
    pub key: u64,
    pub from: Outcome,
    pub to: Outcome,
    /// `caller` -> `other`: start of the lowest-energy 20 ms frame within [`SPLIT_SEARCH_MS`] of
    /// the first opposite window's start. `other` -> `caller`: the end of the longest quiet run
    /// from the previous scored window's start to the first `caller` window's start +
    /// [`SPLIT_SEARCH_MS`] (stage 1), or, when that run is under [`SPLIT_QUIET_RUN_MIN_MS`], the
    /// first `caller` sub-window refined by energy (stage 2).
    pub split_at_ms: u32,
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
/// Gate enrolled, but no check was sent for the utterance and no verdict exists. Also the reason
/// for an utterance whose gate was disabled mid-way before a verdict existed.
pub const UNKNOWN_NO_CHECK: &str = "no_check";
/// The gate was enabled at runtime and its speaker model is still loading.
pub const UNKNOWN_LOADING: &str = "loading";

/// Gate state an utterance started with (`set_speaker_gate` applies from the next onset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UttGate {
    /// Gate off: no scoring, no hold, finals carry no speaker fields.
    Off,
    /// Enabled while the model loads: not scored, never holds, `unknown` / `loading`.
    Loading,
    /// Scored as usual.
    Active,
}

/// Gate settings an utterance started with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UttSettings {
    pub gate: UttGate,
    pub on_mismatch: SpeakerMismatchAction,
    pub barge_in: SpeakerGateBargeIn,
    /// The gate was disabled while this (active) utterance was live: no further scoring or hold;
    /// its final keeps the verdict it has, else `unknown` / `no_check`.
    pub cut: bool,
}

/// Runtime switch state (see [`SpeakerGate::set_enabled`]).
struct Runtime {
    enabled: bool,
    /// Model loaded and the verifier can run.
    loaded: bool,
    /// A model load is in flight.
    loading: bool,
    on_mismatch: SpeakerMismatchAction,
    barge_in: SpeakerGateBargeIn,
    /// Settings per utterance key, taken at its first onset.
    utts: VecDeque<(u64, UttSettings)>,
}

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
    /// while the verdict is `other`. `window`: 1 for the first check. `attempt`: feed-side send
    /// counter (monotonic per gate), used to drop the verdicts of checks a re-open superseded.
    Check {
        key: u64,
        pcm: Vec<u8>,
        speech_bytes: usize,
        recheck: bool,
        window: u32,
        attempt: u64,
        /// Media ms since the key's onset frame at the end of the clip (gap frames included;
        /// pre-roll before the onset excluded). The scored span is
        /// `[end_ms - speech_bytes / 32, end_ms)`; the clip itself reaches back further (pre-roll).
        end_ms: u32,
    },
    /// An utterance that starts as the `caller` after an `other` -> `caller` split
    /// ([`SpeakerGate::inherit_caller`]): arms its watch with label `caller`.
    Inherit {
        key: u64,
    },
    /// The final of `key` was emitted ([`SpeakerGate::settle`]): commit its adaptive-reference
    /// candidates if the utterance qualifies.
    Settle {
        key: u64,
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
    /// Highest window settled per key (first check = 1). Numbers the next window; with the
    /// speaker-change watch on it also counts watch windows (see `reported_windows`).
    windows_scored: VecDeque<(u64, u32)>,
    /// Watch on only: highest window per key that the gate with the watch off would also have
    /// scored (the first window, re-checks while the key is `other` and not yet upgraded). This is
    /// what the final reports (`speakerRechecked`, `speakerCheckWindows`).
    reported_windows: VecDeque<(u64, u32)>,
    /// Watch on only: windows scored only because of the speaker-change watch, per key.
    watch_windows: VecDeque<(u64, u32)>,
    /// Keys whose final read its verdict: no re-check starts after that.
    closed: VecDeque<u64>,
    /// Per re-opened key, the last check attempt sent before the re-open: the verdicts of those
    /// attempts are dropped, so a late one never answers the resumed check.
    reopened_after: VecDeque<(u64, u64)>,
    /// Confirmed speaker changes not yet taken ([`SpeakerGate::take_speaker_changes`]).
    changes: VecDeque<SpeakerChange>,
    /// Keys whose speaker change was confirmed: no further window is cut for them.
    split_keys: VecDeque<u64>,
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
    /// Test hook: verdicts dropped because a re-open superseded their check.
    superseded: std::sync::atomic::AtomicU64,
    adapt_commits: std::sync::atomic::AtomicU64,
    adapt_rejected: std::sync::atomic::AtomicU64,
    adapt_skipped: std::sync::atomic::AtomicU64,
    /// f32 bits of the reference's cosine to the anchor; NaN before enrollment.
    adapt_cos: std::sync::atomic::AtomicU32,
    /// Test hook: `Msg::Settle` messages the verifier task has handled.
    settles: std::sync::atomic::AtomicU64,
    /// Adaptive-reference log lines (counts and cosine only), also printed under `VOICE_DEBUG`.
    /// Test-visible via [`SpeakerGate::adapt_log`].
    adapt_log: Mutex<Vec<String>>,
}

impl Shared {
    fn warn(&self, line: String) {
        eprintln!("[speaker-gate] warn: {line}");
        let mut w = lock(&self.warnings);
        if w.len() < 64 {
            w.push(line);
        }
    }

    fn adapt_note(&self, line: String) {
        crate::agent::voice_debug(&line);
        let mut w = lock(&self.adapt_log);
        if w.len() < 64 {
            w.push(line);
        }
    }

    fn set_adapt_cos(&self, cos: Option<f32>) {
        self.adapt_cos
            .store(cos.unwrap_or(f32::NAN).to_bits(), Ordering::SeqCst);
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
    /// Re-opened after a `short` end: the buffer always rolls (pre-roll plus the latest check), so
    /// the resumed window scores the latest speech whatever `recheck_on_other` says.
    reopened: bool,
    /// Media bytes since the key's onset frame (gap frames included, pre-roll before the onset
    /// excluded). Separate from `speech_seen`, which only schedules windows.
    media_bytes: usize,
}

#[derive(Default)]
struct Feed {
    /// Frames pushed outside an utterance since the last utterance ended (a brief VAD gap).
    gap_bytes: usize,
    /// Per ended key, its media bytes at the end (capped at `BOOK_CAP`).
    media_end: VecDeque<(u64, usize)>,
    ring: Vec<u8>,
    cur: Option<Cur>,
    ended: VecDeque<(u64, Vec<u8>)>,
    /// Barge-ins deferred until their utterance's verdict, one per utterance key. Every check
    /// yields a verdict within the verifier's own timeout and that verdict removes its key, so
    /// it holds at most the live and closing utterances; `reset` clears it.
    /// Utterance key -> playback epoch the barge-in was raised against.
    pending_barge: BTreeMap<u64, u64>,
    /// Keys whose last check went out short (counted speech below `min_check_speech_ms`, or a
    /// resumed segment that ended before its window): (key, counted speech at SpeechEnd, the
    /// window the next scored check of the key is). Decided here at SpeechEnd, so a re-open never
    /// depends on the verifier. A short window is not a scored window: a re-opened key that ends
    /// short again keeps the same next window.
    shorts: VecDeque<(u64, usize, u32)>,
    /// `Msg::Check` sends so far (the attempt number of the last one).
    attempts: u64,
}

/// Per-agent speaker gate (feed side plus the handle to the verifier task).
pub struct SpeakerGate {
    cfg: SpeakerGateConfig,
    tx: UnboundedSender<Msg>,
    rx: Mutex<Option<UnboundedReceiver<Msg>>>,
    shared: Arc<Shared>,
    feed: Mutex<Feed>,
    runtime: Mutex<Runtime>,
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
    /// A gate built from `cfg`: active when `cfg.enabled` (the caller loaded the model), else
    /// inactive until [`Self::set_enabled`] and a model load ([`Self::finish_load`]).
    pub fn new(cfg: SpeakerGateConfig) -> Arc<Self> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = Runtime {
            enabled: cfg.enabled,
            loaded: cfg.enabled,
            loading: false,
            on_mismatch: cfg.on_mismatch,
            barge_in: cfg.barge_in,
            utts: VecDeque::new(),
        };
        Arc::new(Self {
            runtime: Mutex::new(runtime),
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
                superseded: std::sync::atomic::AtomicU64::new(0),
                adapt_commits: std::sync::atomic::AtomicU64::new(0),
                adapt_rejected: std::sync::atomic::AtomicU64::new(0),
                adapt_skipped: std::sync::atomic::AtomicU64::new(0),
                adapt_cos: std::sync::atomic::AtomicU32::new(f32::NAN.to_bits()),
                adapt_log: Mutex::new(Vec::new()),
                settles: std::sync::atomic::AtomicU64::new(0),
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

    /// Whether the adaptive reference is active: `adapt_reference` and `split_on_speaker_change`
    /// both on. With either off the reference is the enrolled embedding for the whole session.
    pub fn adapts_reference(&self) -> bool {
        self.cfg.adapt_reference && self.cfg.split_on_speaker_change
    }

    /// The final of `key` was emitted: the verifier task commits its adaptive-reference
    /// candidates when the utterance ended with no split and every scored window was `caller`.
    /// Never blocks (a channel send; the commit runs on the verifier task). No-op unless
    /// [`Self::adapts_reference`].
    pub fn settle(&self, key: u64) {
        if self.adapts_reference() {
            self.send(Msg::Settle { key });
        }
    }

    /// Adaptive-reference counters and the reference's cosine to the anchor.
    pub fn adapt_stats(&self) -> AdaptStats {
        let cos = f32::from_bits(self.shared.adapt_cos.load(Ordering::SeqCst));
        AdaptStats {
            commits: self.shared.adapt_commits.load(Ordering::SeqCst),
            rejected_anchor: self.shared.adapt_rejected.load(Ordering::SeqCst),
            skipped: self.shared.adapt_skipped.load(Ordering::SeqCst),
            cos_to_anchor: (!cos.is_nan()).then_some(cos),
        }
    }

    /// Adaptive-reference log lines so far (counts and cosine only).
    pub fn adapt_log(&self) -> Vec<String> {
        lock(&self.shared.adapt_log).clone()
    }

    /// Forget the enrolled voice; the next utterances enroll again.
    pub fn reset(&self) {
        self.shared.enrolled.store(false, Ordering::SeqCst);
        {
            let mut feed = lock(&self.feed);
            feed.ended.clear();
            feed.pending_barge.clear();
            feed.shorts.clear();
        }
        self.send(Msg::Reset);
    }

    fn current_settings(rt: &Runtime) -> UttSettings {
        let gate = if !rt.enabled {
            UttGate::Off
        } else if rt.loaded {
            UttGate::Active
        } else {
            UttGate::Loading
        };
        UttSettings {
            gate,
            on_mismatch: rt.on_mismatch,
            barge_in: rt.barge_in,
            cut: false,
        }
    }

    /// Settings of utterance `key`: taken at its first onset and kept for the whole utterance.
    fn snapshot_utt(&self, key: u64) -> UttSettings {
        let mut rt = lock(&self.runtime);
        if let Some((_, s)) = rt.utts.iter().find(|(k, _)| *k == key) {
            return *s;
        }
        let s = Self::current_settings(&rt);
        push_capped(&mut rt.utts, (key, s));
        s
    }

    /// Settings of utterance `key` (the current settings when it never had an onset).
    pub fn utt_settings(&self, key: u64) -> UttSettings {
        let rt = lock(&self.runtime);
        rt.utts
            .iter()
            .find(|(k, _)| *k == key)
            .map_or_else(|| Self::current_settings(&rt), |(_, s)| *s)
    }

    /// True when the verdict of `key` may hold or suppress its barge-in.
    pub fn holds_barge_for(&self, key: u64) -> bool {
        let s = self.utt_settings(key);
        s.gate == UttGate::Active && !s.cut && s.barge_in == SpeakerGateBargeIn::CallerOnly
    }

    /// True once the gate was disabled while `key` was a live active utterance.
    pub fn is_cut(&self, key: u64) -> bool {
        lock(&self.runtime)
            .utts
            .iter()
            .any(|(k, s)| *k == key && s.cut)
    }

    /// True while a model load thread runs, including one past its timeout (tests and
    /// diagnostics).
    #[doc(hidden)]
    pub fn is_loading(&self) -> bool {
        lock(&self.runtime).loading
    }

    /// True when the speaker model is loaded (the verifier can run).
    pub fn is_loaded(&self) -> bool {
        lock(&self.runtime).loaded
    }

    /// `onMismatch` / `bargeIn` for utterances that start from now on.
    pub fn set_policy(
        &self,
        on_mismatch: Option<SpeakerMismatchAction>,
        barge_in: Option<SpeakerGateBargeIn>,
    ) {
        let mut rt = lock(&self.runtime);
        if let Some(m) = on_mismatch {
            rt.on_mismatch = m;
        }
        if let Some(b) = barge_in {
            rt.barge_in = b;
        }
    }

    /// Enable from the next utterance onset. Returns true when the caller must start a model
    /// load (not loaded and no load in flight).
    pub fn set_enabled(&self) -> bool {
        let mut rt = lock(&self.runtime);
        rt.enabled = true;
        if rt.loaded || rt.loading {
            return false;
        }
        rt.loading = true;
        true
    }

    /// The model load passed its bound while its thread still runs: the gate is inactive from
    /// now on. `loading` stays set until the thread returns, so an enable meanwhile reuses this
    /// load instead of starting another thread (see [`Self::late_load_returned`]).
    pub fn load_timed_out(&self) {
        lock(&self.runtime).enabled = false;
    }

    /// The thread of a timed-out load returned. True when an enable came in meanwhile: the caller
    /// applies the result and settles with [`Self::finish_load`] (`loading` stays set until then).
    /// False: the result is dropped and the next enable starts a fresh load.
    pub fn late_load_returned(&self) -> bool {
        let mut rt = lock(&self.runtime);
        if rt.enabled {
            return true;
        }
        rt.loading = false;
        false
    }

    /// The model load finished. `ok == false` (failure): the gate stays inactive and a later
    /// enable retries.
    pub fn finish_load(&self, ok: bool) {
        let mut rt = lock(&self.runtime);
        rt.loading = false;
        if ok {
            rt.loaded = true;
        } else {
            rt.enabled = false;
        }
    }

    /// Config errors for enabling this gate (`modelPath`, threshold, windows).
    pub fn validate_enable(&self) -> Result<(), String> {
        let mut cfg = self.cfg.clone();
        cfg.enabled = true;
        cfg.validate()
    }

    /// Disable, synchronously: utterances that start from now are off, every utterance that
    /// started active is marked cut under the same lock (its final keeps the verdict it has, else
    /// `unknown` / `no_check`, and a held barge-in it settles fires), the live utterance stops
    /// collecting PCM (no further check), and finals waiting for a verdict are woken. Once this
    /// returns, every final reads `cut`. Enrollment is kept. See [`Self::cut_live`] for the held
    /// barge-ins no final owns.
    pub fn set_disabled(&self) {
        {
            let mut rt = lock(&self.runtime);
            rt.enabled = false;
            for (_, s) in rt.utts.iter_mut() {
                if s.gate == UttGate::Active {
                    s.cut = true;
                }
            }
        }
        {
            let mut feed = lock(&self.feed);
            feed.cur = None;
            feed.shorts.clear();
        }
        self.shared.notify.notify_waiters();
    }

    /// After [`Self::set_disabled`]: takes the held barge-ins (key, playback epoch) of cut
    /// utterances that no reading final owns; the caller settles them. A barge-in a final already
    /// took is settled by that final (it reads `cut`), so each fires at most once.
    pub fn cut_live(&self) -> Vec<(u64, u64)> {
        let cut: Vec<u64> = lock(&self.runtime)
            .utts
            .iter()
            .filter(|(_, s)| s.cut)
            .map(|(k, _)| *k)
            .collect();
        let mut feed = lock(&self.feed);
        let keys: Vec<u64> = feed
            .pending_barge
            .keys()
            .copied()
            .filter(|k| cut.contains(k) && !self.recheck_closed(*k))
            .collect();
        keys.into_iter()
            .filter_map(|k| feed.pending_barge.remove(&k).map(|e| (k, e)))
            .collect()
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

    /// Test hook: verdicts dropped because a re-open superseded their check.
    #[doc(hidden)]
    pub fn superseded_verdicts(&self) -> u64 {
        self.shared.superseded.load(Ordering::SeqCst)
    }

    /// Test hook: bytes currently buffered for the live utterance.
    #[doc(hidden)]
    pub fn live_buffer_bytes(&self) -> usize {
        lock(&self.feed).cur.as_ref().map_or(0, |c| c.buf.len())
    }

    /// Enrolled with re-checks on: the buffer rolls (pre-roll plus the latest window) once the
    /// first check went out.
    fn rolling(&self) -> bool {
        (self.cfg.recheck_on_other || self.cfg.split_on_speaker_change) && self.is_enrolled()
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
        let settings = self.snapshot_utt(key);
        let mut feed = lock(&self.feed);
        if settings.gate != UttGate::Active || settings.cut {
            // Not scored: no check, no enrollment PCM. Frames feed the pre-roll ring.
            if feed.cur.as_ref().is_some_and(|c| c.key != key) {
                self.end_locked(&mut feed);
            }
            return;
        }
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
        let reopened = self.reopen_short(&mut feed, key);
        // A key that ended before continues its media clock across the gap.
        let media_bytes = feed
            .media_end
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map_or(0, |(_, m)| m + feed.gap_bytes);
        let cur = match resumed {
            Some((k, buf)) => match reopened {
                Some(cur) => Cur {
                    buf,
                    pre: 0,
                    media_bytes,
                    ..cur
                },
                None => {
                    let ws = self.windows_sent_for(k);
                    Cur {
                        key: k,
                        buf,
                        pre: 0,
                        sent: true,
                        windows_sent: ws,
                        speech_seen: ws as usize * check,
                        rechecks_closed: self.recheck_closed(k),
                        reopened: false,
                        media_bytes,
                    }
                }
            },
            None => {
                let buf = std::mem::take(&mut feed.ring);
                let pre = buf.len();
                if let Some(cur) = reopened {
                    Cur {
                        buf,
                        pre,
                        media_bytes,
                        ..cur
                    }
                } else {
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
                        reopened: false,
                        media_bytes,
                    }
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
        let attempt;
        {
            let mut feed = lock(&self.feed);
            match feed.cur.as_mut() {
                Some(cur) => {
                    let speech = cur.buf.len() - cur.pre;
                    let roll = rolling || cur.reopened;
                    if roll {
                        cur.buf.extend_from_slice(frame);
                    } else if speech < cap {
                        let room = cap - speech;
                        cur.buf.extend_from_slice(&frame[..frame.len().min(room)]);
                    }
                    cur.speech_seen += frame.len();
                    cur.media_bytes += frame.len();
                    // Window k goes out at k × check of counted speech: a first check at `check`,
                    // a re-opened `short` key at (windows sent + 1) × check, its short part
                    // included. The clip is the latest pre-roll + check.
                    if !cur.sent
                        && cur.buf.len() - cur.pre >= check
                        && cur.speech_seen >= (cur.windows_sent as usize + 1) * check
                    {
                        cur.sent = true;
                        cur.windows_sent += 1;
                        let from = cur.buf.len().saturating_sub(keep);
                        to_send = Some((
                            cur.key,
                            cur.buf[from..].to_vec(),
                            check,
                            false,
                            cur.windows_sent,
                            (cur.media_bytes / BYTES_PER_MS) as u32,
                        ));
                    } else if (self.cfg.recheck_on_other || self.cfg.split_on_speaker_change)
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
                        to_send = Some((
                            cur.key,
                            pcm,
                            check,
                            true,
                            cur.windows_sent,
                            (cur.media_bytes / BYTES_PER_MS) as u32,
                        ));
                    }
                    if roll && (cur.sent || cur.reopened) && cur.buf.len() > keep {
                        let n = cur.buf.len() - keep;
                        cur.buf.drain(..n);
                        cur.pre = 0;
                    }
                }
                None => {
                    feed.gap_bytes += frame.len();
                    feed.ring.extend_from_slice(frame);
                    let max = PRE_ROLL_MS * BYTES_PER_MS;
                    if feed.ring.len() > max {
                        let drop_n = feed.ring.len() - max;
                        feed.ring.drain(..drop_n);
                    }
                }
            }
            if to_send.is_some() {
                feed.attempts += 1;
            }
            attempt = feed.attempts;
        }
        if let Some((key, pcm, speech_bytes, recheck, window, end_ms)) = to_send {
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
                attempt,
                end_ms,
            });
        }
    }

    fn mark_sent(&self, key: u64) {
        let mut book = lock(&self.shared.book);
        if !book.sent.contains(&key) {
            push_capped(&mut book.sent, key);
        }
    }

    /// A `short` verdict is provisional while the key can still rejoin. When the last check of
    /// `key` went out short (feed-side record from SpeechEnd) and its final has not read the
    /// verdict, the key is re-opened: any stored verdict is dropped, a late one for the short
    /// window will be dropped by the verifier, and the returned `Cur` (unsent, window count and
    /// counted speech continued) checks the rejoined speech. Caller holds the feed lock and fills
    /// in `buf` / `pre`.
    fn reopen_short(&self, feed: &mut Feed, key: u64) -> Option<Cur> {
        let i = feed.shorts.iter().position(|(k, _, _)| *k == key)?;
        let (_, speech_seen, next_window) = feed.shorts.remove(i)?;
        if self.recheck_closed(key) {
            return None;
        }
        {
            let mut book = lock(&self.shared.book);
            book.verdicts.retain(|(k, _)| *k != key);
            book.reasons.retain(|(k, _)| *k != key);
            book.reopened_after.retain(|(k, _)| *k != key);
            push_capped(&mut book.reopened_after, (key, feed.attempts));
        }
        Some(Cur {
            key,
            buf: Vec::new(),
            pre: 0,
            sent: false,
            // From the feed record only, never from verifier state.
            windows_sent: next_window - 1,
            speech_seen,
            rechecks_closed: false,
            reopened: true,
            media_bytes: 0,
        })
    }

    /// Test hook: the live utterance's key, windows sent and counted speech in ms.
    #[doc(hidden)]
    pub fn live_check_state(&self) -> Option<(u64, u32, usize)> {
        lock(&self.feed)
            .cur
            .as_ref()
            .map(|c| (c.key, c.windows_sent, c.speech_seen / BYTES_PER_MS))
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
        feed.media_end.retain(|(k, _)| *k != cur.key);
        feed.media_end.push_back((cur.key, cur.media_bytes));
        while feed.media_end.len() > BOOK_CAP {
            feed.media_end.pop_front();
        }
        feed.gap_bytes = 0;
        if !cur.sent {
            self.mark_sent(cur.key);
            // A re-opened key that ends before (windows sent + 1) × check of counted speech is
            // not scored (`short` again): a window is never scored below its bound.
            let speech = if cur.windows_sent > 0 {
                0
            } else {
                (cur.buf.len() - cur.pre).min(self.check_bytes())
            };
            if speech < self.cfg.min_check_speech_ms as usize * BYTES_PER_MS {
                // The short first check used window 1, so the next scored window is 2. A
                // re-opened key that ends short again keeps its next window: a short window does
                // not use up a window number.
                let next_window = if cur.reopened {
                    cur.windows_sent + 1
                } else {
                    cur.windows_sent.max(1) + 1
                };
                feed.shorts.retain(|(k, _, _)| *k != cur.key);
                feed.shorts
                    .push_back((cur.key, cur.speech_seen, next_window));
                while feed.shorts.len() > BOOK_CAP {
                    feed.shorts.pop_front();
                }
            }
            feed.attempts += 1;
            self.send(Msg::Check {
                key: cur.key,
                pcm: cur.buf[..cur.pre + speech].to_vec(),
                speech_bytes: speech,
                recheck: false,
                window: cur.windows_sent + 1,
                attempt: feed.attempts,
                end_ms: (cur.media_bytes / BYTES_PER_MS) as u32,
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
        let label = book
            .verdicts
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, o)| *o);
        let split = self.cfg.split_on_speaker_change;
        // With the speaker-change watch on, a `caller` utterance is watched too, until a change
        // was confirmed for it.
        (label == Some(Outcome::Other) || (split && label == Some(Outcome::Caller)))
            && !(split && book.split_keys.contains(&key))
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
    pub fn hold_for_recheck(&self, key: u64, epoch: u64) -> bool {
        let mut feed = lock(&self.feed);
        if self.recheck_open_locked(&feed, key) {
            feed.pending_barge.insert(key, epoch);
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

    /// Windows of `key` scored so far (first check = 1; 0 when none settled), as the final
    /// reports them. With the speaker-change watch on, windows scored only for the watch are not
    /// counted: the value is what the gate with the watch off reports for the same outcomes.
    pub fn windows_scored(&self, key: u64) -> u32 {
        let book = lock(&self.shared.book);
        let per_key = if self.cfg.split_on_speaker_change {
            &book.reported_windows
        } else {
            &book.windows_scored
        };
        per_key
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(0, |(_, w)| *w)
    }

    /// Test hook: windows of `key` scored only because of the speaker-change watch.
    #[doc(hidden)]
    pub fn watch_windows_scored(&self, key: u64) -> u32 {
        lock(&self.shared.book)
            .watch_windows
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
            // A `short` verdict is provisional while its key can still rejoin: hold the
            // barge-in until a rejoined check scores or the utterance closes short.
            Some(Outcome::Unknown)
                if self.stored_unknown_reason(key) == Some(UNKNOWN_SHORT)
                    && self.short_can_rejoin(key) =>
            {
                BargeDecision::Defer
            }
            Some(_) => BargeDecision::Proceed,
            None => BargeDecision::Defer,
        }
    }

    /// True while `key` ended `short` (feed-side record from SpeechEnd, see [`Self::reopen_short`])
    /// and its final has not started reading the verdict, so the same utterance can still rejoin.
    pub fn short_can_rejoin(&self, key: u64) -> bool {
        lock(&self.feed).shorts.iter().any(|(k, _, _)| *k == key) && !self.recheck_closed(key)
    }

    /// True when `verdict` vetoes the barge-in of its utterance: `other`, or (opt-in
    /// `suppress_short_barge_in`) `unknown` because the speech was below `min_check_speech_ms`.
    /// Every other `unknown` fails open.
    /// A re-check releases the barge-in only when it says `caller`; any other re-check result
    /// (including `unknown`) keeps the first `other` verdict's veto.
    /// The one allow rule for a held barge-in, used by both routes that can own it (the verdict
    /// handler with the verdict it received, the final with the verdict it read): fire unless the
    /// verdict vetoes it. Speaking, epoch and protected TTS are checked by the agent afterwards.
    pub fn barge_allowed(&self, verdict: &Verdict) -> bool {
        !self.vetoes_barge(verdict)
    }

    /// The verdict a final of `key` read as `outcome`, in the shape [`Self::barge_allowed`] takes:
    /// `recheck` when a re-check settled, the `unknown` reason when the outcome is `unknown`.
    pub fn verdict_read_by_final(&self, key: u64, outcome: Outcome) -> Verdict {
        let reason = (outcome == Outcome::Unknown).then(|| self.unknown_reason(key));
        Verdict {
            key,
            outcome,
            timed_out: reason == Some(UNKNOWN_TIMEOUT),
            reason,
            recheck: self.rechecked(key),
            window: self.windows_scored(key),
        }
    }

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
        let s = self.utt_settings(key);
        if s.gate == UttGate::Loading {
            return UNKNOWN_LOADING;
        }
        if s.cut {
            return UNKNOWN_NO_CHECK;
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

    pub fn set_pending_barge(&self, key: u64, epoch: u64) {
        lock(&self.feed).pending_barge.insert(key, epoch);
    }

    /// True while a barge-in for `key` waits for its verdict (not cleared).
    pub fn has_pending_barge(&self, key: u64) -> bool {
        lock(&self.feed).pending_barge.contains_key(&key)
    }

    /// True once the final of `key` has started reading its verdict ([`Self::close_recheck`]).
    pub fn final_reading(&self, key: u64) -> bool {
        self.recheck_closed(key)
    }

    /// True (and cleared) when a barge-in for `key` waits for its verdict.
    pub fn take_pending_barge(&self, key: u64) -> Option<u64> {
        lock(&self.feed).pending_barge.remove(&key)
    }

    /// Confirmed speaker changes since the last call, oldest first (only with
    /// `split_on_speaker_change`). Stored before the verdict that completed the run is delivered
    /// to `on_verdict`, so a verdict handler can take it.
    pub fn take_speaker_changes(&self) -> Vec<SpeakerChange> {
        lock(&self.shared.book).changes.drain(..).collect()
    }

    /// U2 of an `other` -> `caller` split starts as the caller (only with
    /// `split_on_speaker_change`): books the confirming `caller` verdict for `key` as window 1
    /// without embedding the seed (a seed that starts in the bed must not cascade into another
    /// split), arms `key`'s own watch on the verifier task, and returns the verdict (for the caller's
    /// bookkeeping only; it does not release anything). U2's barge-in is released by
    /// [`Self::barge_decision`], which reads the booked `Caller` outcome for `key`. Call it before
    /// the first [`Self::begin`] of `key`, so the
    /// feed sees the first check as sent and the next window follows `check_speech_ms` of U2's
    /// speech.
    pub fn inherit_caller(&self, key: u64) -> Verdict {
        {
            let mut book = lock(&self.shared.book);
            book.verdicts.retain(|(k, _)| *k != key);
            push_capped(&mut book.verdicts, (key, Outcome::Caller));
            if !book.sent.contains(&key) {
                push_capped(&mut book.sent, key);
            }
            book.windows_scored.retain(|(k, _)| *k != key);
            push_capped(&mut book.windows_scored, (key, 1));
            book.reported_windows.retain(|(k, _)| *k != key);
            push_capped(&mut book.reported_windows, (key, 1));
        }
        self.send(Msg::Inherit { key });
        Verdict {
            key,
            outcome: Outcome::Caller,
            timed_out: false,
            reason: None,
            recheck: false,
            window: 1,
        }
    }

    /// The verdict a final of `key` can carry right now: the stored verdict, or `Unknown` when no
    /// check was sent. `None` while a sent check or re-check has not settled (use
    /// [`Self::await_outcome`] off the frame path).
    pub fn settled_outcome(&self, key: u64) -> Option<Outcome> {
        if self.is_cut(key) {
            return Some(self.outcome_for(key).unwrap_or(Outcome::Unknown));
        }
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
            if self.is_cut(key) {
                return self.outcome_for(key).unwrap_or(Outcome::Unknown);
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

/// Watch on: count `window` of `key` as a reported window when the gate with the watch off would
/// have scored it (the first window; a re-check while the stored verdict is still `other`), else
/// as a watch window. Call it before the verdict book takes the window's own upgrade: the stored
/// label is then the watch-off label, and the `caller` re-check that upgrades the key counts.
fn book_reported_window(book: &mut Book, key: u64, recheck: bool, window: u32) {
    let counts = !recheck || {
        let label = book
            .verdicts
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, o)| *o);
        label == Some(Outcome::Other)
    };
    if counts {
        if let Some(e) = book.reported_windows.iter_mut().find(|(k, _)| *k == key) {
            e.1 = e.1.max(window);
        } else {
            push_capped(&mut book.reported_windows, (key, window));
        }
    } else if let Some(e) = book.watch_windows.iter_mut().find(|(k, _)| *k == key) {
        e.1 += 1;
    } else {
        push_capped(&mut book.watch_windows, (key, 1));
    }
}

/// Speaker-change watch of one utterance key (verifier task only).
struct Watch {
    /// Label of the first scored window.
    label: Outcome,
    /// Consecutive windows of the opposite label so far.
    run: u32,
    /// `caller` label: span, end and clip of the first window of the opposite run.
    first: Option<(u32, u32, Vec<u8>)>,
    /// `other` label: span, end and clip of the last scored window (the backward range of an
    /// `other` -> `caller` split starts at its start).
    prev: Option<(u32, u32, Vec<u8>)>,
    /// A change was reported: the key is not watched further.
    done: bool,
    /// An `other` -> `caller` change whose stage 1 found no quiet run of
    /// [`SPLIT_QUIET_RUN_MIN_MS`]: the audio stage 2 localizes on (taken once by the verifier).
    refine: Option<Refine>,
}

impl Watch {
    fn armed(label: Outcome, prev: Option<(u32, u32, Vec<u8>)>) -> Self {
        Self {
            label,
            run: 0,
            first: None,
            prev,
            done: false,
            refine: None,
        }
    }
}

/// Audio for the stage 2 split-point localization.
struct Refine {
    tl: Timeline,
    /// Start of the range (the previous scored window's start) and the end of the audio held.
    lo: i64,
    audio_end: i64,
}

/// Mono s16le 16 kHz audio placed on the utterance clock (ms since the key's onset frame).
struct Timeline {
    start_ms: i64,
    pcm: Vec<u8>,
}

impl Timeline {
    /// Merges clips that each end at their `end_ms` (a later part overwrites an earlier one; a
    /// gap is silent).
    fn from_clips(parts: &[(u32, &[u8])]) -> Self {
        let span = |(end, clip): &(u32, &[u8])| {
            let len = (clip.len() / BYTES_PER_MS) as i64;
            (i64::from(*end) - len, i64::from(*end))
        };
        let start_ms = parts.iter().map(|p| span(p).0).min().unwrap_or(0);
        let end_ms = parts.iter().map(|p| span(p).1).max().unwrap_or(0);
        let mut pcm = vec![0_u8; ((end_ms - start_ms).max(0) as usize) * BYTES_PER_MS];
        for part in parts {
            let at = ((span(part).0 - start_ms) as usize) * BYTES_PER_MS;
            let n = (part.1.len() / BYTES_PER_MS) * BYTES_PER_MS;
            pcm[at..at + n].copy_from_slice(&part.1[..n]);
        }
        Self { start_ms, pcm }
    }

    fn end_ms(&self) -> i64 {
        self.start_ms + (self.pcm.len() / BYTES_PER_MS) as i64
    }

    /// `[a_ms, b_ms)` clamped to the audio held.
    fn slice(&self, a_ms: i64, b_ms: i64) -> &[u8] {
        let a = (a_ms.clamp(self.start_ms, self.end_ms()) - self.start_ms) as usize * BYTES_PER_MS;
        let b = (b_ms.clamp(self.start_ms, self.end_ms()) - self.start_ms) as usize * BYTES_PER_MS;
        &self.pcm[a..b.max(a)]
    }

    /// Mean square of the 20 ms frame starting at `t_ms` (which must lie inside the audio).
    fn frame_power(&self, t_ms: i64) -> f64 {
        let f = self.slice(t_ms, t_ms + SPLIT_FRAME_MS as i64);
        let n = f.len() / 2;
        if n == 0 {
            return 0.0;
        }
        f.as_chunks::<2>()
            .0
            .iter()
            .map(|b| {
                let v = f64::from(i16::from_le_bytes(*b));
                v * v
            })
            .sum::<f64>()
            / n as f64
    }

    /// Stage 1 over `[lo, hi)`: (end of the longest quiet run, its length in ms). Power is
    /// smoothed over [`SPLIT_SMOOTH_FRAMES`] frames; a frame is quiet below the midpoint (dB) of
    /// the range's 10th and 90th percentile; ties go to the later run. `None` without a frame.
    fn longest_quiet_run(&self, lo: i64, hi: i64) -> Option<(i64, i64)> {
        let lo = lo.max(self.start_ms).max(0);
        let hi = hi.min(self.end_ms());
        let n = ((hi - lo).max(0) as usize) / SPLIT_FRAME_MS;
        if n == 0 {
            return None;
        }
        let db = |p: f64| 10.0 * p.max(1e-12).log10();
        let raw: Vec<f64> = (0..n)
            .map(|i| self.frame_power(lo + (i * SPLIT_FRAME_MS) as i64))
            .collect();
        let smooth: Vec<f64> = (0..n)
            .map(|i| {
                let a = i.saturating_sub(SPLIT_SMOOTH_FRAMES / 2);
                let b = (i + SPLIT_SMOOTH_FRAMES / 2 + 1).min(n);
                db(raw[a..b].iter().sum::<f64>() / (b - a) as f64)
            })
            .collect();
        let mut sorted = smooth.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let pct = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        let mid = (pct(0.1) + pct(0.9)) / 2.0;
        let (mut best_len, mut best_end, mut run) = (0_usize, lo, 0_usize);
        for (i, v) in smooth.iter().enumerate() {
            if *v < mid {
                run += 1;
                if run >= best_len {
                    best_len = run;
                    best_end = lo + ((i + 1) * SPLIT_FRAME_MS) as i64;
                }
            } else {
                run = 0;
            }
        }
        Some((best_end, (best_len * SPLIT_FRAME_MS) as i64))
    }

    /// Start of the lowest-energy 20 ms frame within `+-r` ms of `p` (ties: nearest `p`, then the
    /// earlier one). Frames outside the audio (and before the onset) are not candidates.
    fn refine_point(&self, p: i64, r: i64) -> i64 {
        let mut best: Option<(f64, i64, i64)> = None;
        let mut t = p - r;
        while t <= p + r {
            if t >= self.start_ms.max(0) && t + SPLIT_FRAME_MS as i64 <= self.end_ms() {
                let cand = (self.frame_power(t), (t - p).abs(), t);
                if best.is_none_or(|b| cand.partial_cmp(&b) == Some(std::cmp::Ordering::Less)) {
                    best = Some(cand);
                }
            }
            t += SPLIT_FRAME_MS as i64;
        }
        best.map_or(p, |b| b.2)
    }
}

/// One scored window for the watch. The first scored (non-re-check) window labels the key. Each
/// later window of the opposite label extends a run, any other window (same label, unscored)
/// ends it. `caller` -> `other` reports after [`SPEAKER_CHANGE_CONFIRM_WINDOWS`] in a row;
/// `other` -> `caller` after [`OTHER_TO_CALLER_CONFIRM_WINDOWS`] (one), and holds the audio for
/// stage 2 when stage 1 finds no quiet run (see [`Watch::refine`]). `span` is the window's
/// `(start_ms, end_ms)` on the utterance clock; `clip` is the PCM that was scored, ending at
/// `end_ms`.
fn watch_step(
    watches: &mut VecDeque<(u64, Watch)>,
    key: u64,
    outcome: Outcome,
    recheck: bool,
    span: (u32, u32),
    clip: Vec<u8>,
) -> Option<SpeakerChange> {
    let scored = matches!(outcome, Outcome::Caller | Outcome::Other);
    if !recheck {
        if scored {
            watches.retain(|(k, _)| *k != key);
            let prev = (outcome == Outcome::Other).then_some((span.0, span.1, clip));
            watches.push_back((key, Watch::armed(outcome, prev)));
            while watches.len() > BOOK_CAP {
                watches.pop_front();
            }
        }
        return None;
    }
    let w = &mut watches.iter_mut().find(|(k, _)| *k == key)?.1;
    if w.done {
        return None;
    }
    if !scored || outcome == w.label {
        w.run = 0;
        w.first = None;
        if scored && w.label == Outcome::Other {
            w.prev = Some((span.0, span.1, clip));
        }
        return None;
    }
    if w.label == Outcome::Other {
        // `other` -> `caller`: confirmed by the first `caller` window.
        let (split_at_ms, refine) = other_to_caller_split(w.prev.take(), span, &clip);
        w.done = true;
        w.refine = refine;
        return Some(SpeakerChange {
            key,
            from: Outcome::Other,
            to: Outcome::Caller,
            split_at_ms,
        });
    }
    if w.run == 0 {
        w.first = Some((span.0, span.1, clip));
    }
    w.run += 1;
    if w.run < SPEAKER_CHANGE_CONFIRM_WINDOWS {
        return None;
    }
    w.done = true;
    let (start_ms, end_ms, clip) = w.first.take()?;
    Some(SpeakerChange {
        key,
        from: w.label,
        to: outcome,
        split_at_ms: split_point(start_ms, end_ms, &clip),
    })
}

/// Stage 1 of the `other` -> `caller` split point. The range runs from the previous scored
/// window's start to the first `caller` window's start + [`SPLIT_SEARCH_MS`]; the split is the end
/// of its longest quiet run. Returns the point and, when the run is shorter than
/// [`SPLIT_QUIET_RUN_MIN_MS`], the audio for stage 2. Without a previous window that tiles into
/// this one (an unscored window in between), the range is the window start +- [`SPLIT_SEARCH_MS`]
/// of the window's own clip and there is no stage 2.
fn other_to_caller_split(
    prev: Option<(u32, u32, Vec<u8>)>,
    span: (u32, u32),
    clip: &[u8],
) -> (u32, Option<Refine>) {
    let hi = i64::from(span.0) + i64::from(SPLIT_SEARCH_MS);
    let prev = prev.filter(|(_, end, _)| *end + 2 * SPLIT_FRAME_MS as u32 >= span.0);
    let (tl, lo) = match prev.as_ref() {
        Some((start, end, pclip)) => (
            Timeline::from_clips(&[(*end, pclip), (span.1, clip)]),
            i64::from(*start),
        ),
        None => (
            Timeline::from_clips(&[(span.1, clip)]),
            i64::from(span.0.saturating_sub(SPLIT_SEARCH_MS)),
        ),
    };
    let Some((point, run)) = tl.longest_quiet_run(lo, hi) else {
        return (span.0, None);
    };
    let point_ms = point.max(0) as u32;
    if run >= i64::from(SPLIT_QUIET_RUN_MIN_MS) || prev.is_none() {
        return (point_ms, None);
    }
    let audio_end = tl.end_ms();
    (
        point_ms,
        Some(Refine {
            tl,
            lo: lo.max(0),
            audio_end,
        }),
    )
}

/// Stage 2 of the `other` -> `caller` split point: embeds [`SPLIT_STAGE2_SUB_MS`] sub-windows on a
/// [`SPLIT_STAGE2_HOP_MS`] hop from the start of the range, and refines the start of the first
/// one that scores `caller` to the lowest-energy frame within [`SPLIT_STAGE2_REFINE_MS`]. Runs on
/// the verifier task, inside the window's own verify budget: the whole stage has `verify` minus
/// what the window's verify (started at `window_started`) already spent, each embed
/// `min(verify, left)`, so the window's verdict never lands later than `verify_timeout_ms` after
/// its check started (a final awaits only that long). No budget left skips the stage. `None` (the
/// caller keeps stage 1's point) on a budget stop, an embed failure, or when no sub-window is
/// `caller`; `waitedMs` is logged either way.
#[allow(clippy::too_many_arguments)]
async fn stage2_split(
    embedder: &Arc<dyn SpeakerEmbedder>,
    reference: &[f32],
    threshold: f32,
    refine: &Refine,
    verify: Duration,
    window_started: Instant,
    shared: &Shared,
    key: u64,
) -> Option<u32> {
    let started = Instant::now();
    let total = verify.saturating_sub(window_started.elapsed());
    let sub = i64::from(SPLIT_STAGE2_SUB_MS);
    let mut found = None;
    let mut failed = None;
    let mut embeds = 0_u32;
    let mut t = refine.lo;
    while t + sub <= refine.audio_end {
        let left = total.saturating_sub(started.elapsed());
        if left.is_zero() {
            failed = Some("total bound");
            break;
        }
        let pcm = refine.tl.slice(t, t + sub).to_vec();
        embeds += 1;
        match embed_bounded(embedder, pcm, verify.min(left)).await {
            Ok(Some(v)) => {
                if cosine(reference, &v).is_some_and(|score| score >= threshold) {
                    found = Some(t);
                    break;
                }
            }
            Ok(None) => {}
            Err(EmbedFail::Timeout) => {
                // The embed ran into the stage budget (always below `verify` once the window's own
                // embed took time), or into `verify` itself.
                failed = Some(if left < verify {
                    "total bound"
                } else {
                    "embed timeout"
                });
                break;
            }
            Err(EmbedFail::Error) => {
                failed = Some("embed failed");
                break;
            }
        }
        t += i64::from(SPLIT_STAGE2_HOP_MS);
    }
    let waited_ms = started.elapsed().as_millis();
    if let Some(why) = failed {
        shared.warn(format!(
            "speaker gate split stage 2 {why} key={key} embeds={embeds} waitedMs={waited_ms}; using stage 1"
        ));
        return None;
    }
    crate::agent::voice_debug(format!(
        "speaker gate split stage 2 key={key} embeds={embeds} found={} waitedMs={waited_ms}",
        found.is_some()
    ));
    let p = found?;
    let at = refine
        .tl
        .refine_point(p, i64::from(SPLIT_STAGE2_REFINE_MS))
        .max(0);
    Some(at as u32)
}

/// Start (utterance clock, ms) of the lowest-energy 20 ms frame of `clip` within
/// [`SPLIT_SEARCH_MS`] of `start_ms`. The clip ends at `end_ms`, so its frames sit at
/// `end_ms - clip_ms + n * 20`; frames before the onset (pre-roll) are not candidates. Ties go to
/// the frame nearest `start_ms`, then the earlier one. Falls back to `start_ms`.
fn split_point(start_ms: u32, end_ms: u32, clip: &[u8]) -> u32 {
    let frame_bytes = SPLIT_FRAME_MS * BYTES_PER_MS;
    let clip_ms = (clip.len() / BYTES_PER_MS) as i64;
    let clip_start = i64::from(end_ms) - clip_ms;
    let lo = (i64::from(start_ms) - i64::from(SPLIT_SEARCH_MS)).max(0);
    let hi = i64::from(start_ms) + i64::from(SPLIT_SEARCH_MS);
    let mut best: Option<(u64, i64, i64)> = None;
    for (n, frame) in clip.chunks_exact(frame_bytes).enumerate() {
        let t = clip_start + (n * SPLIT_FRAME_MS) as i64;
        if t < lo || t > hi {
            continue;
        }
        let energy: u64 = frame
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| {
                let v = i64::from(i16::from_le_bytes(*b));
                (v * v) as u64
            })
            .sum();
        let cand = (energy, (t - i64::from(start_ms)).abs(), t);
        if best.is_none_or(|b| cand < b) {
            best = Some(cand);
        }
    }
    best.map_or(start_ms, |b| b.2 as u32)
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
    let mut watches: VecDeque<(u64, Watch)> = VecDeque::new();
    // Adaptive reference (decision e): the enrolled embedding stays as the anchor; candidates are
    // kept per key until its final settles it. In memory only, never logged or emitted.
    let adapt = cfg.adapt_reference && cfg.split_on_speaker_change;
    let mut anchor: Option<Vec<f32>> = None;
    let mut adapt_keys: VecDeque<(u64, AdaptKey)> = VecDeque::new();

    while let Some(msg) = rx.recv().await {
        let pending = shared
            .backlog
            .fetch_sub(1, Ordering::SeqCst)
            .saturating_sub(1);
        if pending == 0 {
            shared.backlog_logged.store(false, Ordering::SeqCst);
        }
        match msg {
            Msg::Inherit { key } => {
                // U2 of an `other` -> `caller` split starts as the caller, with its own watch.
                watches.retain(|(k, _)| *k != key);
                watches.push_back((key, Watch::armed(Outcome::Caller, None)));
                while watches.len() > BOOK_CAP {
                    watches.pop_front();
                }
            }
            Msg::Reset => {
                watches.clear();
                reference = None;
                anchor = None;
                adapt_keys.clear();
                shared.set_adapt_cos(None);
                enroll_pcm.clear();
                shared.enrolled.store(false, Ordering::SeqCst);
            }
            Msg::Settle { key } => {
                shared.settles.fetch_add(1, Ordering::SeqCst);
                let Some(i) = adapt_keys.iter().position(|(k, _)| *k == key) else {
                    continue;
                };
                let Some((_, state)) = adapt_keys.remove(i) else {
                    continue;
                };
                if !adapt {
                    continue;
                }
                let split = lock(&shared.book).split_keys.contains(&key);
                let (Some(r), Some(a)) = (reference.as_ref(), anchor.as_ref()) else {
                    continue;
                };
                if split || state.dirty || state.candidates.is_empty() {
                    let n = shared.adapt_skipped.fetch_add(1, Ordering::SeqCst) + 1;
                    shared.adapt_note(format!(
                        "speaker gate reference kept key={key} split={split} clean={} candidates={} skipped={n}",
                        !state.dirty,
                        state.candidates.len()
                    ));
                    continue;
                }
                match adapt_step(r, a, &state.candidates) {
                    AdaptStep::Commit {
                        reference: new,
                        cos_to_anchor,
                    } => {
                        reference = Some(new);
                        shared.set_adapt_cos(Some(cos_to_anchor));
                        let n = shared.adapt_commits.fetch_add(1, Ordering::SeqCst) + 1;
                        shared.adapt_note(format!(
                            "speaker gate reference adapted key={key} candidates={} commits={n} cos_to_anchor={cos_to_anchor:.3}",
                            state.candidates.len()
                        ));
                    }
                    AdaptStep::RejectAnchor { cos_to_anchor } => {
                        let n = shared.adapt_rejected.fetch_add(1, Ordering::SeqCst) + 1;
                        shared.adapt_note(format!(
                            "speaker gate reference update rejected key={key} candidates={} rejected={n} cos_to_anchor={cos_to_anchor:.3}",
                            state.candidates.len()
                        ));
                    }
                    AdaptStep::Skip => {
                        shared.adapt_skipped.fetch_add(1, Ordering::SeqCst);
                    }
                }
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
                        if adapt {
                            anchor = Some(v.clone());
                            shared.set_adapt_cos(Some(1.0));
                        }
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
                attempt,
                end_ms,
            } => {
                let superseded = lock(&shared.book)
                    .reopened_after
                    .iter()
                    .find(|(k, _)| *k == key)
                    .is_some_and(|(_, a)| attempt <= *a);
                if superseded {
                    // A check sent before its key was re-opened by a rejoin: the resumed check
                    // answers for the key now. A re-open needs a short record, which only a
                    // `!sent` SpeechEnd writes, and a re-opened key that sends a window from the
                    // frame path is `sent` and never writes another. So every attempt up to the
                    // re-open is a short-end check, whose verdict is `short` or `not_enrolled`.
                    debug_assert!(
                        reference.is_none() || speech_bytes < min_check_bytes,
                        "superseded check would be scored (key={key} attempt={attempt})"
                    );
                    crate::agent::voice_debug(format!(
                        "speaker gate drops superseded verdict key={key} window={window} attempt={attempt}"
                    ));
                    shared.superseded.fetch_add(1, Ordering::SeqCst);
                    shared.notify.notify_waiters();
                    continue;
                }
                let started = Instant::now();
                let clip = cfg.split_on_speaker_change.then(|| pcm.clone());
                // Adaptive reference: the window's embedding when it scored at least
                // `threshold + ADAPT_MARGIN`.
                let mut candidate: Option<Vec<f32>> = None;
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
                                if adapt && score >= cfg.threshold + ADAPT_MARGIN {
                                    candidate = Some(v);
                                }
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
                if adapt && reference.is_some() {
                    let i = match adapt_keys.iter().position(|(k, _)| *k == key) {
                        Some(i) => i,
                        None => {
                            adapt_keys.push_back((key, AdaptKey::default()));
                            while adapt_keys.len() > BOOK_CAP {
                                adapt_keys.pop_front();
                            }
                            adapt_keys.len() - 1
                        }
                    };
                    let state = &mut adapt_keys[i].1;
                    if outcome != Outcome::Caller {
                        state.dirty = true;
                    } else if let Some(v) = candidate.take() {
                        if state.candidates.len() < ADAPT_MAX_CANDIDATES {
                            state.candidates.push(v);
                        }
                    }
                }
                let span_start = end_ms.saturating_sub((speech_bytes / BYTES_PER_MS) as u32);
                let mut change = clip.and_then(|clip| {
                    watch_step(
                        &mut watches,
                        key,
                        outcome,
                        recheck,
                        (span_start, end_ms),
                        clip,
                    )
                });
                // `other` -> `caller` without a quiet run: localize on sub-window embeddings (the
                // verifier task, bounded); any failure keeps stage 1's point.
                let o2c = change.is_some_and(|c| c.from == Outcome::Other);
                if o2c {
                    let refine = watches
                        .iter_mut()
                        .find(|(k, _)| *k == key)
                        .and_then(|(_, w)| w.refine.take());
                    if let (Some(refine), Some(reference)) = (refine, reference.as_ref()) {
                        if let Some(at) = stage2_split(
                            &embedder,
                            reference,
                            cfg.threshold,
                            &refine,
                            verify,
                            started,
                            &shared,
                            key,
                        )
                        .await
                        {
                            if let Some(c) = change.as_mut() {
                                c.split_at_ms = at;
                            }
                        }
                    }
                }
                // U1 of an `other` -> `caller` change stays `other`: nothing upgrades or barges on
                // it (the caller's speech is U2, which inherits the verdict).
                let outcome = if o2c { Outcome::Other } else { outcome };
                {
                    let mut book = lock(&shared.book);
                    if let Some(change) = change {
                        push_capped(&mut book.split_keys, key);
                        push_capped(&mut book.changes, change);
                    }
                    // Count the window against the label as it was before this window's upgrade.
                    if cfg.split_on_speaker_change {
                        book_reported_window(&mut book, key, recheck, window);
                    }
                    if recheck {
                        // Upgrade only: `other` -> `caller`, and only when no change was confirmed
                        // for it (with the watch on, a confirmed `other` -> `caller` change keeps
                        // U1 `other`; this upgrade stays for a key with no watch, and with the
                        // watch off). Anything else keeps `other`.
                        // Once per key: later `caller` watch windows add no duplicate entry.
                        let still_other = book
                            .verdicts
                            .iter()
                            .rev()
                            .find(|(k, _)| *k == key)
                            .is_some_and(|(_, o)| *o == Outcome::Other);
                        if outcome == Outcome::Caller && still_other && !o2c {
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

    /// (key, pcm, speech bytes, window) of every check sent so far.
    fn checks_full(rx: &mut UnboundedReceiver<Msg>) -> Vec<(u64, Vec<u8>, usize, u32)> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let Msg::Check {
                key,
                pcm,
                speech_bytes,
                window,
                ..
            } = msg
            {
                out.push((key, pcm, speech_bytes, window));
            }
        }
        out
    }

    /// A short, B short (rejoin), then C rejoins with no verifier running: C's window count and
    /// counted speech come from the SpeechEnd record only. Key 2 additionally has verifier state
    /// as if B's late window-2 `short` verdict had been processed (windows scored 2); C must not
    /// read it, so C is still one window sent, and its next scored window is 2.
    #[test]
    fn second_short_rejoin_window_is_from_feed_record() {
        for (key, verifier_ran) in [(1_u64, false), (2_u64, true)] {
            let gate = enrolled_gate(true);
            let mut rx = lock(&gate.rx).take().unwrap();
            // A: 3 frames (60 ms) < min 100 ms.
            start(&gate, key, 3);
            gate.end_utterance();
            // B: 3 frames (60 ms), short again.
            gate.begin(key, &[2_u8; FRAME]);
            gate_push(&gate, 2, 2);
            gate.end_utterance();
            let sent: Vec<(u64, usize, u32)> = checks_full(&mut rx)
                .into_iter()
                .map(|(k, _, s, w)| (k, s, w))
                .collect();
            assert_eq!(sent, vec![(key, 60 * 32, 1), (key, 0, 2)]);
            if verifier_ran {
                let mut book = lock(&gate.shared.book);
                push_capped(&mut book.windows_scored, (key, 2));
            }
            // C.
            gate.begin(key, &[2_u8; FRAME]);
            assert_eq!(
                gate.live_check_state(),
                Some((key, 1, 140)),
                "C continues from the feed record (verifier ran: {verifier_ran})"
            );
            // Window 2 goes out at 2 × 200 ms of counted speech: 13 more frames (A+B+C = 400).
            gate_push(&gate, 2, 12);
            assert!(checks_full(&mut rx).is_empty(), "no check below the bound");
            gate_push(&gate, 2, 1);
            let sent = checks_full(&mut rx);
            assert_eq!(sent.len(), 1);
            assert_eq!((sent[0].0, sent[0].2, sent[0].3), (key, 200 * 32, 2));
            assert_eq!(gate.live_check_state(), Some((key, 2, 400)));
        }
    }

    fn gate_push(gate: &SpeakerGate, value: u8, n: usize) {
        for _ in 0..n {
            gate.push_frame(&[value; FRAME]);
        }
    }

    /// `recheck_on_other` off (buffer does not roll for ordinary utterances): a re-opened short key
    /// still keeps pre-roll plus the latest check, so window 2 scores the latest speech (Y), not
    /// the first `check_speech_ms` of the rejoin (X).
    #[test]
    fn reopened_short_scores_latest_speech_without_recheck_on_other() {
        let gate = enrolled_gate(false);
        let mut rx = lock(&gate.rx).take().unwrap();
        // A: one frame (20 ms), short.
        start(&gate, 1, 1);
        gate.end_utterance();
        assert_eq!(checks_full(&mut rx).len(), 1);
        // Rejoin: pre-roll (value 1), first 200 ms X (value 4), then Y (value 5).
        gate_push(&gate, 1, 15);
        let mut fed: Vec<u8> = vec![1_u8; 15 * FRAME];
        gate.begin(1, &[4_u8; FRAME]);
        gate_push(&gate, 4, 9);
        fed.extend(std::iter::repeat_n(4_u8, 10 * FRAME));
        // Counted 20 + 200 ms; window 2 at 400 ms: 9 frames of Y.
        gate_push(&gate, 5, 8);
        fed.extend(std::iter::repeat_n(5_u8, 8 * FRAME));
        assert!(checks_full(&mut rx).is_empty());
        gate_push(&gate, 5, 1);
        fed.extend(std::iter::repeat_n(5_u8, FRAME));
        let sent = checks_full(&mut rx);
        assert_eq!(sent.len(), 1);
        let (_, pcm, speech, window) = &sent[0];
        assert_eq!((*speech, *window), (200 * 32, 2));
        let keep = (PRE_ROLL_MS + 200) * BYTES_PER_MS;
        assert_eq!(pcm.len(), keep, "pre-roll plus the latest check");
        assert_eq!(
            pcm.as_slice(),
            &fed[fed.len() - keep..],
            "the clip is the latest audio"
        );
        assert!(pcm.ends_with(&[5_u8; 9 * FRAME]), "ends with all of Y");
        let window_part = &pcm[pcm.len() - 200 * BYTES_PER_MS..];
        assert_eq!(
            window_part.iter().filter(|b| **b == 4).count(),
            FRAME,
            "only the X that fits in the window (1 frame)"
        );
    }

    /// Lockstep with `SPEAKER_GATE_RECHECK_MAX_WINDOWS` in packages/sdk/src/voice/defaults.ts
    /// (asserted in packages/sdk/tests/speaker-gate-event.test.ts).
    #[test]
    fn default_recheck_max_windows_matches_sdk() {
        assert_eq!(DEFAULT_RECHECK_MAX_WINDOWS, 12);
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
        gate.set_pending_barge(1, 0);
        assert!(gate.take_pending_barge(1).is_some());
        assert!(
            gate.hold_for_recheck(1, 0),
            "live utterance: held for the re-check"
        );
        for _ in 0..5 {
            gate.push_frame(&[3_u8; FRAME]);
        }
        gate.end_utterance();
        assert_eq!(checks(&mut rx).len(), 1, "no re-check");
        assert!(
            gate.take_pending_barge(1).is_none(),
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
        assert!(!gate.hold_for_recheck(1, 0));
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
        gate.set_pending_barge(2, 0);
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
        assert!(gate.take_pending_barge(1).is_none(), "nothing held for A");
        assert!(
            gate.take_pending_barge(2).is_some(),
            "B's held barge-in untouched by A's result"
        );
        assert_eq!(gate.outcome_for(2), Some(Outcome::Caller));
        assert!(gate.rechecked(1));
        assert!(!gate.rechecked(2));
    }

    // ---- speaker-change watch (split_on_speaker_change) ----------------------------------------

    /// `ms` of mono s16le at 16 kHz, every sample `amp`.
    fn tone(ms: usize, amp: i16) -> Vec<u8> {
        let mut out = Vec::with_capacity(ms * BYTES_PER_MS);
        for _ in 0..ms * BYTES_PER_MS / 2 {
            out.extend_from_slice(&amp.to_le_bytes());
        }
        out
    }

    /// A clip of `ms` loud audio with one silent 20 ms frame starting `quiet_at` ms into it.
    fn clip_with_quiet_frame(ms: usize, quiet_at: usize) -> Vec<u8> {
        let mut clip = tone(ms, 10_000);
        let from = quiet_at * BYTES_PER_MS;
        clip[from..from + SPLIT_FRAME_MS * BYTES_PER_MS].fill(0);
        clip
    }

    /// First window labels the caller; two opposite re-check windows in a row confirm a change.
    /// The split is the silent frame of the first opposite window's clip (clip: 500 ms ending at
    /// 400 ms, so it starts at -100 ms; the silent frame is at 260 ms).
    #[test]
    fn caller_to_other_after_two_opposite_windows_splits_at_the_low_energy_frame() {
        let mut w = VecDeque::new();
        assert!(watch_step(&mut w, 1, Outcome::Caller, false, (0, 200), tone(500, 9000)).is_none());
        let first = clip_with_quiet_frame(500, 260 + 100);
        assert!(watch_step(&mut w, 1, Outcome::Other, true, (200, 400), first).is_none());
        let change = watch_step(&mut w, 1, Outcome::Other, true, (400, 600), tone(500, 9000))
            .expect("two opposite windows confirm the change");
        assert_eq!(
            change,
            SpeakerChange {
                key: 1,
                from: Outcome::Caller,
                to: Outcome::Other,
                split_at_ms: 260
            }
        );
        // Reported once.
        assert!(watch_step(&mut w, 1, Outcome::Other, true, (600, 800), tone(500, 9000)).is_none());
    }

    #[test]
    fn one_false_opposite_window_does_not_split() {
        let mut w = VecDeque::new();
        assert!(watch_step(&mut w, 1, Outcome::Caller, false, (0, 200), tone(500, 9000)).is_none());
        for i in 1..=8_u32 {
            // other, caller, other, caller ...: never two opposite windows in a row.
            let outcome = if i % 2 == 1 {
                Outcome::Other
            } else {
                Outcome::Caller
            };
            let span = (i * 200, i * 200 + 200);
            assert!(
                watch_step(&mut w, 1, outcome, true, span, tone(500, 9000)).is_none(),
                "window {i}"
            );
        }
        // An unscored window also ends the run.
        assert!(watch_step(&mut w, 2, Outcome::Caller, false, (0, 200), tone(500, 9000)).is_none());
        assert!(watch_step(&mut w, 2, Outcome::Other, true, (200, 400), tone(500, 9000)).is_none());
        assert!(watch_step(
            &mut w,
            2,
            Outcome::Unknown,
            true,
            (400, 600),
            tone(500, 9000)
        )
        .is_none());
        assert!(watch_step(&mut w, 2, Outcome::Other, true, (600, 800), tone(500, 9000)).is_none());
    }

    /// `other` -> `caller` is confirmed by one `caller` window; the key is not watched further.
    #[test]
    fn other_to_caller_confirms_on_one_window() {
        let mut w = VecDeque::new();
        assert!(watch_step(&mut w, 5, Outcome::Other, false, (0, 200), tone(500, 9000)).is_none());
        let change = watch_step(
            &mut w,
            5,
            Outcome::Caller,
            true,
            (200, 400),
            tone(500, 9000),
        )
        .expect("one caller window confirms other -> caller");
        assert_eq!(
            (change.key, change.from, change.to),
            (5, Outcome::Other, Outcome::Caller)
        );
        assert!(watch_step(
            &mut w,
            5,
            Outcome::Caller,
            true,
            (400, 600),
            tone(500, 9000)
        )
        .is_none());
        assert!(watch_step(&mut w, 5, Outcome::Other, true, (600, 800), tone(500, 9000)).is_none());
    }

    /// A single `other` window never splits `caller` -> `other`; two in a row do.
    #[test]
    fn a_single_other_window_never_splits_caller_to_other() {
        let mut w = VecDeque::new();
        assert!(watch_step(&mut w, 6, Outcome::Caller, false, (0, 200), tone(500, 9000)).is_none());
        assert!(watch_step(&mut w, 6, Outcome::Other, true, (200, 400), tone(500, 9000)).is_none());
        assert!(watch_step(
            &mut w,
            6,
            Outcome::Caller,
            true,
            (400, 600),
            tone(500, 9000)
        )
        .is_none());
        assert!(watch_step(&mut w, 6, Outcome::Other, true, (600, 800), tone(500, 9000)).is_none());
        let c = watch_step(
            &mut w,
            6,
            Outcome::Other,
            true,
            (800, 1000),
            tone(500, 9000),
        )
        .unwrap();
        assert_eq!((c.from, c.to), (Outcome::Caller, Outcome::Other));
    }

    /// The `other` -> `caller` split lands inside the backward range (previous window's start to
    /// the first caller window's start + 300 ms) at the end of the longest quiet run. Window 1 is
    /// 0..1500 ms with 700 ms of silence at 800..1500 ms; window 2 is 1500..3000 ms, loud. The
    /// quiet run of at least 500 ms means stage 1 decides and there is no stage 2 audio.
    #[test]
    fn other_to_caller_split_is_the_end_of_the_quiet_run_in_the_backward_range() {
        let mut w = VecDeque::new();
        let mut first = tone(1500, 10_000);
        first[800 * BYTES_PER_MS..].fill(0);
        assert!(watch_step(&mut w, 1, Outcome::Other, false, (0, 1500), first).is_none());
        let change = watch_step(
            &mut w,
            1,
            Outcome::Caller,
            true,
            (1500, 3000),
            tone(1500, 10_000),
        )
        .unwrap();
        // Smoothing over 5 frames (100 ms) trims 2 frames off the end of the run.
        assert_eq!(change.split_at_ms, 1460);
        assert!(
            (0..=1500 + SPLIT_SEARCH_MS).contains(&change.split_at_ms),
            "inside the backward range"
        );
        assert!(w[0].1.refine.is_none(), "a long quiet run needs no stage 2");

        // A short quiet run (200 ms, under the minimum) keeps the audio for stage 2.
        let mut w = VecDeque::new();
        let mut first = tone(1500, 10_000);
        first[1300 * BYTES_PER_MS..].fill(0);
        assert!(watch_step(&mut w, 2, Outcome::Other, false, (0, 1500), first).is_none());
        watch_step(
            &mut w,
            2,
            Outcome::Caller,
            true,
            (1500, 3000),
            tone(1500, 10_000),
        )
        .unwrap();
        let r = w[0].1.refine.as_ref().expect("stage 2 audio");
        assert_eq!((r.lo, r.audio_end), (0, 3000));
    }

    /// Frames before the onset and frames beyond +-300 ms of the window start are not candidates.
    #[test]
    fn split_point_stays_inside_the_search_range_and_after_the_onset() {
        // Clip of 1000 ms ending at 1000 ms (starts at 0). Window starts at 600: range 300..900.
        let mut clip = tone(1000, 10_000);
        for at in [100_usize, 940] {
            clip[at * BYTES_PER_MS..(at + 20) * BYTES_PER_MS].fill(0);
        }
        assert_eq!(
            split_point(600, 1000, &clip),
            600,
            "outside quiet frames ignored"
        );
        // Pre-roll: clip of 500 ms ending at 200 ms starts at -300 ms; the silence at -300 ms is
        // before the onset.
        let clip = clip_with_quiet_frame(500, 0);
        assert_eq!(split_point(100, 200, &clip), 100);
    }

    fn split_cfg(split: bool, cap: u32) -> SpeakerGateConfig {
        SpeakerGateConfig {
            enabled: true,
            model_path: Some("m.onnx".into()),
            enroll_speech_ms: 100,
            check_speech_ms: 200,
            min_check_speech_ms: 100,
            verify_timeout_ms: 5000,
            recheck_max_windows: cap,
            split_on_speaker_change: split,
            ..SpeakerGateConfig::default()
        }
    }

    /// Feed-side window cutting (no verifier): a `caller` utterance is watched only with the flag
    /// on, and the watch stops at `recheck_max_windows`.
    #[test]
    fn watch_cuts_windows_for_caller_up_to_the_cap_only_with_the_flag() {
        for (split, expected) in [(false, 0_usize), (true, 3)] {
            let gate = SpeakerGate::new(split_cfg(split, 4));
            gate.shared.enrolled.store(true, Ordering::SeqCst);
            let mut rx = lock(&gate.rx).take().unwrap();
            start(&gate, 1, 10);
            assert_eq!(checks(&mut rx).len(), 1);
            store(&gate, 1, Outcome::Caller);
            let mut cut = 0;
            for _ in 0..200 {
                gate.push_frame(&[3_u8; FRAME]);
                cut += checks(&mut rx).len();
                lock(&gate.shared.book).rechecks_in_flight.clear();
            }
            assert_eq!(
                cut, expected,
                "split={split}: windows 2..=4, the cap stops the rest"
            );
        }
    }

    /// Feeds `script` (one vector per window, the first is the enrollment) through a running gate
    /// and returns the verdicts and the gate.
    async fn run_script(
        split: bool,
        script: Vec<Vec<f32>>,
        windows: usize,
    ) -> (Arc<SpeakerGate>, Vec<Verdict>) {
        run_script_cap(split, script, windows, 12).await
    }

    /// [`run_script`] with `recheck_max_windows = cap`.
    async fn run_script_cap(
        split: bool,
        script: Vec<Vec<f32>>,
        windows: usize,
        cap: u32,
    ) -> (Arc<SpeakerGate>, Vec<Verdict>) {
        run_script_cfg(split_cfg(split, cap), script, windows, 10).await
    }

    /// Feeds `script` through a running gate built from `cfg`, `frames` 20 ms frames per window.
    async fn run_script_cfg(
        cfg: SpeakerGateConfig,
        script: Vec<Vec<f32>>,
        windows: usize,
        frames: usize,
    ) -> (Arc<SpeakerGate>, Vec<Verdict>) {
        let (gate, verdicts, _queue) = run_script_steps(
            cfg,
            script.into_iter().map(|v| (None, v)).collect(),
            windows,
            frames,
        )
        .await;
        let got = lock(&verdicts).clone();
        (gate, got)
    }

    /// Embedder whose every call takes its scripted time (`Ok(None)` when the script is spent).
    struct DelayEmbedder {
        queue: Mutex<VecDeque<(Duration, Vec<f32>)>>,
        starts: Mutex<Vec<Instant>>,
    }

    #[async_trait::async_trait]
    impl SpeakerEmbedder for DelayEmbedder {
        async fn embed(
            &self,
            _pcm: Bytes,
            _rate: u32,
        ) -> crate::error::SpeechResult<Option<Vec<f32>>> {
            lock(&self.starts).push(Instant::now());
            let next = lock(&self.queue).pop_front();
            let Some((delay, v)) = next else {
                return Ok(None);
            };
            tokio::time::sleep(delay).await;
            Ok(Some(v))
        }
    }

    /// A gate over a [`DelayEmbedder`] (`check_speech_ms` 1500, `verify_timeout_ms` = `verify`),
    /// enrolled and fed window 1 of key 1 (75 frames); returns the gate, the delivered verdicts
    /// with their delivery instants, and the embedder.
    async fn delay_gate(
        verify: u32,
        script: Vec<(u64, Vec<f32>)>,
    ) -> (
        Arc<SpeakerGate>,
        Arc<Mutex<Vec<(Verdict, Instant)>>>,
        Arc<DelayEmbedder>,
    ) {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        cfg.verify_timeout_ms = verify;
        let gate = SpeakerGate::new(cfg);
        let emb = Arc::new(DelayEmbedder {
            queue: Mutex::new(
                script
                    .into_iter()
                    .map(|(ms, v)| (Duration::from_millis(ms), v))
                    .collect(),
            ),
            starts: Mutex::new(Vec::new()),
        });
        struct Fwd(Arc<DelayEmbedder>);
        #[async_trait::async_trait]
        impl SpeakerEmbedder for Fwd {
            async fn embed(
                &self,
                pcm: Bytes,
                rate: u32,
            ) -> crate::error::SpeechResult<Option<Vec<f32>>> {
                self.0.embed(pcm, rate).await
            }
        }
        let sink: Arc<Mutex<Vec<(Verdict, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let s2 = Arc::clone(&sink);
        gate.spawn(
            Box::new(Fwd(Arc::clone(&emb))),
            Box::new(move |v| lock(&s2).push((v, Instant::now()))),
        );
        gate.send(Msg::Enroll {
            pcm: vec![0_u8; 100 * BYTES_PER_MS],
        });
        for _ in 0..500 {
            if gate.is_enrolled() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        for _ in 0..15 {
            gate.push_frame(&[1_u8; FRAME]);
        }
        gate.begin(1, &alternating(0));
        for f in 1..75 {
            gate.push_frame(&alternating(f));
        }
        for _ in 0..500 {
            if lock(&sink).len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        for f in 0..75 {
            gate.push_frame(&alternating(f));
        }
        (gate, sink, emb)
    }

    /// [`run_script_cfg`] with scripted steps; also returns the verdict sink and the embedder's
    /// queue (what is left of the script).
    async fn run_script_steps(
        cfg: SpeakerGateConfig,
        script: VecDeque<Step>,
        windows: usize,
        frames: usize,
    ) -> (
        Arc<SpeakerGate>,
        Arc<Mutex<Vec<Verdict>>>,
        Arc<Mutex<VecDeque<Step>>>,
    ) {
        let gate = SpeakerGate::new(cfg);
        let verdicts: Arc<Mutex<Vec<Verdict>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&verdicts);
        let queue = Arc::new(Mutex::new(script));
        let n = windows;
        gate.spawn(
            Box::new(SharedSeqEmbedder {
                queue: Arc::clone(&queue),
            }),
            Box::new(move |v| lock(&sink).push(v)),
        );
        gate.send(Msg::Enroll {
            pcm: vec![0_u8; 100 * BYTES_PER_MS],
        });
        for _ in 0..500 {
            if gate.is_enrolled() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(gate.is_enrolled());
        for _ in 0..15 {
            gate.push_frame(&[1_u8; FRAME]);
        }
        gate.begin(1, &alternating(0));
        for f in 1..frames {
            gate.push_frame(&alternating(f));
        }
        for i in 1..=n {
            for _ in 0..1000 {
                if lock(&verdicts).len() >= i {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(lock(&verdicts).len() >= i, "verdict {i} never arrived");
            if i < n {
                for f in 0..frames {
                    gate.push_frame(&alternating(f));
                }
            }
        }
        // Extra speech after the last scripted window: nothing more may be cut after a change.
        for f in 0..4 * frames {
            gate.push_frame(&alternating(f));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        (gate, verdicts, queue)
    }

    /// Frame `f` of speech that has no quiet run (loud and soft frames alternate), so stage 1 of
    /// an `other` -> `caller` split finds no run of the minimum length.
    fn alternating(f: usize) -> [u8; FRAME] {
        [if f.is_multiple_of(2) { 3_u8 } else { 200_u8 }; FRAME]
    }

    /// [`SeqEmbedder`] over a queue the test keeps a handle on.
    struct SharedSeqEmbedder {
        queue: Arc<Mutex<VecDeque<Step>>>,
    }

    #[async_trait::async_trait]
    impl SpeakerEmbedder for SharedSeqEmbedder {
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

    fn caller_v() -> Vec<f32> {
        vec![1.0, 0.0]
    }

    fn other_v() -> Vec<f32> {
        vec![0.0, 1.0]
    }

    /// Flag off: an `other` first window upgraded by a `caller` re-check behaves as before (the
    /// verdict becomes `caller`), no change is ever reported, and a `caller` utterance gets no
    /// further window.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn flag_off_keeps_the_upgrade_and_reports_no_change() {
        let (gate, got) = run_script(false, vec![caller_v(), other_v(), caller_v()], 2).await;
        assert_eq!(
            got.iter()
                .map(|v| (v.outcome, v.recheck, v.window))
                .collect::<Vec<_>>(),
            vec![(Outcome::Other, false, 1), (Outcome::Caller, true, 2)]
        );
        assert_eq!(gate.outcome_for(1), Some(Outcome::Caller));
        assert!(gate.take_speaker_changes().is_empty());

        let (gate, got) =
            run_script(false, vec![caller_v(), caller_v(), other_v(), other_v()], 1).await;
        assert_eq!(got.len(), 1, "a caller utterance is not watched: {got:?}");
        assert_eq!(gate.outcome_for(1), Some(Outcome::Caller));
        assert!(gate.take_speaker_changes().is_empty());
    }

    /// Flag on: caller -> other is confirmed by the second opposite window, reported once, the
    /// verdict of the utterance stays `caller`, and the key is not watched further.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn flag_on_reports_caller_to_other_and_stops_watching() {
        let (gate, got) = run_script(
            true,
            vec![
                caller_v(),
                caller_v(),
                caller_v(),
                other_v(),
                other_v(),
                other_v(),
            ],
            4,
        )
        .await;
        assert_eq!(
            got.len(),
            4,
            "windows 1..=4 only, none after the change: {got:?}"
        );
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            (changes[0].key, changes[0].from, changes[0].to),
            (1, Outcome::Caller, Outcome::Other)
        );
        assert!(gate.take_speaker_changes().is_empty());
        assert_eq!(gate.outcome_for(1), Some(Outcome::Caller));
    }

    /// Flag on: other -> caller splits on the first `caller` window. U1 stays `other` (the
    /// delivered verdict of that window is `other` too: nothing upgrades or barges on it), the
    /// change is stored before the verdict, and the key is not watched further. Flag off for the
    /// same script upgrades (see `flag_off_keeps_the_upgrade_and_reports_no_change`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn other_to_caller_splits_on_one_window() {
        let (gate, got) = run_script(
            true,
            vec![caller_v(), other_v(), caller_v(), caller_v(), caller_v()],
            2,
        )
        .await;
        assert_eq!(
            got.iter()
                .map(|v| (v.outcome, v.recheck, v.window))
                .collect::<Vec<_>>(),
            vec![(Outcome::Other, false, 1), (Outcome::Other, true, 2)],
            "no window after the change"
        );
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            (changes[0].key, changes[0].from, changes[0].to),
            (1, Outcome::Other, Outcome::Caller)
        );
        assert_eq!(gate.outcome_for(1), Some(Outcome::Other));
        assert!(gate.rechecked(1));
        assert_eq!(gate.windows_scored(1), 2);
    }

    /// Flag on, [other, caller]: the book holds the first `other` only (no `caller` upgrade); flag
    /// off for the same script holds `other` then one `caller` upgrade.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn caller_window_after_other_upgrades_only_with_the_flag_off() {
        let script = vec![caller_v(), other_v(), caller_v(), caller_v(), caller_v()];
        let entries = |g: &SpeakerGate| -> Vec<(u64, Outcome)> {
            lock(&g.shared.book)
                .verdicts
                .iter()
                .filter(|(k, _)| *k == 1)
                .copied()
                .collect()
        };
        let (on, _) = run_script(true, script.clone(), 2).await;
        let (off, _) = run_script(false, script, 2).await;
        assert_eq!(entries(&on), vec![(1, Outcome::Other)]);
        assert_eq!(
            entries(&off),
            vec![(1, Outcome::Other), (1, Outcome::Caller)]
        );
        assert_eq!(on.windows_scored(1), 2);
        assert_eq!(off.windows_scored(1), 2);
    }

    /// Flag on, [other, caller] then U2 (inheriting `caller`) and [other, other]: one change per
    /// confirmed switch, on two keys. U2's first verdict is the inherited `caller` (window 1) and
    /// costs no embed: the script holds exactly one vector per scored window, so an extra embed
    /// would shift every later outcome.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn other_to_caller_to_other_gives_two_changes_on_two_keys() {
        let (gate, got) = run_script(
            true,
            vec![caller_v(), other_v(), caller_v(), other_v(), other_v()],
            2,
        )
        .await;
        assert_eq!(got.len(), 2, "{got:?}");
        let first = gate.take_speaker_changes();
        assert_eq!(first.len(), 1, "{first:?}");
        assert_eq!(
            (first[0].key, first[0].from, first[0].to),
            (1, Outcome::Other, Outcome::Caller)
        );
        gate.end_utterance();
        let v = gate.inherit_caller(2);
        assert_eq!(
            (v.key, v.outcome, v.recheck, v.window),
            (2, Outcome::Caller, false, 1)
        );
        assert_eq!(gate.outcome_for(2), Some(Outcome::Caller));
        assert_eq!(gate.windows_scored(2), 1);
        start(&gate, 2, 10);
        let mut changes = Vec::new();
        for _ in 0..4 {
            for _ in 0..200 {
                changes.extend(gate.take_speaker_changes());
                if !changes.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            if !changes.is_empty() {
                break;
            }
            for _ in 0..10 {
                gate.push_frame(&[3_u8; FRAME]);
            }
        }
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            (changes[0].key, changes[0].from, changes[0].to),
            (2, Outcome::Caller, Outcome::Other)
        );
        // Windows 2 and 3 of U2 only (the seed was never embedded); U2 stays `caller`.
        assert_eq!(gate.outcome_for(2), Some(Outcome::Caller));
        assert_eq!(gate.windows_scored(2), 1);
        assert_eq!(gate.watch_windows_scored(2), 2);
    }

    /// Flag on: the `other` -> `caller` result of key 1 lands after key 2 went live (the verifier
    /// holds it behind a parked embed); it is booked on key 1 only. Key 2's own verdict and
    /// watch are untouched (`rt.stale-event-by-key`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stale_other_to_caller_result_is_applied_to_its_own_key_only() {
        let release = Arc::new(Notify::new());
        let steps: VecDeque<Step> = VecDeque::from(vec![
            (None, caller_v()),                       // enrollment
            (None, other_v()),                        // key 1 window 1
            (Some(Arc::clone(&release)), caller_v()), // key 1 window 2, parked
            (None, other_v()),                        // key 2 window 1
        ]);
        let gate = SpeakerGate::new(split_cfg(true, 12));
        gate.spawn(
            Box::new(SharedSeqEmbedder {
                queue: Arc::new(Mutex::new(steps)),
            }),
            Box::new(|_| {}),
        );
        gate.send(Msg::Enroll {
            pcm: vec![0_u8; 100 * BYTES_PER_MS],
        });
        for _ in 0..500 {
            if gate.is_enrolled() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        start(&gate, 1, 10);
        for _ in 0..500 {
            if gate.outcome_for(1).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        for _ in 0..10 {
            gate.push_frame(&[3_u8; FRAME]);
        }
        gate.end_utterance();
        start(&gate, 2, 10);
        gate.end_utterance();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            gate.outcome_for(2),
            None,
            "key 2 waits behind the parked embed"
        );
        release.notify_waiters();
        for _ in 0..500 {
            if gate.outcome_for(2).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].key, 1);
        assert_eq!(gate.outcome_for(1), Some(Outcome::Other));
        assert_eq!(gate.outcome_for(2), Some(Outcome::Other));
        assert_eq!(gate.windows_scored(2), 1);
        assert!(!gate.rechecked(2));
    }

    /// Stage 2 (no quiet run): sub-windows of 1000 ms on a 125 ms hop are embedded on the verifier
    /// task from the previous window's start; the first `caller` one decides the split, which
    /// stays inside +-150 ms of its start. The script scores sub-windows other, other, caller:
    /// the third starts at 250 ms, so the split lies in 100..=400 ms, and the two vectors after it
    /// stay unused.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stage_two_localizes_on_the_first_caller_sub_window() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let steps: VecDeque<Step> = [
            caller_v(), // enrollment
            other_v(),  // window 1
            caller_v(), // window 2: confirms
            other_v(),  // sub-window at 0 ms
            other_v(),  // 125 ms
            caller_v(), // 250 ms
            caller_v(), // unused
            caller_v(), // unused
        ]
        .into_iter()
        .map(|v| (None, v))
        .collect();
        let (gate, _verdicts, queue) = run_script_steps(cfg, steps, 2, 75).await;
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            (changes[0].from, changes[0].to),
            (Outcome::Other, Outcome::Caller)
        );
        assert!(
            (100..=400).contains(&changes[0].split_at_ms),
            "split {}",
            changes[0].split_at_ms
        );
        assert_eq!(lock(&queue).len(), 2, "exactly three sub-windows embedded");
    }

    /// A stage 2 embed that exceeds `verify_timeout_ms` falls back to stage 1: the change is still
    /// reported (with stage 1's point), the wait is bounded and the warning names `waitedMs`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stage_two_timeout_falls_back_to_stage_one() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        cfg.verify_timeout_ms = 300;
        let never = Arc::new(Notify::new());
        let steps: VecDeque<Step> = VecDeque::from(vec![
            (None, caller_v()),        // enrollment
            (None, other_v()),         // window 1
            (None, caller_v()),        // window 2: confirms
            (Some(never), caller_v()), // first sub-window never returns
        ]);
        let t0 = Instant::now();
        let (gate, verdicts, _queue) = run_script_steps(cfg, steps, 2, 75).await;
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            (changes[0].from, changes[0].to),
            (Outcome::Other, Outcome::Caller)
        );
        // Stage 1's point: inside the backward range.
        assert!(changes[0].split_at_ms <= 1500 + SPLIT_SEARCH_MS);
        assert_eq!(lock(&verdicts).len(), 2);
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "bounded by verify_timeout_ms"
        );
        let warns = gate.warnings();
        assert!(
            warns
                .iter()
                .any(|w| w.contains("stage 2 total bound") && w.contains("waitedMs=")),
            "{warns:?}"
        );
    }

    /// Stage 2 stays inside the window's own verify budget (`verify` 300 ms): the window embed
    /// returns at once and each sub-window embed takes 160 ms. The first sub-window embed returns
    /// with under 140 ms left, the second is bounded by what is left and always times out (its
    /// 160 ms sleep is longer than any remaining bound), so the stage stops with "stage 2 total
    /// bound" after at most 2 sub-window embeds and books stage 1's point. A runtime stall only
    /// shrinks the remaining budget, so it can lower the count, never raise it, and it cannot push
    /// the window's own (instant) embed past `verify`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stage_two_stays_inside_the_window_verify_budget() {
        let mut script = vec![(0, caller_v()), (0, other_v()), (0, caller_v())];
        script.extend((0..20).map(|_| (160, other_v())));
        let (gate, sink, emb) = delay_gate(300, script).await;
        for _ in 0..400 {
            if lock(&sink).len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let (v2, _) = lock(&sink)[1];
        assert_eq!((v2.window, v2.recheck), (2, true));
        // 3 = enrollment, window 1, window 2; with 160 ms embeds in a 300 ms budget at most 2
        // sub-window embeds start. Load can only lower the count, never raise it.
        let stage2_embeds = lock(&emb.starts).len() - 3;
        assert!(stage2_embeds <= 2, "{stage2_embeds} stage-2 embeds");
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert!(changes[0].split_at_ms <= 1500 + SPLIT_SEARCH_MS);
        assert!(
            gate.warnings()
                .iter()
                .any(|w| w.contains("stage 2 total bound")
                    && w.contains("waitedMs=")
                    && !w.contains("embed timeout")),
            "{:?}",
            gate.warnings()
        );
    }

    /// With the window's verify budget already spent, stage 2 embeds nothing and reports
    /// "stage 2 total bound" with `waitedMs`; the caller keeps stage 1's point.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stage_two_with_the_budget_spent_embeds_nothing() {
        struct Counting(Mutex<u32>);
        #[async_trait::async_trait]
        impl SpeakerEmbedder for Counting {
            async fn embed(
                &self,
                _pcm: Bytes,
                _rate: u32,
            ) -> crate::error::SpeechResult<Option<Vec<f32>>> {
                *lock(&self.0) += 1;
                Ok(Some(caller_v()))
            }
        }
        // Window 1 loud with a 200 ms quiet tail: a run under the minimum, so stage 1 keeps the
        // audio for stage 2.
        let mut first = tone(1500, 10_000);
        first[1300 * BYTES_PER_MS..].fill(0);
        let clip = tone(1500, 10_000);
        let (_point, refine) = other_to_caller_split(Some((0, 1500, first)), (1500, 3000), &clip);
        let refine = refine.expect("stage 2 audio");
        let counting = Arc::new(Counting(Mutex::new(0)));
        let embedder: Arc<dyn SpeakerEmbedder> = counting.clone();
        let gate = SpeakerGate::new(split_cfg(true, 12));
        let got = stage2_split(
            &embedder,
            &caller_v(),
            0.5,
            &refine,
            Duration::from_millis(300),
            Instant::now() - Duration::from_millis(300),
            &gate.shared,
            1,
        )
        .await;
        assert_eq!(got, None);
        assert_eq!(*lock(&counting.0), 0, "no embed with the budget spent");
        let warns = gate.warnings();
        assert!(
            warns
                .iter()
                .any(|w| w.contains("stage 2 total bound") && w.contains("waitedMs=")),
            "{warns:?}"
        );
    }

    /// A U1 final awaiting its verdict during a slow stage 2 (50 ms sub-windows, the third is
    /// `caller`) gets the booked `other` -> `caller` change, never `other` before the change
    /// exists: the check stays in flight until the change is stored.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_final_awaiting_during_stage_two_sees_the_booked_change() {
        let script = vec![
            (0, caller_v()),
            (0, other_v()),
            (100, caller_v()),
            (50, other_v()),
            (50, other_v()),
            (50, caller_v()),
        ];
        let (gate, sink, _emb) = delay_gate(1000, script).await;
        let t0 = Instant::now();
        // Window 2 is in flight (its embed takes 100 ms, stage 2 another ~150 ms).
        gate.end_utterance();
        let outcome = gate.await_outcome(1).await;
        assert_eq!(outcome, Outcome::Other);
        assert!(
            !gate.recheck_in_flight(1),
            "the await returned with the window still in flight"
        );
        let changes = gate.take_speaker_changes();
        assert_eq!(
            changes.len(),
            1,
            "change booked before the final reads Other"
        );
        assert_eq!(
            (changes[0].from, changes[0].to),
            (Outcome::Other, Outcome::Caller)
        );
        assert!(
            t0.elapsed() < Duration::from_millis(1000),
            "within verify_timeout_ms"
        );
        assert_eq!(lock(&sink).len(), 2);
    }

    /// With 1500 ms windows stage 2's point can land up to start(k) + 650 ms (the bench "start"
    /// variant: the last sub-window starts at start(k) + 500, refined +-150): the first caller
    /// sub-window is the 17th, at 2000 ms, with window 2 starting at 1500 ms.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stage_two_split_can_land_up_to_650_ms_after_the_caller_window_start() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let mut steps: Vec<Step> = vec![(None, caller_v()), (None, other_v()), (None, caller_v())];
        steps.extend((0..16).map(|_| (None, other_v())));
        steps.push((None, caller_v()));
        let (gate, _v, queue) = run_script_steps(cfg, steps.into(), 2, 75).await;
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        let at = changes[0].split_at_ms;
        assert!(
            (1850..=1500 + 650).contains(&at),
            "split {at}: beyond start(k) + 300 (old range), within start(k) + 650"
        );
        assert_eq!(lock(&queue).len(), 0, "all 17 sub-windows embedded");
    }

    /// (a) Flag on: a caller-first utterance watched for 3 more windows without a change reports
    /// what the watch off reports (one window, not re-checked); the 3 watch windows are counted
    /// apart.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn flag_on_caller_watch_windows_do_not_count_as_rechecks() {
        let (gate, got) = run_script_cap(true, vec![caller_v(); 5], 4, 4).await;
        assert_eq!(got.len(), 4, "{got:?}");
        assert!(gate.take_speaker_changes().is_empty());
        assert!(!gate.rechecked(1));
        assert_eq!(gate.windows_scored(1), 1);
        assert_eq!(gate.watch_windows_scored(1), 3);
        let v = gate.verdict_read_by_final(1, Outcome::Caller);
        assert_eq!((v.recheck, v.window), (false, 1));
    }

    /// (b) Flag on: an `other`-first utterance with a `caller` window reports the same `rechecked`
    /// / windows as the watch off for the same script. The watch off upgrades; the watch on
    /// reports one `other` -> `caller` change at that window and cuts no further window.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn flag_on_other_first_recheck_reports_like_flag_off() {
        for (script, n) in [
            (vec![caller_v(), other_v(), other_v(), caller_v()], 3),
            (vec![caller_v(), other_v(), caller_v(), caller_v()], 2),
        ] {
            let (off, got_off) = run_script_cap(false, script.clone(), n, 3).await;
            let (on, got_on) = run_script_cap(true, script, n, 3).await;
            assert_eq!((got_off.len(), got_on.len()), (n, n));
            assert!(off.rechecked(1));
            let changes = on.take_speaker_changes();
            assert_eq!(changes.len(), 1, "{changes:?}");
            assert_eq!(
                (changes[0].from, changes[0].to),
                (Outcome::Other, Outcome::Caller)
            );
            assert_eq!(
                (on.rechecked(1), on.windows_scored(1)),
                (off.rechecked(1), off.windows_scored(1))
            );
            assert_eq!(on.watch_windows_scored(1), 0);
            assert_eq!(off.watch_windows_scored(1), 0);
            // The watch off upgrades the verdict; the watch on keeps U1 `other`.
            assert_eq!(off.outcome_for(1), Some(Outcome::Caller));
            assert_eq!(on.outcome_for(1), Some(Outcome::Other));
        }
    }

    /// (c) Flag on: a caller -> other split leaves U1 reporting one window, not re-checked; the
    /// other key (U2) starts fresh.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn flag_on_caller_to_other_split_u1_reports_one_window() {
        let (gate, got) =
            run_script(true, vec![caller_v(), caller_v(), other_v(), other_v()], 3).await;
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(gate.take_speaker_changes().len(), 1);
        assert!(!gate.rechecked(1));
        assert_eq!(gate.windows_scored(1), 1);
        assert_eq!(gate.watch_windows_scored(1), 2);
        start(&gate, 2, 10);
        assert!(!gate.rechecked(2));
        assert_eq!(gate.windows_scored(2), 0);
        assert_eq!(gate.watch_windows_scored(2), 0);
    }

    /// The media clock keeps counting through a brief VAD gap (gap frames included), so the
    /// re-check window cut at 600 ms of media reports `end_ms == 600`, not 400 (counted speech).
    #[test]
    fn media_clock_continues_across_a_brief_gap() {
        let gate = SpeakerGate::new(split_cfg(true, 12));
        gate.shared.enrolled.store(true, Ordering::SeqCst);
        let mut rx = lock(&gate.rx).take().unwrap();
        start(&gate, 1, 15);
        let first: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(matches!(first[0], Msg::Check { end_ms: 200, .. }));
        store(&gate, 1, Outcome::Other);
        gate.end_utterance();
        for _ in 0..5 {
            gate.push_frame(&[1_u8; FRAME]);
        }
        gate.begin(1, &[2_u8; FRAME]);
        let mut end_ms = None;
        for _ in 0..30 {
            if let Ok(Msg::Check { end_ms: e, .. }) = rx.try_recv() {
                end_ms = Some(e);
                break;
            }
            gate.push_frame(&[2_u8; FRAME]);
        }
        assert_eq!(end_ms, Some(600));
    }

    /// With the watch on and `recheck_on_other` off, `end_locked` leaves a held barge-in of an
    /// `other` key exactly as with the watch off: the verdict handler owns it.
    #[test]
    fn pending_barge_after_end_matches_flag_off_without_recheck() {
        let mut kept = Vec::new();
        for split in [false, true] {
            let mut cfg = split_cfg(split, 12);
            cfg.recheck_on_other = false;
            let gate = SpeakerGate::new(cfg);
            gate.shared.enrolled.store(true, Ordering::SeqCst);
            let _rx = lock(&gate.rx).take().unwrap();
            start(&gate, 1, 10);
            store(&gate, 1, Outcome::Other);
            gate.set_pending_barge(1, 0);
            gate.end_utterance();
            kept.push(gate.has_pending_barge(1));
        }
        assert_eq!(kept[0], kept[1]);
    }

    #[test]
    fn split_flag_defaults_off_and_one_window_cap_is_rejected() {
        assert!(!SpeakerGateConfig::default().split_on_speaker_change);
        let mut cfg = split_cfg(true, 1);
        cfg.recheck_on_other = false;
        assert!(cfg.validate().unwrap_err().contains("splitOnSpeakerChange"));
        cfg.recheck_max_windows = 0;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]).unwrap() - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).unwrap().abs() < 1e-6);
        assert!(cosine(&[0.0, 0.0], &[1.0, 0.0]).is_none());
        assert!(cosine(&[1.0], &[1.0, 2.0]).is_none());
    }

    // ---- adaptive reference (adapt_reference, decision e) --------------------------------------

    fn adapt_cfg(adapt: bool, cap: u32) -> SpeakerGateConfig {
        SpeakerGateConfig {
            adapt_reference: adapt,
            ..split_cfg(true, cap)
        }
    }

    async fn wait_for(gate: &SpeakerGate, what: &str, f: impl Fn(&SpeakerGate) -> bool) {
        for _ in 0..2000 {
            if f(gate) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("timed out waiting for {what}: {:?}", gate.adapt_stats());
    }

    /// Scores 0.8 against `caller_v`: `caller`, and above threshold + margin (0.559).
    fn near_caller_v() -> Vec<f32> {
        vec![0.8, 0.6]
    }

    /// Scores 0.5 against `caller_v`: `caller` (>= 0.459) but below threshold + margin.
    fn below_margin_v() -> Vec<f32> {
        vec![0.5, 0.866_025_4]
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn adapt_commits_a_clean_caller_utterance_at_settle() {
        let (gate, got) = run_script_cfg(
            adapt_cfg(true, 2),
            vec![caller_v(), near_caller_v(), near_caller_v()],
            2,
            10,
        )
        .await;
        assert!(got.iter().all(|v| v.outcome == Outcome::Caller), "{got:?}");
        assert_eq!(gate.adapt_stats().commits, 0, "nothing before the final");
        assert_eq!(gate.adapt_stats().cos_to_anchor, Some(1.0));
        gate.settle(1);
        wait_for(&gate, "commit", |g| g.adapt_stats().commits == 1).await;
        let cos = gate.adapt_stats().cos_to_anchor.unwrap();
        assert!(cos < 1.0 && cos > 0.99, "one EMA step of weight 0.1: {cos}");
        // A second settle of the same key finds nothing to commit.
        gate.settle(1);
        wait_for(&gate, "second settle", |g| {
            g.shared.settles.load(Ordering::SeqCst) == 2
        })
        .await;
        assert_eq!(gate.adapt_stats().commits, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_candidate_below_the_margin_is_never_committed() {
        let (gate, got) = run_script_cfg(
            adapt_cfg(true, 2),
            vec![caller_v(), below_margin_v(), below_margin_v()],
            2,
            10,
        )
        .await;
        assert!(got.iter().all(|v| v.outcome == Outcome::Caller), "{got:?}");
        gate.settle(1);
        wait_for(&gate, "skip", |g| g.adapt_stats().skipped == 1).await;
        let st = gate.adapt_stats();
        assert_eq!((st.commits, st.rejected_anchor), (0, 0));
        assert_eq!(st.cos_to_anchor, Some(1.0), "reference unchanged");
    }

    /// Confident caller windows, then a confirmed caller -> other change: the key is split and
    /// commits nothing, although its first windows were candidates.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_split_utterance_commits_nothing() {
        let (gate, _got) = run_script_cfg(
            adapt_cfg(true, 4),
            vec![
                caller_v(),
                near_caller_v(),
                near_caller_v(),
                other_v(),
                other_v(),
            ],
            4,
            10,
        )
        .await;
        assert_eq!(gate.take_speaker_changes().len(), 1);
        gate.settle(1);
        wait_for(&gate, "skip", |g| g.adapt_stats().skipped == 1).await;
        let st = gate.adapt_stats();
        assert_eq!((st.commits, st.cos_to_anchor), (0, Some(1.0)));
        assert!(
            gate.adapt_log().iter().any(|l| l.contains("split=true")),
            "{:?}",
            gate.adapt_log()
        );
    }

    /// An other -> caller split (U1) commits nothing either: its first window is `other`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_other_to_caller_split_commits_nothing() {
        let (gate, _got) = run_script_cfg(
            adapt_cfg(true, 4),
            vec![caller_v(), other_v(), near_caller_v()],
            2,
            10,
        )
        .await;
        assert_eq!(gate.take_speaker_changes().len(), 1);
        gate.settle(1);
        wait_for(&gate, "skip", |g| g.adapt_stats().skipped == 1).await;
        assert_eq!(gate.adapt_stats().commits, 0);
    }

    #[test]
    fn an_update_below_the_anchor_bound_is_rejected() {
        let anchor = vec![1.0, 0.0];
        // Already drifted to cosine 0.81 against the anchor; a candidate further away.
        let reference = vec![0.81, (1.0_f32 - 0.81 * 0.81).sqrt()];
        match adapt_step(&reference, &anchor, &[vec![0.0, 1.0]]) {
            AdaptStep::RejectAnchor { cos_to_anchor } => {
                assert!(cos_to_anchor < ANCHOR_MIN_COSINE, "{cos_to_anchor}")
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
        // Toward the anchor: committed, unit length, closer than before.
        match adapt_step(&reference, &anchor, &[vec![1.0, 0.0]]) {
            AdaptStep::Commit {
                reference: new,
                cos_to_anchor,
            } => {
                assert!(cos_to_anchor > 0.81);
                let n: f32 = new.iter().map(|x| x * x).sum::<f32>().sqrt();
                assert!((n - 1.0).abs() < 1e-5, "re-normalized: {n}");
            }
            other => panic!("expected a commit, got {other:?}"),
        }
        // EMA weight: one step from the anchor toward an orthogonal candidate moves at most
        // ADAPT_ALPHA of the way.
        let AdaptStep::Commit { cos_to_anchor, .. } =
            adapt_step(&anchor, &anchor, &[vec![0.0, 1.0]])
        else {
            panic!("expected a commit");
        };
        let expected =
            (1.0 - ADAPT_ALPHA) / ((1.0 - ADAPT_ALPHA).powi(2) + ADAPT_ALPHA.powi(2)).sqrt();
        assert!((cos_to_anchor - expected).abs() < 1e-5);
        assert_eq!(adapt_step(&anchor, &anchor, &[]), AdaptStep::Skip);
        assert_eq!(adapt_step(&anchor, &anchor, &[vec![1.0]]), AdaptStep::Skip);
        const { assert!(ADAPT_ALPHA <= 0.1) };
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reset_clears_the_reference_and_the_anchor() {
        let (gate, verdicts, queue) = run_script_steps(
            adapt_cfg(true, 2),
            [caller_v(), near_caller_v(), near_caller_v()]
                .into_iter()
                .map(|v| (None, v))
                .collect(),
            2,
            10,
        )
        .await;
        gate.settle(1);
        wait_for(&gate, "commit", |g| g.adapt_stats().commits == 1).await;
        gate.reset();
        wait_for(&gate, "reset", |g| g.adapt_stats().cos_to_anchor.is_none()).await;
        assert!(!gate.is_enrolled());
        // Re-enroll on another voice: the new anchor and reference are that voice only.
        lock(&queue).clear();
        lock(&queue).push_back((None, other_v()));
        gate.send(Msg::Enroll {
            pcm: vec![0_u8; 100 * BYTES_PER_MS],
        });
        wait_for(&gate, "re-enroll", |g| g.is_enrolled()).await;
        assert_eq!(gate.adapt_stats().cos_to_anchor, Some(1.0));
        lock(&queue).push_back((None, other_v()));
        let before = lock(&verdicts).len();
        gate.begin(2, &alternating(0));
        for f in 1..10 {
            gate.push_frame(&alternating(f));
        }
        for _ in 0..2000 {
            if lock(&verdicts).iter().any(|v| v.key == 2) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let v = lock(&verdicts)[before..]
            .iter()
            .find(|v| v.key == 2)
            .cloned()
            .expect("verdict for key 2");
        assert_eq!(
            v.outcome,
            Outcome::Caller,
            "scored against the new reference only"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn option_off_leaves_the_reference_unchanged() {
        for cfg in [adapt_cfg(false, 2), {
            // adapt on, split off: no effect either.
            let mut c = adapt_cfg(true, 2);
            c.split_on_speaker_change = false;
            c
        }] {
            let (gate, _got) = run_script_cfg(
                cfg,
                vec![caller_v(), near_caller_v(), near_caller_v()],
                1,
                10,
            )
            .await;
            assert!(!gate.adapts_reference());
            gate.settle(1); // no-op: nothing is sent
            gate.send(Msg::Settle { key: 1 });
            wait_for(&gate, "settle", |g| {
                g.shared.settles.load(Ordering::SeqCst) >= 1
            })
            .await;
            assert_eq!(gate.shared.settles.load(Ordering::SeqCst), 1);
            assert_eq!(gate.adapt_stats(), AdaptStats::default());
            assert!(gate.adapt_log().is_empty());
        }
    }

    /// No embedding value reaches a log, warning or verdict string: the adapt log lines carry
    /// only `key=value` pairs of ids, counts, booleans and a 3-decimal cosine.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_embedding_values_in_adapt_logs_or_verdicts() {
        let reference = vec![0.612_345_7, 0.398_765_4, 0.276_543_2];
        let cand = vec![0.623_456_1, 0.387_654_3, 0.265_432_1];
        let far = vec![0.912_345_6, -0.301_234_5, -0.310_987_6];
        let (gate, got) = run_script_cfg(
            adapt_cfg(true, 4),
            vec![
                reference.clone(),
                cand.clone(),
                cand.clone(),
                far.clone(),
                far.clone(),
            ],
            4,
            10,
        )
        .await;
        // A second utterance that commits.
        let (gate2, got2) = run_script_cfg(
            adapt_cfg(true, 2),
            vec![reference.clone(), cand.clone(), cand.clone()],
            2,
            10,
        )
        .await;
        gate.settle(1);
        gate2.settle(1);
        wait_for(&gate, "skip", |g| g.adapt_stats().skipped == 1).await;
        wait_for(&gate2, "commit", |g| g.adapt_stats().commits == 1).await;
        let mut lines: Vec<String> = Vec::new();
        for g in [&gate, &gate2] {
            lines.extend(g.adapt_log());
            lines.extend(g.warnings());
            lines.push(format!("{:?}", g.adapt_stats()));
        }
        lines.extend(got.iter().chain(&got2).map(|v| format!("{v:?}")));
        lines.extend(gate.take_speaker_changes().iter().map(|c| format!("{c:?}")));
        assert!(gate2.adapt_log().iter().any(|l| l.contains("adapted")));
        let shape = |l: &str| {
            l.split_whitespace().all(|t| match t.split_once('=') {
                None => t.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                Some((_, v)) => {
                    v == "true"
                        || v == "false"
                        || v.chars().all(|c| c.is_ascii_digit())
                        || (v.len() == 5 && v.starts_with("0.") || v == "1.000")
                }
            })
        };
        for l in gate.adapt_log().iter().chain(&gate2.adapt_log()) {
            assert!(shape(l), "adapt log line with an unexpected token: {l}");
        }
        for v in reference.iter().chain(&cand).chain(&far) {
            for digits in [format!("{v}"), format!("{v:.3}"), format!("{v:.4}")] {
                for l in &lines {
                    assert!(!l.contains(&digits), "embedding value {digits} in: {l}");
                }
            }
        }
    }

    /// Source guard: no format string in the gate interpolates an embedding, the reference, the
    /// anchor or a candidate (by name or with a Debug `{:?}` placeholder on a vector).
    #[test]
    fn gate_source_never_formats_embeddings() {
        let src = include_str!("speaker_gate.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        for name in [
            "reference",
            "anchor",
            "candidate",
            "candidates",
            "v",
            "new",
            "r",
            "a",
            "c",
            "mixed",
            "sum",
            "pcm",
            "clip",
        ] {
            for pat in [
                format!("{{{name}}}"),
                format!("{{{name}:"),
                format!("{{{name}:?}}"),
            ] {
                assert!(
                    !prod.contains(&pat),
                    "format placeholder {pat} in speaker_gate.rs (embeddings and PCM are never logged)"
                );
            }
        }
    }
}
