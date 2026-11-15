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
//! Other -> caller split ([`SpeakerGateConfig::split_on_speaker_change`]): a `caller` window after
//! `other` windows splits the utterance only when one of those `other` windows scored under
//! [`CONFIDENT_OTHER_MAX_SCORE`] (confident evidence of another voice). Without one, the `other`
//! label may be the caller's own voice scored just under the threshold, so the change is not
//! split: the `caller` window upgrades the whole utterance as a re-check does (no caller words are
//! cut), and a near-miss confirm becomes a `caller` verdict.
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

/// `other` -> `caller` sub-window scan: sub-window length embedded to find the caller's onset,
/// scanned from the previous scored window's start. The scan's constants come from
/// `crates/vendor-sherpa-onnx/examples/NEAR_MISS_BENCH.md` (decisions w and x, config A): it
/// loses the fewest caller words on held-out sets, at the cost of more leaked bystander words.
pub const SPLIT_STAGE2_SUB_MS: u32 = 750;
/// Sub-window scan: hop between sub-windows.
pub const SPLIT_STAGE2_HOP_MS: u32 = 125;
/// The `other` -> `caller` cut is the start of the first `caller` sub-window minus this many ms,
/// never before the previous scored window's start. A sub-window scores `caller` only once most
/// of it is caller audio, so its start lies after the caller's onset as often as before it.
pub const SPLIT_BACKOFF_MS: u32 = 300;
/// Near-miss re-check: a window of an `other` utterance (watch on) scoring in
/// `[NEAR_MISS_MIN_SCORE, threshold)` runs the sub-window scan; one `caller` sub-window confirms
/// it as the `caller` window (`NEAR_MISS_BENCH.md`, band lower edge 0.30, need 1).
pub const NEAR_MISS_MIN_SCORE: f32 = 0.30;
/// Confident `other` evidence (decision bb): an `other` -> `caller` change splits only when U1
/// holds an `other` window scoring below this. Without one, a near-threshold `other` verdict is
/// not evidence of another speaker, and the change stays the re-check upgrade (the whole utterance
/// becomes `caller`; decisions h, w and aa). Value from
/// `crates/vendor-sherpa-onnx/examples/NEAR_MISS_BENCH.md` (6j): the fewest caller words lost
/// above the floor over caller-only utterances and the held-out bystander-to-caller scenes.
pub const CONFIDENT_OTHER_MAX_SCORE: f32 = 0.20;
/// Frames of power smoothing in the quiet-run search (100 ms).
const SPLIT_SMOOTH_FRAMES: usize = 5;

/// The split point is searched within this many ms either side of the first opposite window's
/// start.
pub const SPLIT_SEARCH_MS: u32 = 300;
/// `caller` -> `other`: an open quiet run (quiet to the range end, the middle of the first `other`
/// window) cuts no later than this many ms after the first `other` window's start (decision s,
/// chunk 6e), unless the caller is still speaking then: the cut never lands before the quiet
/// run's start, so it never clips the caller. It bounds how much of a steady quieter next speaker
/// with no pause stays in the caller part. The smallest cap with 0 caller words cut on tails and takeovers:
/// `crates/vendor-sherpa-onnx/examples/CALLER_TO_OTHER_BENCH.md` (cut point).
pub const CALLER_TO_OTHER_OPEN_CUT_MAX_MS: u32 = 450;

/// Frame length (ms) of the split-point energy search.
const SPLIT_FRAME_MS: usize = 20;
/// The `other` -> `caller` sub-window scan ends at least this long before a final awaiting the
/// same window gives up ([`SpeakerGate::await_outcome`], `verify_timeout_ms`), so the change is
/// booked before the final reads the verdict. The scan's bound counts from the window's enqueue,
/// which is never later than the final's wait start (a window is sent during speech).
const SCAN_AWAIT_MARGIN_MS: u64 = 50;

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
/// audio (and, for the sub-window embeddings of an `other` -> `caller` change, on the
/// audio only: they score fixed spans), never on when an embedding finished. Both directions are
/// reported: `from` and `to` are `Caller` or `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeakerChange {
    pub key: u64,
    pub from: Outcome,
    pub to: Outcome,
    /// `caller` -> `other`: the end of the longest quiet run from the previous `caller` window's
    /// start to the middle of the first `other` window (see `caller_to_other_split`). `other` ->
    /// `caller`: the start of the first `caller` sub-window ([`SPLIT_STAGE2_SUB_MS`] on a
    /// [`SPLIT_STAGE2_HOP_MS`] hop from the previous scored window's start) minus
    /// [`SPLIT_BACKOFF_MS`], never before the previous window's start; that start itself without
    /// a `caller` sub-window or when the scan runs out of budget.
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
        /// When the check was sent: the sub-window scan's total bound counts from here
        /// ([`SCAN_AWAIT_MARGIN_MS`]).
        enqueued: Instant,
    },
    /// Decision hh: SpeechEnd resolved `key`'s deferred split as the skip upgrade
    /// ([`SpeakerGate::resolve_deferred_split_at_end`]): deliver the confirming window's verdict
    /// again (`other` re-check, read as `caller`) so the held barge-in is settled.
    DeferredEnd {
        key: u64,
        /// The deferred window.
        window: u32,
    },
    /// An utterance that starts as the `caller` after an `other` -> `caller` split
    /// ([`SpeakerGate::inherit_caller`]): arms its watch with label `caller`.
    Inherit {
        key: u64,
        /// U2's caller floor on its own clock (decision y): its `caller` -> `other` cut never
        /// lands before it.
        floor: Option<u32>,
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
    /// Keys whose `other` -> `caller` split the agent skipped and that were upgraded to `caller`
    /// instead ([`SpeakerGate::upgrade_after_skipped_split`]).
    skip_upgraded: VecDeque<u64>,
    /// Per U1 key of a confirmed `other` -> `caller` change: the caller floor of its U2, on U2's
    /// clock (decision y; [`SpeakerGate::caller_floor_for`]).
    caller_floors: VecDeque<(u64, u32)>,
    /// Decision hh: keys whose `other` -> `caller` split is deferred (a `caller` window's split
    /// scan completed with no `caller` sub-window), with the remembered window's fallback cut and
    /// that window's number.
    veto_pending: VecDeque<(u64, u32, u32)>,
    /// Keys whose SpeechEnd was seen (split on): a split deferred after it is resolved at once.
    speech_ended: VecDeque<u64>,
}

/// Decision hh at the end of speech: the deferred split becomes the `not_speaking` skip's upgrade.
/// The same book state as a stored `other` -> `caller` change (a split key) that the agent then
/// skips with `not_speaking` ([`SpeakerGate::upgrade_after_skipped_split`]).
fn book_deferred_end_upgrade(book: &mut Book, key: u64, cut: u32) {
    if !book.split_keys.contains(&key) {
        push_capped(&mut book.split_keys, key);
    }
    book_skip_upgrade(book, key);
    eprintln!(
        "[speaker-gate] info: speaker split skipped key={key} reason=not_speaking split_at_ms={cut} fallback=upgrade (deferred split at SpeechEnd)"
    );
}

