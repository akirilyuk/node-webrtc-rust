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
    /// `speech_bytes` is the clip without the pre-roll.
    Check {
        key: u64,
        pcm: Vec<u8>,
        speech_bytes: usize,
    },
    Reset,
}

#[derive(Default)]
struct Book {
    verdicts: VecDeque<(u64, Outcome)>,
    sent: VecDeque<u64>,
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

    fn cap_bytes(&self) -> usize {
        let ms = if self.is_enrolled() {
            self.cfg.check_speech_ms
        } else {
            self.cfg.check_speech_ms.max(self.cfg.enroll_speech_ms)
        };
        ms as usize * BYTES_PER_MS
    }

    fn check_bytes(&self) -> usize {
        self.cfg.check_speech_ms as usize * BYTES_PER_MS
    }

    /// SpeechStart of utterance `key`; `frame` is the onset frame (16 kHz mono s16le).
    /// A second start for the same key (brief gap) continues the same utterance.
    ///
    /// One check per utterance key: the first verdict is final. A segment that joins a key whose
    /// check already went out (pending or settled) does not send another check, so a speaker who
    /// joins the utterance during the gate hold cannot relabel it.
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
        let cur = match resumed {
            Some((k, buf)) => Cur {
                key: k,
                buf,
                pre: 0,
                sent: true,
            },
            None => {
                let buf = std::mem::take(&mut feed.ring);
                Cur {
                    key,
                    pre: buf.len(),
                    buf,
                    sent: self.check_sent(key),
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
        let mut to_send = None;
        {
            let mut feed = lock(&self.feed);
            match feed.cur.as_mut() {
                Some(cur) => {
                    let speech = cur.buf.len() - cur.pre;
                    if speech < cap {
                        let room = cap - speech;
                        cur.buf.extend_from_slice(&frame[..frame.len().min(room)]);
                    }
                    let speech = cur.buf.len() - cur.pre;
                    if !cur.sent && speech >= check {
                        cur.sent = true;
                        to_send = Some((cur.key, cur.buf[..cur.pre + check].to_vec(), check));
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
        if let Some((key, pcm, speech_bytes)) = to_send {
            self.mark_sent(key);
            self.send(Msg::Check {
                key,
                pcm,
                speech_bytes,
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
            });
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

    /// Decide a barge-in attempt for the live utterance `key`.
    pub fn barge_decision(&self, key: u64) -> BargeDecision {
        if !self.is_enrolled() {
            return BargeDecision::Proceed;
        }
        match self.outcome_for(key) {
            Some(Outcome::Other) => BargeDecision::Suppress,
            Some(_) => BargeDecision::Proceed,
            None => BargeDecision::Defer,
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
    /// check was sent. `None` while a sent check has not settled (use [`Self::await_outcome`]
    /// off the frame path).
    pub fn settled_outcome(&self, key: u64) -> Option<Outcome> {
        if !self.check_sent(key) {
            return Some(self.outcome_for(key).unwrap_or(Outcome::Unknown));
        }
        self.outcome_for(key)
    }

    /// Verdict for the final of `key`: waits at most `verify_timeout_ms` for a check that was
    /// sent but has not settled, then `Unknown`.
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
                return o;
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
            } => {
                let started = Instant::now();
                let (outcome, timed_out) = match reference.as_ref() {
                    None => (Outcome::Unknown, false),
                    Some(_) if speech_bytes < min_check_bytes => (Outcome::Unknown, false),
                    Some(reference) => match embed_bounded(&embedder, pcm, verify).await {
                        Ok(Some(v)) => match cosine(reference, &v) {
                            Some(score) => {
                                crate::agent::voice_debug(format!(
                                    "speaker gate score key={key} score={score:.3}"
                                ));
                                if score >= cfg.threshold {
                                    (Outcome::Caller, false)
                                } else {
                                    (Outcome::Other, false)
                                }
                            }
                            None => (Outcome::Unknown, false),
                        },
                        Ok(None) => (Outcome::Unknown, false),
                        Err(EmbedFail::Timeout) => {
                            let elapsed_ms = started.elapsed().as_millis();
                            shared.warn(format!(
                                "speaker gate verify timeout key={key} elapsed_ms={elapsed_ms}"
                            ));
                            (Outcome::Unknown, true)
                        }
                        Err(EmbedFail::Error) => (Outcome::Unknown, false),
                    },
                };
                {
                    let mut book = lock(&shared.book);
                    push_capped(&mut book.verdicts, (key, outcome));
                }
                shared.notify.notify_waiters();
                on_verdict(Verdict {
                    key,
                    outcome,
                    timed_out,
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

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]).unwrap() - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).unwrap().abs() < 1e-6);
        assert!(cosine(&[0.0, 0.0], &[1.0, 0.0]).is_none());
        assert!(cosine(&[1.0], &[1.0, 2.0]).is_none());
    }
}