/// The skip -> upgrade transition: `caller` booked for `key` if it still reads `other`, and the
/// key marked skip-upgraded (its `other` re-check verdicts then count as `caller`).
fn book_skip_upgrade(book: &mut Book, key: u64) {
    let still_other = book
        .verdicts
        .iter()
        .rev()
        .find(|(k, _)| *k == key)
        .is_some_and(|(_, o)| *o == Outcome::Other);
    if still_other {
        push_capped(&mut book.verdicts, (key, Outcome::Caller));
    }
    if !book.skip_upgraded.contains(&key) {
        push_capped(&mut book.skip_upgraded, key);
    }
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
    /// Test hook: with `parked`, hold only deliveries whose window confirmed a speaker change.
    park_changes_only: AtomicBool,
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
                park_changes_only: AtomicBool::new(false),
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
        self.clear_deferred_splits();
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
        self.clear_deferred_splits();
        self.shared.notify.notify_waiters();
    }

    /// Decision hh: drop deferred splits and SpeechEnd records (reset, disable): an old key is
    /// never upgraded at its end after them.
    fn clear_deferred_splits(&self) {
        let mut book = lock(&self.shared.book);
        book.veto_pending.clear();
        book.speech_ended.clear();
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
        self.shared.park_changes_only.store(false, Ordering::SeqCst);
        self.shared.parked.store(on, Ordering::SeqCst);
        if !on {
            self.shared.park_release.notify_waiters();
        }
    }

    /// Test hook: like [`Self::park_verdict_delivery`], but only the delivery of a window that
    /// confirmed a speaker change is held (stored first, so the change is pending while held).
    #[doc(hidden)]
    pub fn park_change_delivery(&self, on: bool) {
        self.shared.park_changes_only.store(on, Ordering::SeqCst);
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
        if self.cfg.split_on_speaker_change {
            // A resumed key is live again: a deferral that completes now waits for its next
            // window or SpeechEnd, it is not resolved at once.
            lock(&self.shared.book)
                .speech_ended
                .retain(|k| *k != cur.key);
        }
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
                enqueued: Instant::now(),
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
        if self.cfg.split_on_speaker_change {
            self.resolve_deferred_split_at_end(cur.key);
        }
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
                enqueued: Instant::now(),
            });
        } else if (self.cfg.recheck_on_other || self.cfg.split_on_speaker_change)
            && self.outcome_for(cur.key) == Some(Outcome::Other)
            && !self.recheck_in_flight(cur.key)
            && !self.change_pending(cur.key)
        {
            // Ended with no re-check window in flight: the `other` verdict stands and the barge-in
            // held for a further window is dropped. A window in flight keeps it (the final awaits
            // that result). So does a confirmed speaker change the agent has not applied yet: the
            // split drops it, a skipped `other` -> `caller` split releases it
            // (`upgrade_after_skipped_split`), and a final that comes first settles it. The split
            // implies the re-check hold (`recheck_open_locked`), so it also implies this release.
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
        // The split watch keeps scoring an `other` key's windows whatever `recheck_on_other`
        // says, so the split implies the re-check hold: U1's barge-in is held, not suppressed,
        // until the change (or a skipped split's upgrade) decides it.
        if !(self.cfg.recheck_on_other || self.cfg.split_on_speaker_change) {
            return false;
        }
        if self.recheck_in_flight(key) {
            return true;
        }
        // Decision hh: a deferred split is decided by the next `caller` window or at the end of
        // speech (the skip upgrade, which releases the barge-in), so its barge-in stays held even
        // past the window cap.
        if self.cfg.split_on_speaker_change
            && lock(&self.shared.book)
                .veto_pending
                .iter()
                .any(|(k, _, _)| *k == key)
        {
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
        {
            let mut book = lock(&self.shared.book);
            if !book.closed.contains(&key) {
                push_capped(&mut book.closed, key);
            }
        }
        drop(feed);
        // Decision hh: a final that reads while the utterance is still live (its STT final came
        // before VAD SpeechEnd) decides a deferred split as SpeechEnd does, before it reads.
        if self.cfg.split_on_speaker_change {
            self.resolve_deferred_split_at_end(key);
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

    /// The caller floor for U2 of the `other` -> `caller` change of `key` (U1), on U2's clock:
    /// booked with the change, so a taken change always has it. `None` for any other key.
    pub fn caller_floor_for(&self, key: u64) -> Option<u32> {
        lock(&self.shared.book)
            .caller_floors
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, f)| *f)
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
    ///
    /// `floor_ms` is U2's caller floor on U2's clock ([`Self::caller_floor_for`] of U1's key):
    /// U2's `caller` -> `other` cut searches from it and never lands before it (decision y).
    pub fn inherit_caller(&self, key: u64, floor_ms: Option<u32>) -> Verdict {
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
        self.send(Msg::Inherit {
            key,
            floor: floor_ms,
        });
        Verdict {
            key,
            outcome: Outcome::Caller,
            timed_out: false,
            reason: None,
            recheck: false,
            window: 1,
        }
    }

    /// The agent skipped the `other` -> `caller` split of `key` (`not_speaking`, `replay_*`,
    /// `stt_unavailable`) while the utterance is still live: the skip is final, so the utterance
    /// falls back to the upgrade of the gate with the split off. Books `caller` for `key`
    /// synchronously (the final then reads `caller` with the other speaker's words in it, decision
    /// h: leak rather than drop the caller); the verdict handler releases the held barge-in on the
    /// confirming window's verdict ([`Self::upgraded_after_skipped_split`]). The key stays a split
    /// key: no further window is cut and the adaptive reference commits nothing from it.
    pub fn upgrade_after_skipped_split(&self, key: u64) {
        book_skip_upgrade(&mut lock(&self.shared.book), key);
        self.shared.notify.notify_waiters();
    }

    /// Decision hh, SpeechEnd with a deferred `other` -> `caller` split still pending: today's
    /// outcome from the remembered window, which at the end of speech is the `not_speaking` skip.
    /// Books `caller` for `key` synchronously, before the final reads its verdict, and marks the
    /// key as split (no further window is cut, the adaptive reference commits nothing from it).
    /// The held barge-in is released once, by the verdict handler: the deferred window's verdict
    /// is delivered again after the upgrade (`Msg::DeferredEnd`) and reads as `caller`
    /// ([`Self::upgraded_after_skipped_split`]), as after the agent's `not_speaking` skip. Runs at
    /// SpeechEnd and when the final starts reading ([`Self::close_recheck`]), whichever is first.
    fn resolve_deferred_split_at_end(&self, key: u64) {
        let window = {
            let mut book = lock(&self.shared.book);
            if !book.speech_ended.contains(&key) {
                push_capped(&mut book.speech_ended, key);
            }
            let Some(i) = book.veto_pending.iter().position(|(k, _, _)| *k == key) else {
                return;
            };
            let (_, cut, window) = book.veto_pending.remove(i).unwrap_or((key, 0, 1));
            book_deferred_end_upgrade(&mut book, key, cut);
            window
        };
        self.shared.notify.notify_waiters();
        // As after the agent's skip: the confirming window's verdict is delivered after the
        // upgrade, so the verdict handler releases the held barge-in. The deferred window's
        // verdict went out before SpeechEnd and re-held it; nothing else would release it before
        // the final, which comes only after the playback.
        self.send(Msg::DeferredEnd { key, window });
    }

    /// Whether `key` was upgraded by [`Self::upgrade_after_skipped_split`].
    pub fn upgraded_after_skipped_split(&self, key: u64) -> bool {
        lock(&self.shared.book).skip_upgraded.contains(&key)
    }

    /// A confirmed speaker change for `key` is stored and not yet taken by the agent.
    fn change_pending(&self, key: u64) -> bool {
        lock(&self.shared.book).changes.iter().any(|c| c.key == key)
    }

    /// Test hook: confirmed speaker changes not yet taken.
    #[doc(hidden)]
    pub fn pending_speaker_changes(&self) -> usize {
        lock(&self.shared.book).changes.len()
    }

    /// Decision hh: `other` -> `caller` splits deferred and not yet decided (test hook).
    #[doc(hidden)]
    pub fn deferred_speaker_splits(&self) -> usize {
        lock(&self.shared.book).veto_pending.len()
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
    /// Span, end and clip of the last scored window of the watch's own label (the backward range
    /// of a split starts at its start).
    prev: Option<(u32, u32, Vec<u8>)>,
    /// A change was reported: the key is not watched further.
    done: bool,
    /// `other` label: a window of U1 scored `other` below [`CONFIDENT_OTHER_MAX_SCORE`]
    /// (decision bb); without one an `other` -> `caller` change is not a split.
    confident_other: bool,
    /// U2 of an `other` -> `caller` split (decision y): the caller floor on this key's clock. The
    /// `caller` -> `other` cut searches from it and never lands before it. `None` for any key not
    /// born from such a split.
    floor: Option<u32>,
}

impl Watch {
    fn armed(label: Outcome, prev: Option<(u32, u32, Vec<u8>)>) -> Self {
        Self {
            label,
            run: 0,
            first: None,
            prev,
            done: false,
            confident_other: false,
            floor: None,
        }
    }

    /// Open on `other`: a window of this key may confirm an `other` -> `caller` change, so the
    /// sub-window scan applies to it.
    fn open_on_other(&self) -> bool {
        !self.done && self.label == Outcome::Other
    }

    /// The next `caller` window confirms an `other` -> `caller` split (decision bb: U1 holds a
    /// confident `other` window). The verifier runs the split scan before [`watch_step`] on
    /// exactly these windows.
    fn splits_on_next_caller(&self) -> bool {
        self.open_on_other()
            && self.confident_other
            && self.run + 1 >= OTHER_TO_CALLER_CONFIRM_WINDOWS
    }
}

/// Audio for the `other` -> `caller` sub-window scan.
struct Refine {
    tl: Timeline,
    /// Start of the scan (the previous scored window's start; also the cut without a `caller`
    /// sub-window) and the end of the audio held.
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

    /// The longest quiet run over `[lo, hi)`, with the raw frames and the threshold kept. Power is
    /// smoothed over [`SPLIT_SMOOTH_FRAMES`] frames; a frame is quiet below the midpoint (dB) of
    /// the range's 10th and 90th percentile; ties go to the later run. `None` without a frame.
    fn quiet_scan(&self, lo: i64, hi: i64) -> Option<QuietScan> {
        let lo = lo.max(self.start_ms).max(0);
        let hi = hi.min(self.end_ms());
        let n = ((hi - lo).max(0) as usize) / SPLIT_FRAME_MS;
        if n == 0 {
            return None;
        }
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
        let raw_db = raw.into_iter().map(db).collect();
        let mut sorted = smooth.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let pct = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        let mid = (pct(0.1) + pct(0.9)) / 2.0;
        let (mut best_len, mut best_end, mut run) = (0_usize, 0_usize, 0_usize);
        for (i, v) in smooth.iter().enumerate() {
            if *v < mid {
                run += 1;
                if run >= best_len {
                    best_len = run;
                    best_end = i + 1;
                }
            } else {
                run = 0;
            }
        }
        Some(QuietScan {
            lo,
            raw_db,
            mid,
            best_end,
            best_len,
        })
    }
}

fn db(p: f64) -> f64 {
    10.0 * p.max(1e-12).log10()
}

/// The result of [`Timeline::quiet_scan`]: frame indexes count from `lo`.
struct QuietScan {
    lo: i64,
    /// Unsmoothed frame power, dB.
    raw_db: Vec<f64>,
    /// The quiet threshold (dB) of the smoothed power.
    mid: f64,
    /// One past the last frame of the longest quiet run.
    best_end: usize,
    best_len: usize,
}

impl QuietScan {
    fn at(&self, frame: usize) -> i64 {
        self.lo + (frame * SPLIT_FRAME_MS) as i64
    }

    /// Decision r (chunk 6e): a closed run (a loud frame follows inside the range) ends right
    /// before the next speaker's first loud raw frame, searched within the smoothing reach of the
    /// smoothed run end (the smoothing moves a run's edges by up to two frames). An open run (quiet
    /// to the range end) ends at the smoothed run end, but no later than `open_max` (decision s)
    /// and never before the run's raw start (the caller is still speaking until then): the run's
    /// first frame after any loud raw frames within the smoothing reach of its smoothed start.
    /// `None` without a quiet run.
    fn raw_cut(&self, open_max: i64) -> Option<i64> {
        if self.best_len == 0 {
            return None;
        }
        let n = self.raw_db.len();
        if self.best_end >= n {
            let start = self.best_end - self.best_len;
            let to = (start + SPLIT_SMOOTH_FRAMES / 2).min(n);
            let raw_start = (start..to)
                .rev()
                .find(|i| self.raw_db[*i] >= self.mid)
                .map_or(start, |i| i + 1);
            return Some(self.at(self.best_end).min(open_max).max(self.at(raw_start)));
        }
        let reach = SPLIT_SMOOTH_FRAMES / 2;
        let from = self
            .best_end
            .saturating_sub(reach)
            .max(self.best_end - self.best_len);
        let to = (self.best_end + reach).min(n);
        let at = (from..to)
            .find(|i| self.raw_db[*i] >= self.mid)
            .unwrap_or(self.best_end);
        Some(self.at(at))
    }
}

/// One scored window for the watch. The first scored (non-re-check) window labels the key. Each
/// later window of the opposite label extends a run, any other window (same label, unscored)
/// ends it. `caller` -> `other` reports after [`SPEAKER_CHANGE_CONFIRM_WINDOWS`] in a row;
/// `other` -> `caller` after [`OTHER_TO_CALLER_CONFIRM_WINDOWS`] (one); its cut is the previous
/// window's start here, and the verifier replaces it with the sub-window scan's cut. `span` is the window's
/// `(start_ms, end_ms)` on the utterance clock; `clip` is the PCM that was scored, ending at
/// `end_ms`.
fn watch_step(
    watches: &mut VecDeque<(u64, Watch)>,
    key: u64,
    outcome: Outcome,
    score: Option<f32>,
    recheck: bool,
    span: (u32, u32),
    clip: Vec<u8>,
) -> Option<SpeakerChange> {
    let scored = matches!(outcome, Outcome::Caller | Outcome::Other);
    // Decision bb: an `other` window below CONFIDENT_OTHER_MAX_SCORE is confident evidence. A
    // `None` score counts as confident: only the direct `watch_step` unit tests pass one (every
    // `other` verdict of the verifier carries its score; `run` asserts it).
    let confident =
        outcome == Outcome::Other && score.is_none_or(|s| s < CONFIDENT_OTHER_MAX_SCORE);
    if !recheck {
        if scored {
            watches.retain(|(k, _)| *k != key);
            let mut w = Watch::armed(outcome, Some((span.0, span.1, clip)));
            w.confident_other = confident;
            watches.push_back((key, w));
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
        if scored {
            w.prev = Some((span.0, span.1, clip));
        }
        w.confident_other |= confident;
        return None;
    }
    if w.label == Outcome::Other && !w.confident_other {
        // Decision bb: no confident `other` window in U1, so no split. The watch ends and the
        // window's `caller` verdict upgrades the utterance (the re-check upgrade).
        w.done = true;
        return None;
    }
    let confirm = if w.label == Outcome::Other {
        OTHER_TO_CALLER_CONFIRM_WINDOWS
    } else {
        SPEAKER_CHANGE_CONFIRM_WINDOWS
    };
    if w.run == 0 && w.label == Outcome::Caller {
        w.first = Some((span.0, span.1, clip.clone()));
    }
    w.run += 1;
    if w.run < confirm {
        return None;
    }
    w.done = true;
    if w.label == Outcome::Other {
        // `other` -> `caller`: the cut without a `caller` sub-window.
        let split_at_ms = other_to_caller_split(w.prev.take().as_ref(), span, &clip).lo as u32;
        return Some(SpeakerChange {
            key,
            from: Outcome::Other,
            to: Outcome::Caller,
            split_at_ms,
        });
    }
    let first = w.first.take()?;
    Some(SpeakerChange {
        key,
        from: w.label,
        to: outcome,
        split_at_ms: caller_to_other_split(w.prev.take(), first, w.floor),
    })
}

/// The `caller` -> `other` split point (chunk 6e, decisions q, r and s): the longest quiet run
/// (ties later) from the previous `caller` window's start to the middle of the
/// first `other` window, so the cut lands after the caller's speech: in the gap before a
/// bystander, or in the bed after a short caller phrase whose bed windows scored `other`. A closed
/// run cuts right before the next speaker's first loud raw frame; an open run (quiet to the range
/// end) cuts at its end, so the caller's fading last word is never cut, but no later than
/// [`CALLER_TO_OTHER_OPEN_CUT_MAX_MS`] after the first `other` window's start
/// ([`QuietScan::raw_cut`]). Without a previous window that tiles into the first `other` one (an unscored window in between), the
/// range starts [`SPLIT_SEARCH_MS`] before the window (its pre-roll). Without a quiet run (no
/// frame in range), today's [`split_point`]. Bench: `CALLER_TO_OTHER_BENCH.md` (cut point).
///
/// `floor` (decision y, U2 of an `other` -> `caller` split only): the range starts at
/// `max(range start, floor)` and the cut is never before the floor; an empty range
/// (`floor >= hi`) cuts at the floor. The audio before the floor is proven caller (the sub-window
/// that confirmed U2), so the pre-onset gap U2's backed-off start holds is never a cut candidate.
fn caller_to_other_split(
    prev: Option<(u32, u32, Vec<u8>)>,
    first: (u32, u32, Vec<u8>),
    floor: Option<u32>,
) -> u32 {
    let (start, end, clip) = first;
    let hi = i64::from(start) + i64::from(end.saturating_sub(start) / 2);
    let prev = prev.filter(|(_, pend, _)| *pend + 2 * SPLIT_FRAME_MS as u32 >= start);
    let (tl, lo) = match prev.as_ref() {
        Some((pstart, pend, pclip)) => (
            Timeline::from_clips(&[(*pend, pclip), (end, &clip)]),
            i64::from(*pstart),
        ),
        None => (
            Timeline::from_clips(&[(end, &clip)]),
            i64::from(start) - i64::from(SPLIT_SEARCH_MS),
        ),
    };
    if let Some(floor) = floor.filter(|f| i64::from(*f) >= hi) {
        return floor;
    }
    let lo = floor.map_or(lo, |f| lo.max(i64::from(f)));
    tl.quiet_scan(lo, hi)
        .and_then(|q| q.raw_cut(i64::from(start) + i64::from(CALLER_TO_OTHER_OPEN_CUT_MAX_MS)))
        .map_or_else(|| split_point(start, end, &clip), |at| at.max(0) as u32)
        .max(floor.unwrap_or(0))
}

/// U2's caller floor (decision y) on U2's clock, for an `other` -> `caller` cut `cut` (U1's clock)
/// whose sub-window scan found the first `caller` sub-window at `found`: the end of that
/// sub-window (`found + SPLIT_STAGE2_SUB_MS`). U2's clock starts at the agent's split frame,
/// `cut` rounded DOWN to [`SPLIT_FRAME_MS`] (`split_live_utterance`), so the floor is measured
/// from that rounded origin: it is exact on U2's audio (up to one frame later than `found +
/// SUB - cut`). With no `caller` sub-window (fallback cut or a budget stop): [`SPLIT_BACKOFF_MS`]
/// from U2's start.
fn caller_floor(cut: u32, found: Option<i64>) -> u32 {
    let origin = i64::from(cut) / SPLIT_FRAME_MS as i64 * SPLIT_FRAME_MS as i64;
    found.map_or(SPLIT_BACKOFF_MS, |p| {
        (p + i64::from(SPLIT_STAGE2_SUB_MS) - origin).max(0) as u32
    })
}

/// The audio of the `other` -> `caller` sub-window scan and its fallback cut. The scan starts at
/// the previous scored window's start (the cut without a `caller` sub-window) and runs over that
/// window and this one. Without a previous window that tiles into this one (an unscored window in
/// between), it runs over this window's own clip from its start - [`SPLIT_SEARCH_MS`]. The cut
/// without a `caller` sub-window is the returned `lo`.
fn other_to_caller_split(
    prev: Option<&(u32, u32, Vec<u8>)>,
    span: (u32, u32),
    clip: &[u8],
) -> Refine {
    let prev = prev.filter(|(_, end, _)| *end + 2 * SPLIT_FRAME_MS as u32 >= span.0);
    let (tl, lo) = match prev {
        Some((start, end, pclip)) => (
            Timeline::from_clips(&[(*end, pclip), (span.1, clip)]),
            i64::from(*start),
        ),
        None => (
            Timeline::from_clips(&[(span.1, clip)]),
            i64::from(span.0.saturating_sub(SPLIT_SEARCH_MS)),
        ),
    };
    let lo = lo.max(tl.start_ms).max(0);
    let audio_end = tl.end_ms();
    Refine { tl, lo, audio_end }
}

/// The `other` -> `caller` cut: the first `caller` sub-window's start minus [`SPLIT_BACKOFF_MS`],
/// never before the scan's start; the scan's start without one.
fn other_to_caller_cut(lo: i64, found: Option<i64>) -> u32 {
    found
        .map_or(lo, |p| (p - i64::from(SPLIT_BACKOFF_MS)).max(lo))
        .max(0) as u32
}

/// How a sub-window scan ended (decision hh tells a completed scan from a stopped one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanEnd {
    /// Start of the first `caller` sub-window.
    Found(i64),
    /// Every sub-window of the range was embedded (at least one); none scored `caller`.
    Completed,
    /// The range holds no full sub-window: nothing was scored, so nothing vetoes the split
    /// (today's fallback cut).
    Empty,
    /// A budget stop or an embed failure.
    Stopped,
}

impl ScanEnd {
    fn found(self) -> Option<i64> {
        match self {
            Self::Found(t) => Some(t),
            _ => None,
        }
    }
}

/// The `other` -> `caller` sub-window scan: embeds [`SPLIT_STAGE2_SUB_MS`] sub-windows on a
/// [`SPLIT_STAGE2_HOP_MS`] hop from the start of the range and returns the start of the first one
/// that scores `caller`. `what` names the use in the logs (`split` for a confirmed change,
/// `near-miss` for the re-check of a window just under the threshold). Runs on the verifier task,
/// inside the window's own verify budget counted from the check's enqueue: the whole scan has
/// `verify - SCAN_AWAIT_MARGIN_MS` minus what has passed since `enqueued` (queue wait and the
/// window's own embed), each embed `min(verify, left)`, so the window's verdict lands at least
/// [`SCAN_AWAIT_MARGIN_MS`] before a final awaiting it gives up, even when the check queued behind
/// another embed. No budget left skips the scan. [`ScanEnd::Stopped`] on a budget stop or an embed
/// failure, [`ScanEnd::Completed`] when no sub-window is `caller`; `waitedMs` is logged either way.
#[allow(clippy::too_many_arguments)]
async fn scan_caller_sub_window(
    embedder: &Arc<dyn SpeakerEmbedder>,
    reference: &[f32],
    threshold: f32,
    refine: &Refine,
    verify: Duration,
    enqueued: Instant,
    shared: &Shared,
    key: u64,
    what: &str,
) -> ScanEnd {
    let started = Instant::now();
    let total = verify
        .saturating_sub(Duration::from_millis(SCAN_AWAIT_MARGIN_MS))
        .saturating_sub(enqueued.elapsed());
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
                // The embed ran into the scan budget (always below `verify` once the window's own
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
        let then = if what == "split" {
            "cut at the previous window start"
        } else {
            "not confirmed"
        };
        shared.warn(format!(
            "speaker gate {what} scan {why} key={key} embeds={embeds} waitedMs={waited_ms}; {then}"
        ));
        return ScanEnd::Stopped;
    }
    crate::agent::voice_debug(format!(
        "speaker gate {what} scan key={key} embeds={embeds} found={} waitedMs={waited_ms}",
        found.is_some()
    ));
    match found {
        Some(t) => ScanEnd::Found(t),
        None if embeds == 0 => ScanEnd::Empty,
        None => ScanEnd::Completed,
    }
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
            Msg::DeferredEnd { key, window } => {
                on_verdict(Verdict {
                    key,
                    outcome: Outcome::Other,
                    timed_out: false,
                    reason: None,
                    recheck: true,
                    window,
                });
            }
            Msg::Inherit { key, floor } => {
                // U2 of an `other` -> `caller` split starts as the caller, with its own watch and
                // its caller floor (decision y).
                watches.retain(|(k, _)| *k != key);
                let mut watch = Watch::armed(Outcome::Caller, None);
                watch.floor = floor;
                watches.push_back((key, watch));
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
                enqueued,
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
                let mut window_score: Option<f32> = None;
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
                                window_score = Some(score);
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
                let span = (span_start, end_ms);
                // The `other` -> `caller` sub-window scan (watch on, re-check window of a key whose
                // watch is open on `other`), once per window, over the previous scored window and
                // this one. Bounded by the window's remaining verify budget.
                // - `near-miss`: the window scored in [NEAR_MISS_MIN_SCORE, threshold). One
                //   `caller` sub-window confirms it as the `caller` window (same cut, inheritance
                //   and barge path); a budget stop confirms nothing.
                // - `split`: the window scored `caller` and confirms a split
                //   ([`Watch::splits_on_next_caller`]). The scan gives the cut; a budget stop
                //   keeps the previous window's start. Decision hh: a completed scan with no
                //   `caller` sub-window defers the split (the window counts as `other`, the watch
                //   stays open, the fallback cut is remembered); a second `caller` window without
                //   a `caller` sub-window cuts at the remembered point; SpeechEnd with the split
                //   still deferred upgrades ([`SpeakerGate::resolve_deferred_split_at_end`]).
                let near = recheck
                    && outcome == Outcome::Other
                    && window_score.is_some_and(|s| s >= NEAR_MISS_MIN_SCORE && s < cfg.threshold);
                let split_window = recheck && outcome == Outcome::Caller;
                let mut scan: Option<(i64, ScanEnd)> = None;
                if let (true, Some(clip), Some(reference)) =
                    (near || split_window, clip.as_ref(), reference.as_ref())
                {
                    let refine = watches
                        .iter()
                        .find(|(k, _)| *k == key)
                        .filter(|(_, w)| {
                            if near {
                                w.open_on_other()
                            } else {
                                w.splits_on_next_caller()
                            }
                        })
                        .map(|(_, w)| other_to_caller_split(w.prev.as_ref(), span, clip));
                    if let Some(refine) = refine {
                        let found = scan_caller_sub_window(
                            &embedder,
                            reference,
                            cfg.threshold,
                            &refine,
                            verify,
                            enqueued,
                            &shared,
                            key,
                            if near { "near-miss" } else { "split" },
                        )
                        .await;
                        scan = Some((refine.lo, found));
                    }
                }
                let near_found = scan.and_then(|(_, end)| end.found()).filter(|_| near);
                let defer = split_window && matches!(scan, Some((_, ScanEnd::Completed))) && {
                    let mut book = lock(&shared.book);
                    let fresh = !book.veto_pending.iter().any(|(k, _, _)| *k == key)
                        && !book.split_keys.contains(&key);
                    if let (true, Some((lo, _))) = (fresh, scan) {
                        let cut = other_to_caller_cut(lo, None);
                        if book.speech_ended.contains(&key) {
                            // Deferred after SpeechEnd: resolved at once; the `other` verdict this
                            // window delivers reads `caller` through the skip upgrade.
                            book_deferred_end_upgrade(&mut book, key, cut);
                        } else {
                            push_capped(&mut book.veto_pending, (key, cut, window));
                        }
                    }
                    fresh
                };
                if defer {
                    crate::agent::voice_debug(format!(
                        "speaker gate split deferred key={key} window={window}: no caller sub-window"
                    ));
                }
                let watch_outcome = if near_found.is_some() {
                    Outcome::Caller
                } else if defer {
                    Outcome::Other
                } else {
                    outcome
                };
                debug_assert!(
                    outcome != Outcome::Other || window_score.is_some(),
                    "an `other` verdict carries its score (key={key})"
                );
                let mut change = clip.and_then(|clip| {
                    watch_step(
                        &mut watches,
                        key,
                        watch_outcome,
                        window_score,
                        recheck,
                        span,
                        clip,
                    )
                });
                // Decision bb: a near-miss confirm that is not a split (no confident `other`
                // window in U1) is the re-check upgrade: this window counts as `caller`.
                let outcome = if near_found.is_some() && change.is_none() {
                    Outcome::Caller
                } else {
                    outcome
                };
                // `other` -> `caller`: the cut comes from this window's sub-window scan.
                let o2c = change.is_some_and(|c| c.from == Outcome::Other);
                let mut o2c_found: Option<i64> = None;
                // Decision hh: a deferred split without a `caller` sub-window on this window cuts
                // at the remembered window's fallback point.
                let deferred_cut = o2c
                    .then(|| {
                        let mut book = lock(&shared.book);
                        let i = book.veto_pending.iter().position(|(k, _, _)| *k == key)?;
                        book.veto_pending.remove(i).map(|(_, c, _)| c)
                    })
                    .flatten();
                if let (true, Some((lo, end))) = (o2c, scan) {
                    let found = end.found();
                    if let Some(c) = change.as_mut() {
                        c.split_at_ms = match (found, deferred_cut) {
                            (None, Some(cut)) => cut,
                            _ => other_to_caller_cut(lo, found),
                        };
                    }
                    o2c_found = found;
                }
                // U1 of an `other` -> `caller` change stays `other`: nothing upgrades or barges on
                // it (the caller's speech is U2, which inherits the verdict).
                let outcome = if o2c || defer {
                    Outcome::Other
                } else {
                    outcome
                };
                let has_change = change.is_some();
                {
                    let mut book = lock(&shared.book);
                    if let Some(change) = change {
                        if o2c {
                            // Booked with the change: the agent reads it when it takes the change.
                            let floor = caller_floor(change.split_at_ms, o2c_found);
                            book.caller_floors.retain(|(k, _)| *k != key);
                            push_capped(&mut book.caller_floors, (key, floor));
                        }
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
                    if !shared.parked.load(Ordering::SeqCst)
                        || (shared.park_changes_only.load(Ordering::SeqCst) && !has_change)
                    {
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
    /// The split is the end of the quiet run around the silent frame of the first opposite
    /// window's clip (clip: 500 ms ending at 400 ms, so it starts at -100 ms; the silent frame is
    /// at 260 ms), bounded by the middle of that window (300 ms).
    #[test]
    fn caller_to_other_after_two_opposite_windows_splits_at_the_quiet_run() {
        let mut w = VecDeque::new();
        assert!(watch_step(
            &mut w,
            1,
            Outcome::Caller,
            None,
            false,
            (0, 200),
            tone(500, 9000)
        )
        .is_none());
        let first = clip_with_quiet_frame(500, 260 + 100);
        assert!(watch_step(&mut w, 1, Outcome::Other, None, true, (200, 400), first).is_none());
        let change = watch_step(
            &mut w,
            1,
            Outcome::Other,
            None,
            true,
            (400, 600),
            tone(500, 9000),
        )
        .expect("two opposite windows confirm the change");
        assert_eq!(
            change,
            SpeakerChange {
                key: 1,
                from: Outcome::Caller,
                to: Outcome::Other,
                split_at_ms: 300
            }
        );
        // Reported once.
        assert!(watch_step(
            &mut w,
            1,
            Outcome::Other,
            None,
            true,
            (600, 800),
            tone(500, 9000)
        )
        .is_none());
    }

    /// Mono s16le on the utterance clock: each part is `(ms, amp, alternating)`; alternating parts
    /// flip the sign sample by sample (same power).
    fn parts_pcm(parts: &[(usize, i16, bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (ms, amp, alt) in parts {
            for i in 0..ms * BYTES_PER_MS / 2 {
                let v = if *alt && i % 2 == 1 { -amp } else { *amp };
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    /// Feeds 1500 ms windows (clips with 300 ms pre-roll) of `pcm` (starting at -300 ms) through
    /// the watch with `outcomes`; returns the change.
    fn c2o_windows(pcm: &[u8], outcomes: &[Outcome]) -> Option<SpeakerChange> {
        let mut w = VecDeque::new();
        let mut change = None;
        for (k, o) in outcomes.iter().enumerate() {
            let (start, end) = (k * 1500, (k + 1) * 1500);
            let clip = pcm[start * BYTES_PER_MS..(end + 300) * BYTES_PER_MS].to_vec();
            if let Some(c) =
                watch_step(&mut w, 1, *o, None, k > 0, (start as u32, end as u32), clip)
            {
                change = Some(c);
            }
        }
        change
    }

    /// U2 of an `other` -> `caller` split (decision y): its watch is armed as `caller` by
    /// `Msg::Inherit` with `floor`, so every scored window of U2 is a re-check. Windows are 1500 ms
    /// on U2's clock from 0 (window 2 is `(0, 1500)`), clips carry 300 ms of pre-roll (`pcm`
    /// starts at -300 ms). Returns the change.
    fn c2o_windows_u2(
        pcm: &[u8],
        outcomes: &[Outcome],
        floor: Option<u32>,
    ) -> Option<SpeakerChange> {
        let mut armed = Watch::armed(Outcome::Caller, None);
        armed.floor = floor;
        let mut w = VecDeque::from(vec![(1_u64, armed)]);
        let mut change = None;
        for (k, o) in outcomes.iter().enumerate() {
            let (start, end) = (k * 1500, (k + 1) * 1500);
            let clip = pcm[start * BYTES_PER_MS..(end + 300) * BYTES_PER_MS].to_vec();
            if let Some(c) = watch_step(&mut w, 1, *o, None, true, (start as u32, end as u32), clip)
            {
                change = Some(c);
            }
        }
        change
    }

    /// The n05 floor of the eu-load run 43618944729: cut A at 6075 (U1 clock) from a `caller`
    /// sub-window found at 6375; U2's origin is the split frame 6060, so the floor is
    /// 6375 + 750 - 6060 = 1065 on U2's clock.
    const N05_FLOOR: u32 = 1065;

    /// The n05 shape on U2 (eu-load 43618944729): U2 starts 624 ms before the caller's onset
    /// (cut A's back-off): the bystander's tail to 425 ms, a quiet gap to 625 ms, the caller's
    /// ~1.5 s phrase to 2150 ms, then the bed (100 ms of it before the range end, shorter than
    /// the gap). Window 2 scores `caller`, windows 3-4 `other` on
    /// the bed tail (a false caller -> other change). Without the floor (ac82b61b) the longest
    /// quiet run in [0, 2250) is the pre-onset gap and the cut lands right before the caller's
    /// first loud frame, moving the whole phrase to U3. With the floor the search starts at 1065
    /// and the cut lands after the phrase end, so U2 keeps the phrase and U3 is bed.
    #[test]
    fn split_born_u2_caller_to_other_cut_lands_after_the_phrase_n05() {
        let pcm = parts_pcm(&[
            (725, 9000, false),
            (200, 300, true),
            (1525, 9000, false),
            (2650, 300, true),
        ]);
        let outcomes = [Outcome::Caller, Outcome::Other, Outcome::Other];
        assert_eq!(caller_floor(6075, Some(6375)), N05_FLOOR);
        let c = c2o_windows_u2(&pcm, &outcomes, Some(N05_FLOOR)).expect("change");
        assert_eq!((c.from, c.to), (Outcome::Caller, Outcome::Other));
        assert!(c.split_at_ms >= N05_FLOOR, "{c:?}");
        // The search's 20 ms frames start at the floor (1065), so the phrase end (2150) falls
        // inside a frame: the cut is within one frame of it, never inside the phrase body.
        assert!(
            (2150 - SPLIT_FRAME_MS as u32..=2150 + 2 * SPLIT_FRAME_MS as u32)
                .contains(&c.split_at_ms),
            "the cut lands at the phrase end (2150): {c:?}"
        );
        // The same windows without the floor (ac82b61b): cut before the caller's onset.
        let old = c2o_windows_u2(&pcm, &outcomes, None).expect("change");
        assert!(old.split_at_ms <= 625 + SPLIT_FRAME_MS as u32, "{old:?}");
    }

    /// A real takeover after a split-born U2: the caller (625 .. 1900 ms) pauses 150 ms and a
    /// quieter speaker B takes over at 2050 ms. The cut is at or after the floor and inside the
    /// takeover boundary (the pause), right before B's first loud frame.
    #[test]
    fn split_born_u2_real_takeover_still_splits_after_the_floor() {
        let pcm = parts_pcm(&[
            (725, 9000, false),
            (200, 300, true),
            (1275, 9000, false),
            (150, 300, true),
            (2450, 4000, false),
        ]);
        let c = c2o_windows_u2(
            &pcm,
            &[Outcome::Caller, Outcome::Other, Outcome::Other],
            Some(N05_FLOOR),
        )
        .expect("change");
        assert!(c.split_at_ms >= N05_FLOOR, "{c:?}");
        assert!(
            (1900..=2050).contains(&c.split_at_ms),
            "inside the pause before B: {c:?}"
        );
    }

    /// No `caller` sub-window (fallback cut or a budget stop): the floor is `SPLIT_BACKOFF_MS`
    /// from U2's start. U2 starts in a 250 ms quiet gap before the caller (250 .. 2100 ms), then
    /// bed: without the floor the cut lands at the gap's end (before the caller); with the 300 ms
    /// floor the search starts after the gap and the cut lands after the phrase.
    #[test]
    fn split_born_u2_fallback_floor_is_the_back_off() {
        assert_eq!(caller_floor(1510, None), SPLIT_BACKOFF_MS);
        let pcm = parts_pcm(&[
            (300, 9000, false),
            (250, 300, true),
            (1850, 9000, false),
            (2400, 300, true),
        ]);
        let outcomes = [Outcome::Caller, Outcome::Other, Outcome::Other];
        let old = c2o_windows_u2(&pcm, &outcomes, None).expect("change");
        assert!(old.split_at_ms <= 250 + SPLIT_FRAME_MS as u32, "{old:?}");
        let c = c2o_windows_u2(&pcm, &outcomes, Some(SPLIT_BACKOFF_MS)).expect("change");
        assert!(c.split_at_ms >= SPLIT_BACKOFF_MS, "{c:?}");
        assert!(
            (2100..=2100 + 2 * SPLIT_FRAME_MS as u32).contains(&c.split_at_ms),
            "{c:?}"
        );
    }

    /// The floor on U2's clock: the found `caller` sub-window's end minus U2's origin, the cut
    /// rounded down to the split frame (the agent's `split_at_ms / 20`); clamped cut A included.
    #[test]
    fn caller_floor_is_the_sub_window_end_on_u2s_clock() {
        assert_eq!(caller_floor(1200, Some(1500)), 1050, "unclamped cut A");
        assert_eq!(
            caller_floor(1500, Some(1700)),
            950,
            "cut clamped to the scan start"
        );
        assert_eq!(
            caller_floor(1519, Some(1819)),
            1069,
            "origin rounded down to 1500"
        );
        assert_eq!(caller_floor(0, None), SPLIT_BACKOFF_MS);
    }

    /// Floor at or past the range end (window 2 itself scores `other`, so the range ends at 750):
    /// the cut is the floor, so the proven `caller` sub-window stays whole in U2.
    #[test]
    fn split_born_u2_empty_range_cuts_at_the_floor() {
        let pcm = parts_pcm(&[(725, 9000, false), (200, 300, true), (3875, 9000, false)]);
        let c = c2o_windows_u2(&pcm, &[Outcome::Other, Outcome::Other], Some(N05_FLOOR))
            .expect("change");
        assert_eq!(c.split_at_ms, N05_FLOOR);
    }

    /// Keys that are not split-born carry no floor: the n05 audio through a watch armed by a first
    /// window (today's path) cuts as on ac82b61b, the same as a U2 watch with no floor.
    #[test]
    fn keys_not_split_born_cut_as_before() {
        let pcm = parts_pcm(&[
            (725, 9000, false),
            (200, 300, true),
            (1525, 9000, false),
            (2650, 300, true),
        ]);
        let outcomes = [Outcome::Caller, Outcome::Other, Outcome::Other];
        let plain = c2o_windows(&pcm, &outcomes).expect("change");
        let u2 = c2o_windows_u2(&pcm, &outcomes, None).expect("change");
        assert_eq!(plain, u2);
        assert!(
            plain.split_at_ms <= 625 + SPLIT_FRAME_MS as u32,
            "{plain:?}"
        );
    }

    /// The n03 shape (chunk 6e): a caller phrase that ends 400 ms into window 3, then bed only;
    /// windows 3 and 4 score `other` and confirm a (false) caller -> other change. The cut lands
    /// after the caller's last voiced frame (3400 ms), so the caller's sentence stays whole and the
    /// `other` part is bed. Today's `split_point` (lowest-energy frame within 300 ms of the window
    /// start, one level throughout the caller) cut at 3000 ms, inside the phrase.
    #[test]
    fn caller_to_other_cut_lands_after_the_caller_phrase_on_bed_windows() {
        // -300 .. 3400 caller, 3400 .. 6300 bed.
        let pcm = parts_pcm(&[(3700, 9000, false), (2900, 300, true)]);
        let c = c2o_windows(
            &pcm,
            &[
                Outcome::Caller,
                Outcome::Caller,
                Outcome::Other,
                Outcome::Other,
            ],
        )
        .expect("two bed windows confirm the change");
        assert_eq!((c.from, c.to), (Outcome::Caller, Outcome::Other));
        assert!(
            (3401..=3750).contains(&c.split_at_ms),
            "cut {} after the caller's end (3400), within window 3's middle (3750)",
            c.split_at_ms
        );
        let first = pcm[3000 * BYTES_PER_MS..4800 * BYTES_PER_MS].to_vec();
        assert!(
            split_point(3000, 4500, &first) < 3400,
            "today's cut lands inside the caller's phrase"
        );
    }

    /// A real takeover: the caller ends at 3100 ms, a bystander starts at 3400 ms (300 ms gap at bed
    /// level). The cut lands in the gap: no caller word moves to the `other` part and no bystander
    /// word stays in the caller part.
    #[test]
    fn caller_to_other_takeover_cuts_between_the_caller_and_the_bystander() {
        // -300 .. 3100 caller, 3100 .. 3400 bed, 3400 .. 6300 bystander.
        let pcm = parts_pcm(&[(3400, 9000, false), (300, 300, true), (2900, 9000, true)]);
        let c = c2o_windows(
            &pcm,
            &[
                Outcome::Caller,
                Outcome::Caller,
                Outcome::Other,
                Outcome::Other,
            ],
        )
        .expect("the bystander confirms the change");
        assert_eq!((c.from, c.to), (Outcome::Caller, Outcome::Other));
        assert!(
            (3100..=3400).contains(&c.split_at_ms),
            "cut {} between the caller's end (3100) and the bystander's onset (3400)",
            c.split_at_ms
        );
    }

    /// Decisions r and s (chunk 6e): a steady quieter next speaker (-6 dB) with no pause at all,
    /// in the worst case: from the first `other` window's start (3000 ms). The quiet run is open
    /// (the bystander lasts to the range end), so the cut lands at or after the step, never inside
    /// the caller, and at most [`CALLER_TO_OTHER_OPEN_CUT_MAX_MS`] after the window start.
    #[test]
    fn caller_to_other_steady_quieter_bystander_cuts_at_or_after_the_step() {
        const STEP: u32 = 3000;
        // -300 .. 3000 caller, 3000 .. 6300 bystander at half the amplitude.
        let pcm = parts_pcm(&[(3300, 9000, false), (3300, 4500, true)]);
        let c = c2o_windows(
            &pcm,
            &[
                Outcome::Caller,
                Outcome::Caller,
                Outcome::Other,
                Outcome::Other,
            ],
        )
        .expect("the bystander confirms the change");
        assert_eq!((c.from, c.to), (Outcome::Caller, Outcome::Other));
        assert!(
            (STEP..=STEP + CALLER_TO_OTHER_OPEN_CUT_MAX_MS).contains(&c.split_at_ms),
            "cut {} at or after the step ({STEP}), at most {CALLER_TO_OTHER_OPEN_CUT_MAX_MS} ms later",
            c.split_at_ms
        );
    }

    /// The open-run cap never clips the caller: the caller stays loud until 3600 ms, 150 ms past
    /// the cap (3000 + [`CALLER_TO_OTHER_OPEN_CUT_MAX_MS`]), then bed. The cut waits for the quiet
    /// run, at or after the caller's last loud frame, instead of landing at 3450 ms.
    #[test]
    fn caller_to_other_open_run_cap_never_clips_the_caller() {
        const CALLER_END: u32 = 3600;
        // -300 .. 3600 caller, 3600 .. 6300 bed.
        let pcm = parts_pcm(&[(3900, 9000, false), (2700, 300, true)]);
        let c = c2o_windows(
            &pcm,
            &[
                Outcome::Caller,
                Outcome::Caller,
                Outcome::Other,
                Outcome::Other,
            ],
        )
        .expect("the bed windows confirm the change");
        assert_eq!((c.from, c.to), (Outcome::Caller, Outcome::Other));
        assert!(
            c.split_at_ms >= CALLER_END,
            "cut {} at or after the caller's last loud frame (ends {CALLER_END}), not at the cap \
             ({})",
            c.split_at_ms,
            3000 + CALLER_TO_OTHER_OPEN_CUT_MAX_MS
        );
    }

    #[test]
    fn one_false_opposite_window_does_not_split() {
        let mut w = VecDeque::new();
        assert!(watch_step(
            &mut w,
            1,
            Outcome::Caller,
            None,
            false,
            (0, 200),
            tone(500, 9000)
        )
        .is_none());
        for i in 1..=8_u32 {
            // other, caller, other, caller ...: never two opposite windows in a row.
            let outcome = if i % 2 == 1 {
                Outcome::Other
            } else {
                Outcome::Caller
            };
            let span = (i * 200, i * 200 + 200);
            assert!(
                watch_step(&mut w, 1, outcome, None, true, span, tone(500, 9000)).is_none(),
                "window {i}"
            );
        }
        // An unscored window also ends the run.
        assert!(watch_step(
            &mut w,
            2,
            Outcome::Caller,
            None,
            false,
            (0, 200),
            tone(500, 9000)
        )
        .is_none());
        assert!(watch_step(
            &mut w,
            2,
            Outcome::Other,
            None,
            true,
            (200, 400),
            tone(500, 9000)
        )
        .is_none());
        assert!(watch_step(
            &mut w,
            2,
            Outcome::Unknown,
            None,
            true,
            (400, 600),
            tone(500, 9000)
        )
        .is_none());
        assert!(watch_step(
            &mut w,
            2,
            Outcome::Other,
            None,
            true,
            (600, 800),
            tone(500, 9000)
        )
        .is_none());
    }

    /// `other` -> `caller` is confirmed by one `caller` window; the key is not watched further.
    #[test]
    fn other_to_caller_confirms_on_one_window() {
        let mut w = VecDeque::new();
        assert!(watch_step(
            &mut w,
            5,
            Outcome::Other,
            None,
            false,
            (0, 200),
            tone(500, 9000)
        )
        .is_none());
        let change = watch_step(
            &mut w,
            5,
            Outcome::Caller,
            None,
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
            None,
            true,
            (400, 600),
            tone(500, 9000)
        )
        .is_none());
        assert!(watch_step(
            &mut w,
            5,
            Outcome::Other,
            None,
            true,
            (600, 800),
            tone(500, 9000)
        )
        .is_none());
    }

    /// A single `other` window never splits `caller` -> `other`; two in a row do.
    #[test]
    fn a_single_other_window_never_splits_caller_to_other() {
        let mut w = VecDeque::new();
        assert!(watch_step(
            &mut w,
            6,
            Outcome::Caller,
            None,
            false,
            (0, 200),
            tone(500, 9000)
        )
        .is_none());
        assert!(watch_step(
            &mut w,
            6,
            Outcome::Other,
            None,
            true,
            (200, 400),
            tone(500, 9000)
        )
        .is_none());
        assert!(watch_step(
            &mut w,
            6,
            Outcome::Caller,
            None,
            true,
            (400, 600),
            tone(500, 9000)
        )
        .is_none());
        assert!(watch_step(
            &mut w,
            6,
            Outcome::Other,
            None,
            true,
            (600, 800),
            tone(500, 9000)
        )
        .is_none());
        let c = watch_step(
            &mut w,
            6,
            Outcome::Other,
            None,
            true,
            (800, 1000),
            tone(500, 9000),
        )
        .unwrap();
        assert_eq!((c.from, c.to), (Outcome::Caller, Outcome::Other));
    }

    /// Chunk 6g (decision x, config A): no stage 1 on `other` -> `caller`. Before the sub-window
    /// scan the change carries the previous window's start (the cut without a `caller`
    /// sub-window), even where a long quiet run ends later (window 1 here is silent from 800 ms;
    /// stage 1 used to cut at 1460), and the scan's audio runs from the previous window's start
    /// to the end of this window.
    #[test]
    fn other_to_caller_split_starts_at_the_previous_window_start() {
        let mut w = VecDeque::new();
        let mut first = tone(1500, 10_000);
        first[800 * BYTES_PER_MS..].fill(0);
        let r = other_to_caller_split(
            Some(&(0, 1500, first.clone())),
            (1500, 3000),
            &tone(1500, 10_000),
        );
        assert_eq!((r.lo, r.audio_end), (0, 3000));
        assert!(watch_step(&mut w, 1, Outcome::Other, None, false, (0, 1500), first).is_none());
        assert!(w[0].1.splits_on_next_caller());
        let change = watch_step(
            &mut w,
            1,
            Outcome::Caller,
            None,
            true,
            (1500, 3000),
            tone(1500, 10_000),
        )
        .unwrap();
        assert_eq!(change.split_at_ms, 0);
        assert!(!w[0].1.open_on_other(), "the change ends the watch");

        // Without a previous window the scan runs over the window's own clip from its start - 300
        // ms, never before the clip's first sample.
        let r = other_to_caller_split(None, (1500, 3000), &tone(1800, 10_000));
        assert_eq!((r.lo, r.audio_end), (1200, 3000));
        let r = other_to_caller_split(None, (1500, 3000), &tone(1500, 10_000));
        assert_eq!(r.lo, 1500);
    }

    /// The cut is the first `caller` sub-window's start minus [`SPLIT_BACKOFF_MS`], clamped to the
    /// scan's start; without a `caller` sub-window (or on a budget stop) it is the scan's start.
    #[test]
    fn other_to_caller_cut_backs_off_and_clamps_to_the_scan_start() {
        assert_eq!(other_to_caller_cut(1500, Some(2000)), 1700);
        assert_eq!(other_to_caller_cut(1500, Some(1700)), 1500, "clamped");
        assert_eq!(other_to_caller_cut(1500, Some(1500)), 1500, "clamped");
        assert_eq!(
            other_to_caller_cut(1500, None),
            1500,
            "no caller sub-window"
        );
        assert_eq!(other_to_caller_cut(0, Some(250)), 0, "clamped at 0");
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
        delay_gate_with(verify, script, None).await
    }

    /// [`delay_gate`], sending `before_window2` to the verifier right before window 2 fills (a
    /// check window 2 queues behind).
    async fn delay_gate_with(
        verify: u32,
        script: Vec<(u64, Vec<f32>)>,
        before_window2: Option<Msg>,
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
        if let Some(msg) = before_window2 {
            gate.send(msg);
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

    /// Option off (chunk 6e): the n03 script (caller, caller, other, other) gets the first verdict
    /// only, no window is watched and no change (so no cut) is reported.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn option_off_reports_no_caller_to_other_cut_on_the_n03_shape() {
        let (gate, got) =
            run_script(false, vec![caller_v(), caller_v(), other_v(), other_v()], 1).await;
        assert_eq!(
            got.iter()
                .map(|v| (v.outcome, v.recheck))
                .collect::<Vec<_>>(),
            vec![(Outcome::Caller, false)]
        );
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

    /// Decision y plumbing: a confirmed `other` -> `caller` change books U2's caller floor with
    /// the change (on U2's clock: the back-off without a `caller` sub-window, else the found
    /// sub-window's end, at most `SPLIT_BACKOFF_MS + SPLIT_STAGE2_SUB_MS` plus one frame of
    /// origin rounding). No change, or the flag off: no floor.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn other_to_caller_change_books_the_u2_caller_floor() {
        let script = vec![caller_v(), other_v(), caller_v(), caller_v(), caller_v()];
        let (on, _) = run_script(true, script.clone(), 2).await;
        assert_eq!(on.take_speaker_changes().len(), 1);
        let floor = on
            .caller_floor_for(1)
            .expect("floor booked with the change");
        let max = SPLIT_BACKOFF_MS + SPLIT_STAGE2_SUB_MS + SPLIT_FRAME_MS as u32;
        assert!((SPLIT_BACKOFF_MS..max).contains(&floor), "{floor}");
        assert_eq!(on.caller_floor_for(2), None);
        let (off, _) = run_script(false, script, 2).await;
        assert!(off.take_speaker_changes().is_empty());
        assert_eq!(off.caller_floor_for(1), None);
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
        let v = gate.inherit_caller(2, None);
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

    /// The sub-window scan: sub-windows of 750 ms on a 125 ms hop are embedded on the verifier task
    /// from the previous window's start (0 ms); the first `caller` one decides the cut, its start
    /// minus 300 ms. The script scores sub-windows other x4, caller: the fifth starts at 500 ms, so
    /// the cut is 200 ms, and the two vectors after it stay unused. (Chunk 6b expected 500..=1000
    /// with 1250/250 sub-windows moved forward to a quiet run; config A backs off instead, which
    /// loses fewer caller words, decision w.)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn split_cuts_300_ms_before_the_first_caller_sub_window() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let steps: VecDeque<Step> = [
            caller_v(), // enrollment
            other_v(),  // window 1
            caller_v(), // window 2: confirms
            other_v(),  // sub-window at 0 ms
            other_v(),  // 125 ms
            other_v(),  // 250 ms
            other_v(),  // 375 ms
            caller_v(), // 500 ms
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
        assert_eq!(changes[0].split_at_ms, 200);
        assert_eq!(lock(&queue).len(), 2, "exactly five sub-windows embedded");
    }

    /// 19 scripted `other` sub-windows: a split scan over 0..3000 ms (1500 ms windows) that
    /// completes without a `caller` sub-window.
    fn no_hit_scan(steps: &mut Vec<Step>) {
        steps.extend((0..19).map(|_| (None, other_v())));
    }

    /// Decision hh, the eu-load n02 shape (43725495546): U1 window 1 scores -0.064 (confident
    /// `other`), window 2 0.300 (near-miss, its scan finds no `caller` sub-window), window 3 just
    /// over the threshold on bystander-only audio, and its split scan completes (19 embeds) with
    /// no `caller` sub-window. The split is deferred: no change, window 3 is delivered as `other`
    /// (the barge-in stays held), U1 stays `other` and the watch stays open. A 4th `caller`
    /// window whose scan finds a `caller` sub-window at 500 ms into its range splits there:
    /// window 3's start (3000 ms) + 500 - 300. On dabb7e29 window 3 split at 1500 ms.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn n02_scan_without_a_caller_sub_window_defers_the_split() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let thr = cfg.threshold;
        let script = |more: bool| {
            let mut steps: Vec<Step> = vec![
                (None, caller_v()),
                (None, scored_v(-0.064)),
                (None, scored_v(0.300)),
            ];
            no_hit_scan(&mut steps); // window 2's near-miss scan
            steps.push((None, scored_v(thr + 0.004)));
            no_hit_scan(&mut steps); // window 3's split scan
            if more {
                steps.push((None, caller_v()));
                steps.extend((0..4).map(|_| (None, other_v())));
                steps.push((None, caller_v()));
            }
            steps
        };
        let (gate, verdicts, _q) = run_script_steps(cfg.clone(), script(false).into(), 3, 75).await;
        assert!(
            gate.take_speaker_changes().is_empty(),
            "deferred, not split"
        );
        assert_eq!(
            lock(&verdicts)
                .iter()
                .find(|v| v.window == 3)
                .map(|v| (v.outcome, v.recheck, v.window)),
            Some((Outcome::Other, true, 3)),
            "window 3 is delivered as other: the barge-in stays held"
        );
        assert_eq!(gate.outcome_for(1), Some(Outcome::Other), "U1 stays other");
        assert!(!gate.upgraded_after_skipped_split(1));

        let (gate, _v, _q) = run_script_steps(cfg, script(true).into(), 4, 75).await;
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            (changes[0].from, changes[0].to, changes[0].split_at_ms),
            (
                Outcome::Other,
                Outcome::Caller,
                3000 + 500 - SPLIT_BACKOFF_MS
            )
        );
    }

    /// Decision hh: two `caller` windows whose scans both complete without a `caller` sub-window.
    /// The first defers; the second applies today's cut from the remembered window: its previous
    /// window's start (0 ms), not the second window's (1500 ms). On dabb7e29 the first window
    /// split and no 3rd verdict was delivered.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_scan_without_a_caller_sub_window_cuts_at_the_deferred_point() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let mut steps: Vec<Step> = vec![(None, caller_v()), (None, other_v()), (None, caller_v())];
        no_hit_scan(&mut steps);
        steps.push((None, caller_v()));
        no_hit_scan(&mut steps);
        let (gate, verdicts, queue) = run_script_steps(cfg, steps.into(), 3, 75).await;
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].split_at_ms, 0,
            "the deferred window's fallback cut"
        );
        assert_eq!(
            lock(&verdicts)
                .iter()
                .map(|v| (v.outcome, v.window))
                .collect::<Vec<_>>(),
            vec![
                (Outcome::Other, 1),
                (Outcome::Other, 2),
                (Outcome::Other, 3)
            ]
        );
        assert!(
            lock(&queue).is_empty(),
            "both scans embedded 19 sub-windows"
        );
        assert_eq!(gate.outcome_for(1), Some(Outcome::Other), "U1 stays other");
    }

    /// Decision hh: SpeechEnd with the split still deferred is the `not_speaking` skip: `caller`
    /// is booked for U1 synchronously (before the final reads it), the key is a skip-upgraded
    /// split key, and the held barge-in is kept for the final that reads `caller` (not dropped as
    /// for an `other` key). On dabb7e29 the change was booked on window 2 instead.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn speech_end_with_the_split_deferred_upgrades_to_caller() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let mut steps: Vec<Step> = vec![(None, caller_v()), (None, other_v()), (None, caller_v())];
        no_hit_scan(&mut steps);
        let (gate, verdicts, _q) = run_script_steps(cfg.clone(), steps.into(), 2, 75).await;
        assert!(gate.take_speaker_changes().is_empty(), "deferred");
        assert_eq!(gate.outcome_for(1), Some(Outcome::Other));
        gate.set_pending_barge(1, 0);
        let delivered = lock(&verdicts).len();
        gate.end_utterance();
        assert_eq!(
            gate.outcome_for(1),
            Some(Outcome::Caller),
            "booked at SpeechEnd"
        );
        assert_eq!(gate.settled_outcome(1), Some(Outcome::Caller));
        assert!(gate.upgraded_after_skipped_split(1));
        assert!(
            gate.has_pending_barge(1),
            "kept for the final that reads caller"
        );
        assert!(lock(&gate.shared.book).split_keys.contains(&1));
        assert!(lock(&gate.shared.book).veto_pending.is_empty());
        assert!(gate.take_speaker_changes().is_empty());
        // The state that hung the agent's final: the window 2 verdict already went out (`other`,
        // barge-in re-held) and no window follows. The confirming window's verdict is delivered
        // again after the upgrade, so the verdict handler settles the held barge-in.
        for _ in 0..400 {
            if lock(&verdicts).len() > delivered {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let after: Vec<_> = lock(&verdicts)[delivered..]
            .iter()
            .map(|v| (v.key, v.outcome, v.recheck, v.window))
            .collect();
        assert_eq!(
            after,
            vec![(1, Outcome::Other, true, 2)],
            "one delivery after the upgrade"
        );
        assert!(gate.upgraded_after_skipped_split(1), "read as caller");
        let bound = Duration::from_millis(u64::from(cfg.verify_timeout_ms) + 500);
        assert_eq!(
            tokio::time::timeout(bound, gate.await_outcome(1))
                .await
                .ok(),
            Some(Outcome::Caller),
            "the final reads caller within the bound"
        );
    }

    /// Waits until `verdicts` holds at least `n` entries.
    async fn wait_verdicts(verdicts: &Arc<Mutex<Vec<Verdict>>>, n: usize) {
        for _ in 0..400 {
            if lock(verdicts).len() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("verdict {n} never arrived");
    }

    /// Decision hh at the window cap: window 3 (= `recheck_max_windows`) is `caller` but its
    /// scan finds no `caller` sub-window, so the split is deferred with no window left. The
    /// barge-in stays held (not suppressed) until SpeechEnd books `caller`. On e127a4bc the cap
    /// closed the hold and the barge-in was suppressed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_deferral_at_the_window_cap_keeps_the_barge_held() {
        let mut cfg = split_cfg(true, 3);
        cfg.check_speech_ms = 1500;
        let mut steps: Vec<Step> = vec![
            (None, caller_v()),
            (None, other_v()),
            (None, other_v()),
            (None, caller_v()),
        ];
        no_hit_scan(&mut steps);
        let (gate, verdicts, _q) = run_script_steps(cfg, steps.into(), 3, 75).await;
        assert_eq!(lock(&verdicts).last().map(|v| v.window), Some(3));
        assert_eq!(gate.deferred_speaker_splits(), 1, "window 3 deferred");
        assert!(gate.hold_for_recheck(1, 0), "held past the cap");
        assert_eq!(gate.barge_decision(1), BargeDecision::Defer);
        gate.end_utterance();
        assert_eq!(gate.outcome_for(1), Some(Outcome::Caller));
        assert!(
            gate.has_pending_barge(1),
            "left for the redelivered verdict"
        );
    }

    /// A key that ended (SpeechEnd seen) and resumes is live again: a later `caller` window whose
    /// scan finds no `caller` sub-window defers, it is not upgraded at once. Mirror: once that
    /// deferral is resolved at the next SpeechEnd (upgraded, a split key), a further resume stays
    /// `caller`: no re-check starts for a split key, so nothing defers or splits again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_resumed_key_defers_instead_of_upgrading() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let mut steps: Vec<Step> = vec![(None, caller_v()), (None, other_v()), (None, caller_v())];
        no_hit_scan(&mut steps);
        // Window 1 only (run_script_steps feeds extra speech after its last window).
        let gate = SpeakerGate::new(cfg);
        let verdicts: Arc<Mutex<Vec<Verdict>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&verdicts);
        let queue = Arc::new(Mutex::new(VecDeque::from(steps)));
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
        for _ in 0..15 {
            gate.push_frame(&[1_u8; FRAME]);
        }
        gate.begin(1, &alternating(0));
        for f in 1..75 {
            gate.push_frame(&alternating(f));
        }
        wait_verdicts(&verdicts, 1).await;
        gate.end_utterance();
        gate.begin(1, &alternating(0));
        for f in 1..75 {
            gate.push_frame(&alternating(f));
        }
        wait_verdicts(&verdicts, 2).await;
        for _ in 0..400 {
            if gate.deferred_speaker_splits() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(gate.deferred_speaker_splits(), 1, "deferred");
        assert_eq!(gate.outcome_for(1), Some(Outcome::Other));
        assert!(!gate.upgraded_after_skipped_split(1));
        // Mirror: resolved at the next SpeechEnd, then resumed once more.
        gate.end_utterance();
        assert_eq!(gate.outcome_for(1), Some(Outcome::Caller));
        gate.begin(1, &alternating(0));
        for f in 1..75 {
            gate.push_frame(&alternating(f));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(lock(&queue).is_empty(), "nothing else scripted");
        assert_eq!(gate.outcome_for(1), Some(Outcome::Caller), "stays caller");
        assert_eq!(gate.deferred_speaker_splits(), 0);
        assert!(gate.take_speaker_changes().is_empty());
    }

    /// Ruling 4: the final reads while the utterance is still live (`close_recheck` before
    /// SpeechEnd, an STT final ahead of VAD) with a deferral pending. It is decided there as at
    /// SpeechEnd: `caller` booked before the read, the deferred window's verdict delivered once
    /// more after it, nothing left pending.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_live_final_read_resolves_a_pending_deferral() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let mut steps: Vec<Step> = vec![(None, caller_v()), (None, other_v()), (None, caller_v())];
        no_hit_scan(&mut steps);
        let (gate, verdicts, _q) = run_script_steps(cfg, steps.into(), 2, 75).await;
        assert_eq!(gate.deferred_speaker_splits(), 1);
        gate.set_pending_barge(1, 0);
        let delivered = lock(&verdicts).len();
        gate.close_recheck(1);
        assert_eq!(
            gate.outcome_for(1),
            Some(Outcome::Caller),
            "booked before the read"
        );
        assert_eq!(gate.settled_outcome(1), Some(Outcome::Caller));
        assert_eq!(gate.deferred_speaker_splits(), 0, "nothing pending");
        wait_verdicts(&verdicts, delivered + 1).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let after: Vec<_> = lock(&verdicts)[delivered..]
            .iter()
            .map(|v| (v.key, v.outcome, v.recheck, v.window))
            .collect();
        assert_eq!(after, vec![(1, Outcome::Other, true, 2)], "one redelivery");
        assert!(gate.upgraded_after_skipped_split(1), "read as caller");
        gate.end_utterance();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(lock(&verdicts).len(), delivered + 1, "SpeechEnd adds none");
    }

    /// Ruling 5: reset (and disable) drop a pending deferral, so the old key is not upgraded at
    /// its end.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reset_drops_a_pending_deferral() {
        for disable in [false, true] {
            let mut cfg = split_cfg(true, 12);
            cfg.check_speech_ms = 1500;
            let mut steps: Vec<Step> =
                vec![(None, caller_v()), (None, other_v()), (None, caller_v())];
            no_hit_scan(&mut steps);
            let (gate, verdicts, _q) = run_script_steps(cfg, steps.into(), 2, 75).await;
            assert_eq!(gate.deferred_speaker_splits(), 1);
            let delivered = lock(&verdicts).len();
            if disable {
                gate.set_disabled();
            } else {
                gate.reset();
            }
            assert_eq!(gate.deferred_speaker_splits(), 0, "disable={disable}");
            gate.end_utterance();
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(!gate.upgraded_after_skipped_split(1), "disable={disable}");
            assert_ne!(
                gate.outcome_for(1),
                Some(Outcome::Caller),
                "disable={disable}"
            );
            assert_eq!(lock(&verdicts).len(), delivered, "no redelivery");
        }
    }

    /// Decision hh, the race: window 2's split scan is still running at SpeechEnd and completes
    /// with no `caller` sub-window after it. The deferral resolves at once as the skip upgrade
    /// (nothing is left pending for a SpeechEnd that already happened), and window 2's verdict,
    /// delivered after the upgrade, reads as `caller`. Without the fix the split stayed deferred
    /// with U1 `other`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_deferral_after_speech_end_upgrades_at_once() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let parked = Arc::new(Notify::new());
        let mut steps: Vec<Step> = vec![(None, caller_v()), (None, other_v()), (None, caller_v())];
        no_hit_scan(&mut steps);
        steps[3].0 = Some(Arc::clone(&parked));
        let left = steps.len() - 4;
        let (gate, verdicts, queue) = run_script_steps(cfg, steps.into(), 1, 75).await;
        for f in 0..75 {
            gate.push_frame(&alternating(f));
        }
        for _ in 0..400 {
            if lock(&queue).len() <= left {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(lock(&queue).len(), left, "window 2's scan is parked");
        gate.end_utterance();
        assert_eq!(
            gate.outcome_for(1),
            Some(Outcome::Other),
            "nothing deferred yet"
        );
        parked.notify_one();
        for _ in 0..400 {
            if lock(&verdicts).len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let v: Vec<_> = lock(&verdicts)
            .iter()
            .map(|v| (v.outcome, v.recheck, v.window))
            .collect();
        assert_eq!(v.len(), 2, "{v:?}");
        assert_eq!(
            v[1],
            (Outcome::Other, true, 2),
            "delivered after the upgrade"
        );
        assert!(gate.take_speaker_changes().is_empty());
        assert_eq!(gate.deferred_speaker_splits(), 0, "nothing left pending");
        assert_eq!(
            gate.outcome_for(1),
            Some(Outcome::Caller),
            "upgraded at once"
        );
        assert!(gate.upgraded_after_skipped_split(1));
        assert!(lock(&gate.shared.book).split_keys.contains(&1));
    }

    /// A scan embed that exceeds `verify_timeout_ms` falls back to the previous window's start
    /// (chunk 6b fell back to stage 1's point; there is no stage 1 now): the change is still
    /// reported, the wait is bounded and the warning names `waitedMs`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn split_scan_timeout_cuts_at_the_previous_window_start() {
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
        assert_eq!(changes[0].split_at_ms, 0, "the previous window's start");
        assert_eq!(lock(&verdicts).len(), 2);
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "bounded by verify_timeout_ms"
        );
        let warns = gate.warnings();
        assert!(
            warns
                .iter()
                .any(|w| w.contains("split scan total bound") && w.contains("waitedMs=")),
            "{warns:?}"
        );
    }

    /// The scan stays inside the window's own verify budget (`verify` 300 ms): the window embed
    /// returns at once and each sub-window embed takes 160 ms. The first sub-window embed returns
    /// with under 140 ms left, the second is bounded by what is left and always times out (its
    /// 160 ms sleep is longer than any remaining bound), so the scan stops with "split scan total
    /// bound" after at most 2 sub-window embeds and books the previous window's start (0 ms;
    /// chunk 6b booked stage 1's point). A runtime stall only
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
        assert!(stage2_embeds <= 2, "{stage2_embeds} scan embeds");
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].split_at_ms, 0);
        assert!(
            gate.warnings()
                .iter()
                .any(|w| w.contains("split scan total bound")
                    && w.contains("waitedMs=")
                    && !w.contains("embed timeout")),
            "{:?}",
            gate.warnings()
        );
    }

    /// With the window's verify budget already spent, the scan embeds nothing and reports
    /// "split scan total bound" with `waitedMs`; the cut stays the previous window's start.
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
        let first = tone(1500, 10_000);
        let clip = tone(1500, 10_000);
        let refine = other_to_caller_split(Some(&(0, 1500, first)), (1500, 3000), &clip);
        let point = refine.lo as u32;
        let counting = Arc::new(Counting(Mutex::new(0)));
        let embedder: Arc<dyn SpeakerEmbedder> = counting.clone();
        let gate = SpeakerGate::new(split_cfg(true, 12));
        let got = scan_caller_sub_window(
            &embedder,
            &caller_v(),
            0.5,
            &refine,
            Duration::from_millis(300),
            Instant::now() - Duration::from_millis(300),
            &gate.shared,
            1,
            "split",
        )
        .await;
        assert_eq!(got, ScanEnd::Stopped);
        assert_eq!((point, other_to_caller_cut(refine.lo, got.found())), (0, 0));
        assert_eq!(*lock(&counting.0), 0, "no embed with the budget spent");
        let warns = gate.warnings();
        assert!(
            warns
                .iter()
                .any(|w| w.contains("split scan total bound") && w.contains("waitedMs=")),
            "{warns:?}"
        );
    }

    /// Sub-window scan test embedder: the bystander alternates +-amp sample by sample, the caller holds
    /// +amp (same power), so a sub-window scores `caller` once at least half of it is caller audio
    /// (positive share >= 0.75), like a real embedding that needs most of its window.
    struct MajorityEmbedder;
    #[async_trait::async_trait]
    impl SpeakerEmbedder for MajorityEmbedder {
        async fn embed(
            &self,
            pcm: Bytes,
            _rate: u32,
        ) -> crate::error::SpeechResult<Option<Vec<f32>>> {
            let s = pcm.as_chunks::<2>().0;
            let pos = s.iter().filter(|b| i16::from_le_bytes(**b) > 0).count();
            Ok(Some(if pos * 4 >= s.len() * 3 {
                caller_v()
            } else {
                other_v()
            }))
        }
    }

    /// Bystander (`by_ms`), a quiet gap at a bed level (`gap_ms`), then the caller (`caller_ms`),
    /// on the utterance clock from 0.
    fn no_pause_scene(by_ms: usize, gap_ms: usize, caller_ms: usize) -> Refine {
        let mut pcm = Vec::new();
        for (ms, kind) in [(by_ms, 0), (gap_ms, 1), (caller_ms, 2)] {
            for i in 0..ms * BYTES_PER_MS / 2 {
                let v: i16 = match kind {
                    0 if i % 2 == 0 => 9000,
                    0 => -9000,
                    1 if i % 2 == 0 => 300,
                    1 => -300,
                    _ => 9000,
                };
                pcm.extend_from_slice(&v.to_le_bytes());
            }
        }
        let end = (pcm.len() / BYTES_PER_MS) as u32;
        let tl = Timeline::from_clips(&[(end, &pcm)]);
        let audio_end = tl.end_ms();
        Refine {
            tl,
            lo: 0,
            audio_end,
        }
    }

    /// The `other` -> `caller` cut on `refine`: the scan, then the back-off.
    async fn split_on(refine: &Refine) -> u32 {
        let embedder: Arc<dyn SpeakerEmbedder> = Arc::new(MajorityEmbedder);
        let gate = SpeakerGate::new(split_cfg(true, 12));
        let found = scan_caller_sub_window(
            &embedder,
            &caller_v(),
            0.5,
            refine,
            Duration::from_millis(5000),
            Instant::now(),
            &gate.shared,
            1,
            "split",
        )
        .await;
        other_to_caller_cut(refine.lo, found.found())
    }

    /// 6g step 1 (bench parity, not a CI test): replays the scenes `near_miss_bench` dumped with
    /// `NM_SPLIT_DUMP=<dir>` through the product's `other_to_caller_split` + `scan_caller_sub_window` (the
    /// bench's sub-window scores scripted in order, cosine = score) and compares each cut with the
    /// bench's `split_at`. Run: `NM_SPLIT_DUMP=<dir> cargo test -p node-webrtc-rust-speech
    /// bench_split_parity -- --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "bench parity: needs NM_SPLIT_DUMP"]
    async fn bench_split_parity() {
        let dir = std::path::PathBuf::from(std::env::var("NM_SPLIT_DUMP").expect("NM_SPLIT_DUMP"));
        let index = std::fs::read_to_string(dir.join("index.txt")).expect("index.txt");
        let (mut same, mut diff, mut found_n, mut deferred_n) =
            (0_usize, Vec::new(), 0_usize, 0_usize);
        for line in index.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            let n = f[0];
            let ps: u32 = f[1].parse().unwrap();
            let ws: u32 = f[2].parse().unwrap();
            let bench: u32 = f[3].parse().unwrap();
            let threshold: f32 = f[4].parse().unwrap();
            let scores: Vec<f32> = f[5..].iter().map(|x| x.parse().unwrap()).collect();
            let pcm = std::fs::read(dir.join(format!("{n}.pcm"))).unwrap();
            let cut = |a: u32, b: u32| {
                pcm[(a as usize * BYTES_PER_MS).min(pcm.len())
                    ..(b as usize * BYTES_PER_MS).min(pcm.len())]
                    .to_vec()
            };
            let pre = 300_u32;
            let prev = (ps, ws, cut(ps - pre, ws));
            let clip = cut(ws - pre, ws + 1500);
            let refine = other_to_caller_split(Some(&prev), (ws, ws + 1500), &clip);
            let queue = scores
                .iter()
                .map(|s| (None, vec![*s, (1.0 - s * s).max(0.0).sqrt()]))
                .collect();
            let embedder: Arc<dyn SpeakerEmbedder> = Arc::new(SeqEmbedder {
                queue: Mutex::new(queue),
            });
            let gate = SpeakerGate::new(split_cfg(true, 12));
            let found = scan_caller_sub_window(
                &embedder,
                &[1.0, 0.0],
                threshold,
                &refine,
                Duration::from_secs(60),
                Instant::now(),
                &gate.shared,
                1,
                "split",
            )
            .await;
            found_n += usize::from(found.found().is_some());
            // Decision hh: a completed scan with no `caller` sub-window defers the split (no cut
            // on this window); the dump's scan is unbudgeted, so the bench defers exactly when no
            // sub-window score reaches the threshold (its `bench_cut` is then the fallback).
            let got = match found {
                ScanEnd::Completed => None,
                end => Some(other_to_caller_cut(refine.lo, end.found())),
            };
            let want = scores.iter().any(|s| *s >= threshold).then_some(bench);
            if got == want {
                same += 1;
                deferred_n += usize::from(got.is_none());
            } else {
                diff.push(format!("{n}: product {got:?} bench {want:?}"));
            }
        }
        println!(
            "bench_split_parity: {same} same, {} differ, {found_n} with a caller sub-window, {deferred_n} deferred (decision hh)",
            diff.len()
        );
        for d in &diff {
            println!("  {d}");
        }
    }

    /// The n00 shape (no pause): the bystander ends at 2400 ms, the caller starts 140 ms later. A
    /// 750 ms sub-window scores `caller` once half of it is caller audio: the first is at 2250 ms,
    /// so the cut is 1950 ms, 450 ms inside the bystander's speech and 590 ms before the caller's
    /// onset. Chunk 6b cut in the gap (2400..=2540) by moving forward to a quiet run, which clipped
    /// real caller words on other scenes; decision w takes the leak to keep the caller's words, so
    /// the cut never lands after the onset.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_pause_cut_backs_off_before_the_caller_onset() {
        let at = split_on(&no_pause_scene(2400, 140, 1460)).await;
        assert_eq!(at, 2250 - SPLIT_BACKOFF_MS);
        assert!(
            at < 2540,
            "split {at}: never after the caller's onset at 2540"
        );
    }

    /// One level throughout (no gap): the cut is the first `caller` sub-window's start minus
    /// [`SPLIT_BACKOFF_MS`], before the caller's onset. (Chunk 6b cut at the sub-window start.)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_a_gap_the_cut_backs_off_from_the_first_caller_sub_window() {
        let refine = no_pause_scene(1500, 0, 2500);
        let at = split_on(&refine).await;
        let sub = SPLIT_STAGE2_SUB_MS as usize;
        let hop = SPLIT_STAGE2_HOP_MS as usize;
        // First sub-window with at least half caller audio.
        let first = (0..)
            .map(|k| k * hop)
            .find(|t| t + sub >= 1500 + sub / 2)
            .unwrap();
        assert_eq!(at as usize, first - SPLIT_BACKOFF_MS as usize);
        assert!((at as usize) < 1500, "never past the caller's onset");
    }

    /// A U1 final awaiting its verdict during a slow sub-window scan (50 ms embeds, the third is
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
        // Window 2 is in flight (its embed takes 100 ms, the scan another ~150 ms).
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

    /// Window 2 queues 600 ms behind another check's embed, then confirms an `other` -> `caller`
    /// change whose sub-window scan is slow (19 embeds of 100 ms, none `caller`). The scan's bound
    /// counts from window 2's enqueue and ends [`SCAN_AWAIT_MARGIN_MS`] before the final's await
    /// gives up, so the change (fallback cut) is booked before the final reads `other`. Counted
    /// from the dequeue (55fc6cb4), the scan ran ~600 ms past the await and the final saw no change.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_scan_queued_behind_another_embed_ends_before_the_final_gives_up() {
        let mut script = vec![
            (0, caller_v()),
            (0, other_v()),
            (600, other_v()),
            (0, caller_v()),
        ];
        script.extend((0..19).map(|_| (100, other_v())));
        let dummy = Msg::Check {
            key: 99,
            pcm: vec![1_u8; (PRE_ROLL_MS + 1500) * BYTES_PER_MS],
            speech_bytes: 1500 * BYTES_PER_MS,
            recheck: false,
            window: 1,
            attempt: 0,
            end_ms: 1500,
            enqueued: Instant::now(),
        };
        let (gate, _sink, _emb) = delay_gate_with(1000, script, Some(dummy)).await;
        let t0 = Instant::now();
        gate.end_utterance();
        let outcome = gate.await_outcome(1).await;
        assert_eq!(outcome, Outcome::Other);
        assert!(
            !gate.recheck_in_flight(1),
            "the await gave up with window 2 still in flight"
        );
        let changes = gate.take_speaker_changes();
        assert_eq!(
            changes.len(),
            1,
            "change booked before the final reads Other"
        );
        assert_eq!(
            changes[0].split_at_ms, 0,
            "fallback cut: previous window's start"
        );
        assert!(
            t0.elapsed() < Duration::from_millis(1000),
            "within verify_timeout_ms"
        );
    }

    /// With 1500 ms windows the cut can land up to start(k) + 450 ms: the last 750 ms sub-window
    /// starts at start(k) + 750 (the end of the held audio minus 750), minus [`SPLIT_BACKOFF_MS`].
    /// The first caller sub-window is the 19th (the last), at 2250 ms, with window 2 starting at
    /// 1500 ms. (Chunk 6b: up to start(k) + 750 with 1250/250 sub-windows moved forward 500 ms.)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn split_lands_at_most_450_ms_after_the_caller_window_start() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let mut steps: Vec<Step> = vec![(None, caller_v()), (None, other_v()), (None, caller_v())];
        steps.extend((0..18).map(|_| (None, other_v())));
        steps.push((None, caller_v()));
        let (gate, _v, queue) = run_script_steps(cfg, steps.into(), 2, 75).await;
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].split_at_ms, 1500 + 450);
        assert_eq!(lock(&queue).len(), 0, "all 19 sub-windows embedded");
    }

    /// A score in `[NEAR_MISS_MIN_SCORE, threshold)` (cosine with `caller_v`).
    fn near_v(cfg: &SpeakerGateConfig) -> Vec<f32> {
        assert!(cfg.threshold > NEAR_MISS_MIN_SCORE);
        let s = (NEAR_MISS_MIN_SCORE + cfg.threshold) / 2.0;
        vec![s, (1.0 - s * s).sqrt()]
    }

    /// The n05 shape (decision x): two windows just under the threshold, the second holding the
    /// caller's onset. Window 2's near-miss scan (audio 0..3000) finds no `caller` sub-window, so
    /// nothing is confirmed and window 2 becomes the previous window; window 3's scan (1500..4500)
    /// finds one at 2000 ms: the window is confirmed as the `caller` window, the change cuts at
    /// 1700 ms (2000 - 300), U1 stays `other`, and the found sub-window is not scanned again.
    /// Before chunk 6g both windows stayed `other` and no change was reported.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn near_miss_windows_confirm_on_one_caller_sub_window() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let near = near_v(&cfg);
        let mut steps: Vec<Step> =
            vec![(None, caller_v()), (None, other_v()), (None, near.clone())];
        steps.extend((0..19).map(|_| (None, other_v()))); // window 2's scan
        steps.push((None, near)); // window 3
        steps.extend((0..4).map(|_| (None, other_v()))); // window 3's scan: 1500..1875
        steps.push((None, caller_v())); // 2000 ms
        steps.push((None, caller_v())); // unused
        let (gate, verdicts, queue) = run_script_steps(cfg, steps.into(), 3, 75).await;
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            (changes[0].key, changes[0].from, changes[0].to),
            (1, Outcome::Other, Outcome::Caller)
        );
        assert_eq!(changes[0].split_at_ms, 1700);
        assert_eq!(
            lock(&queue).len(),
            1,
            "no second scan after the near-miss confirm"
        );
        assert_eq!(gate.outcome_for(1), Some(Outcome::Other), "U1 stays other");
        assert!(lock(&verdicts).iter().all(|v| v.outcome == Outcome::Other));
    }

    /// A bystander-only near-miss: no `caller` sub-window, no change, U1 `other`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_near_miss_without_a_caller_sub_window_does_not_split() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let near = near_v(&cfg);
        let mut steps: Vec<Step> = vec![(None, caller_v()), (None, other_v()), (None, near)];
        steps.extend((0..19).map(|_| (None, other_v())));
        let (gate, _v, queue) = run_script_steps(cfg, steps.into(), 2, 75).await;
        assert!(gate.take_speaker_changes().is_empty());
        assert_eq!(
            lock(&queue).len(),
            0,
            "the scan embedded all 19 sub-windows"
        );
        assert_eq!(gate.outcome_for(1), Some(Outcome::Other));
    }

    /// A vector scoring `s` against `caller_v`.
    fn scored_v(s: f32) -> Vec<f32> {
        vec![s, (1.0 - s * s).sqrt()]
    }

    /// Decision bb, the airport A utt-19 shape (chunk 7): window 1 scores 0.451 (`other`, just
    /// under 0.459) at the caller's own onset and window 2 scores `caller`. U1 holds no window
    /// below `CONFIDENT_OTHER_MAX_SCORE`, so there is no split: the window's `caller` verdict
    /// upgrades the whole utterance. On 08e37677 this split and dropped U1 as `other`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_near_threshold_first_window_then_caller_upgrades_without_a_split() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let steps: Vec<Step> = vec![
            (None, caller_v()),
            (None, scored_v(0.451)),
            (None, caller_v()),
        ];
        let (gate, verdicts, _q) = run_script_steps(cfg, steps.into(), 2, 75).await;
        assert!(gate.take_speaker_changes().is_empty(), "no split");
        assert_eq!(gate.outcome_for(1), Some(Outcome::Caller), "upgraded");
        let v = lock(&verdicts);
        assert!(
            v.iter()
                .any(|v| v.window == 2 && v.recheck && v.outcome == Outcome::Caller),
            "window 2 is the caller re-check: {v:?}"
        );
    }

    /// Decision bb: a near-miss confirm (window 2 in the band, a `caller` sub-window) with no
    /// confident `other` window in U1 (window 1 also in the band) is the re-check upgrade: the
    /// window's verdict is `caller` and no change is booked.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_near_miss_confirm_without_confident_other_upgrades() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let near = near_v(&cfg);
        let mut steps: Vec<Step> = vec![(None, caller_v()), (None, near.clone()), (None, near)];
        steps.extend((0..4).map(|_| (None, other_v()))); // window 2's scan: 0..375
        steps.push((None, caller_v())); // 500 ms
        steps.push((None, caller_v())); // unused
        let (gate, verdicts, _q) = run_script_steps(cfg, steps.into(), 2, 75).await;
        assert!(gate.take_speaker_changes().is_empty(), "no split");
        assert_eq!(gate.outcome_for(1), Some(Outcome::Caller), "upgraded");
        let v = lock(&verdicts);
        assert!(
            v.iter()
                .any(|v| v.window == 2 && v.recheck && v.outcome == Outcome::Caller),
            "window 2 is the caller re-check: {v:?}"
        );
    }

    /// U3 of a `caller` -> `other` split is judged from scratch: its first window is its own
    /// (non-re-check) check. When that window is `other` but near the threshold (0.40, so no
    /// window below `CONFIDENT_OTHER_MAX_SCORE`) and the next window is `caller`, the key is
    /// upgraded to `caller` and not split (decision bb): the caller's own voice scored a little low.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_u3_near_threshold_first_window_then_caller_upgrades_without_a_split() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let steps: Vec<Step> = vec![
            (None, caller_v()),
            (None, scored_v(0.40)),
            (None, caller_v()),
            (None, caller_v()), // unused
        ];
        let (gate, verdicts, _q) = run_script_steps(cfg, steps.into(), 2, 75).await;
        assert!(gate.take_speaker_changes().is_empty(), "no split");
        assert_eq!(gate.outcome_for(1), Some(Outcome::Caller), "upgraded");
        let v = lock(&verdicts);
        assert!(
            v.iter()
                .any(|v| v.window == 2 && v.recheck && v.outcome == Outcome::Caller),
            "window 2 is the caller re-check: {v:?}"
        );
    }

    /// Decision bb: a U1 with one window below `CONFIDENT_OTHER_MAX_SCORE` (the 6j worst case
    /// [0.370, 0.215, 0.190]) still splits on the next `caller` window, U1 stays `other`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_confident_other_window_in_u1_still_splits() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        let mut steps: Vec<Step> = vec![
            (None, caller_v()),
            (None, scored_v(0.370)),
            (None, scored_v(0.215)),
            (None, scored_v(0.190)),
            (None, caller_v()),
        ];
        // The split scan: a `caller` sub-window last (decision hh defers a scan without one).
        steps.extend((0..18).map(|_| (None, other_v())));
        steps.push((None, caller_v()));
        let (gate, _v, _q) = run_script_steps(cfg, steps.into(), 4, 75).await;
        let changes = gate.take_speaker_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            (changes[0].from, changes[0].to),
            (Outcome::Other, Outcome::Caller)
        );
        assert_eq!(gate.outcome_for(1), Some(Outcome::Other), "U1 stays other");
    }

    /// Decision bb on the watch itself: U1 [0.22, 0.41] (no window below 0.20) then `caller` is
    /// no change, and the watch is done (a later `caller` window reports nothing either); the
    /// same U1 with a 0.19 window splits.
    #[test]
    fn watch_needs_a_confident_other_window_to_split() {
        let mut w = VecDeque::new();
        assert!(watch_step(
            &mut w,
            1,
            Outcome::Other,
            Some(0.22),
            false,
            (0, 1500),
            tone(1500, 9000)
        )
        .is_none());
        assert!(watch_step(
            &mut w,
            1,
            Outcome::Other,
            Some(0.41),
            true,
            (1500, 3000),
            tone(1500, 9000)
        )
        .is_none());
        assert!(watch_step(
            &mut w,
            1,
            Outcome::Caller,
            Some(0.6),
            true,
            (3000, 4500),
            tone(1500, 9000)
        )
        .is_none());
        assert!(watch_step(
            &mut w,
            1,
            Outcome::Caller,
            Some(0.6),
            true,
            (4500, 6000),
            tone(1500, 9000)
        )
        .is_none());
        let mut w = VecDeque::new();
        assert!(watch_step(
            &mut w,
            2,
            Outcome::Other,
            Some(0.22),
            false,
            (0, 1500),
            tone(1500, 9000)
        )
        .is_none());
        assert!(watch_step(
            &mut w,
            2,
            Outcome::Other,
            Some(0.19),
            true,
            (1500, 3000),
            tone(1500, 9000)
        )
        .is_none());
        let c = watch_step(
            &mut w,
            2,
            Outcome::Caller,
            Some(0.6),
            true,
            (3000, 4500),
            tone(1500, 9000),
        );
        assert!(c.is_some_and(|c| c.from == Outcome::Other && c.to == Outcome::Caller));
    }

    /// A near-miss scan that runs out of the window's verify budget confirms nothing; the wait is
    /// bounded and the warning names `waitedMs`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_near_miss_scan_timeout_confirms_nothing() {
        let mut cfg = split_cfg(true, 12);
        cfg.check_speech_ms = 1500;
        cfg.verify_timeout_ms = 300;
        let never = Arc::new(Notify::new());
        let near = near_v(&cfg);
        let steps: VecDeque<Step> = VecDeque::from(vec![
            (None, caller_v()),
            (None, other_v()),
            (None, near),
            (Some(never), caller_v()), // first sub-window never returns
        ]);
        let t0 = Instant::now();
        let (gate, verdicts, _queue) = run_script_steps(cfg, steps, 2, 75).await;
        assert!(gate.take_speaker_changes().is_empty());
        // Window 2 is delivered `other`; later windows (nothing left to score) are `unknown`.
        let v2 = lock(&verdicts)[1];
        assert_eq!((v2.window, v2.outcome), (2, Outcome::Other));
        assert_eq!(gate.outcome_for(1), Some(Outcome::Other));
        assert!(t0.elapsed() < Duration::from_secs(5), "bounded");
        let warns = gate.warnings();
        assert!(
            warns
                .iter()
                .any(|w| w.contains("near-miss scan total bound") && w.contains("waitedMs=")),
            "{warns:?}"
        );
    }

    /// Option off: a near-miss window runs no scan, as before chunk 6g. The next vector is window
    /// 3's score (a scan would have taken it as its first sub-window), which upgrades the key to
    /// `caller` as the watch-off path always did; no change is reported.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn option_off_runs_no_near_miss_scan() {
        let mut cfg = split_cfg(false, 12);
        cfg.check_speech_ms = 1500;
        let near = near_v(&cfg);
        let steps: Vec<Step> = vec![
            (None, caller_v()),
            (None, other_v()),
            (None, near),
            (None, caller_v()), // window 3
        ];
        let (gate, v, queue) = run_script_steps(cfg, steps.into(), 3, 75).await;
        assert!(gate.take_speaker_changes().is_empty());
        assert_eq!(lock(&queue).len(), 0);
        assert_eq!(
            lock(&v)[2].outcome,
            Outcome::Caller,
            "window 3 scored caller_v"
        );
        assert_eq!(
            gate.outcome_for(1),
            Some(Outcome::Caller),
            "the watch-off upgrade"
        );
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

    /// `recheck_on_other` off: with the split off, `end_locked` leaves a held barge-in of an
    /// `other` key to the verdict handler (unchanged). With the split on, the split implies the
    /// re-check hold and so the SpeechEnd release: ended with nothing in flight and no change
    /// pending, the held barge-in is dropped (a later `cut_live` cannot fire a bystander barge).
    /// A re-check still in flight keeps it.
    #[test]
    fn end_drops_held_barge_with_split_on_without_recheck() {
        let kept = |split: bool, in_flight: bool| {
            let mut cfg = split_cfg(split, 12);
            cfg.recheck_on_other = false;
            let gate = SpeakerGate::new(cfg);
            gate.shared.enrolled.store(true, Ordering::SeqCst);
            let _rx = lock(&gate.rx).take().unwrap();
            start(&gate, 1, 10);
            store(&gate, 1, Outcome::Other);
            if in_flight {
                lock(&gate.shared.book).rechecks_in_flight.push_back(1);
            }
            gate.set_pending_barge(1, 0);
            gate.end_utterance();
            gate.has_pending_barge(1)
        };
        assert!(
            kept(false, false),
            "(a) split off: kept for the verdict handler"
        );
        assert!(
            !kept(true, false),
            "(b) split on, nothing in flight: dropped"
        );
        assert!(kept(true, true), "(c) split on, re-check in flight: kept");
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

    #[test]
    fn open_run_raw_start_skips_a_loud_frame_inside_the_reach() {
        // Smoothed open run starts at frame 2, but raw frame 3 is still loud (the caller): the
        // cut lands after it, at frame 4, not at the first quiet raw frame (2).
        let scan = QuietScan {
            lo: 0,
            raw_db: vec![-10.0, -10.0, -40.0, -25.0, -40.0, -40.0, -40.0],
            mid: -30.0,
            best_end: 7,
            best_len: 5,
        };
        assert_eq!(scan.raw_cut(0), Some((4 * SPLIT_FRAME_MS) as i64));
    }
}
