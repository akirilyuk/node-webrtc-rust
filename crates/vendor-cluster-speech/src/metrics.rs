use std::sync::atomic::{AtomicU64, Ordering};

static REOPEN_IDLE_ERROR: AtomicU64 = AtomicU64::new(0);
static REOPEN_RELOCATE: AtomicU64 = AtomicU64::new(0);
static REOPEN_GOAWAY: AtomicU64 = AtomicU64::new(0);
/// STT audio accepted by `push_audio` and not yet written to the speech pod, all sessions.
static STT_QUEUED_BYTES: AtomicU64 = AtomicU64::new(0);

/// Per-utterance Transcribe streams opened from idle (`cluster_stt_utterance_streams_total`).
static UTTERANCE_STREAMS: AtomicU64 = AtomicU64::new(0);

/// Upper bounds (ms) of the `cluster_stt_stream_open_ms` histogram buckets; one extra overflow bucket.
pub const STREAM_OPEN_BUCKETS_MS: [u64; 10] = [10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000];

/// 16 kHz mono s16le: 32 bytes per millisecond.
const STT_BYTES_PER_MS: u64 = 32;

pub(crate) fn add_stt_queued_bytes(n: usize) {
    STT_QUEUED_BYTES.fetch_add(n as u64, Ordering::Relaxed);
}

/// Saturating subtract (a zeroed session can race with the worker's own subtract).
pub(crate) fn sub_stt_queued_bytes(n: usize) {
    let _ = STT_QUEUED_BYTES.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
        Some(cur.saturating_sub(n as u64))
    });
}

/// Milliseconds of STT audio queued behind the speech pod, summed over all sessions.
pub fn stt_queued_ms_total() -> u64 {
    STT_QUEUED_BYTES.load(Ordering::Relaxed) / STT_BYTES_PER_MS
}

pub fn inc_stt_reopen(reason: &str) {
    match reason {
        "idle_error" => {
            REOPEN_IDLE_ERROR.fetch_add(1, Ordering::Relaxed);
        }
        "relocate" => {
            REOPEN_RELOCATE.fetch_add(1, Ordering::Relaxed);
        }
        "goaway" => {
            REOPEN_GOAWAY.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
}

pub fn stt_reopen_total(reason: &str) -> u64 {
    match reason {
        "idle_error" => REOPEN_IDLE_ERROR.load(Ordering::Relaxed),
        "relocate" => REOPEN_RELOCATE.load(Ordering::Relaxed),
        "goaway" => REOPEN_GOAWAY.load(Ordering::Relaxed),
        _ => 0,
    }
}

/// Count one Transcribe stream opened for a single utterance from idle
/// (`cluster_stt_utterance_streams_total`).
pub fn inc_stt_utterance_streams() {
    UTTERANCE_STREAMS.fetch_add(1, Ordering::Relaxed);
}

pub fn stt_utterance_streams_total() -> u64 {
    UTTERANCE_STREAMS.load(Ordering::Relaxed)
}

/// Histogram `cluster_stt_stream_open_ms`, attribute `reason`: time from sending the Transcribe
/// open to the stream's `Ready`.
struct OpenHistogram {
    count: AtomicU64,
    sum_ms: AtomicU64,
    /// Non-cumulative bucket counts; index `STREAM_OPEN_BUCKETS_MS.len()` is the overflow bucket.
    buckets: [AtomicU64; STREAM_OPEN_BUCKETS_MS.len() + 1],
}

impl OpenHistogram {
    const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            sum_ms: AtomicU64::new(0),
            buckets: [const { AtomicU64::new(0) }; STREAM_OPEN_BUCKETS_MS.len() + 1],
        }
    }

    fn record(&self, ms: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_ms.fetch_add(ms, Ordering::Relaxed);
        let idx = STREAM_OPEN_BUCKETS_MS
            .iter()
            .position(|le| ms <= *le)
            .unwrap_or(STREAM_OPEN_BUCKETS_MS.len());
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    fn reset(&self) {
        self.count.store(0, Ordering::Relaxed);
        self.sum_ms.store(0, Ordering::Relaxed);
        for b in &self.buckets {
            b.store(0, Ordering::Relaxed);
        }
    }
}

static OPEN_MS_SESSION_START: OpenHistogram = OpenHistogram::new();
static OPEN_MS_UTTERANCE: OpenHistogram = OpenHistogram::new();
static OPEN_MS_REOPEN: OpenHistogram = OpenHistogram::new();

fn open_histogram(reason: &str) -> Option<&'static OpenHistogram> {
    match reason {
        "session_start" => Some(&OPEN_MS_SESSION_START),
        "utterance" => Some(&OPEN_MS_UTTERANCE),
        "reopen" => Some(&OPEN_MS_REOPEN),
        _ => None,
    }
}

/// Record one stream open latency. `reason`: `session_start` | `utterance` | `reopen`.
pub fn record_stt_stream_open_ms(reason: &str, ms: u64) {
    if let Some(h) = open_histogram(reason) {
        h.record(ms);
    }
}

/// `(count, sum_ms)` of `cluster_stt_stream_open_ms` for `reason`.
pub fn stt_stream_open_ms_stats(reason: &str) -> (u64, u64) {
    open_histogram(reason)
        .map(|h| {
            (
                h.count.load(Ordering::Relaxed),
                h.sum_ms.load(Ordering::Relaxed),
            )
        })
        .unwrap_or((0, 0))
}

/// Non-cumulative bucket counts of `cluster_stt_stream_open_ms` for `reason`
/// (bounds in [`STREAM_OPEN_BUCKETS_MS`], last entry is the overflow bucket).
pub fn stt_stream_open_ms_buckets(reason: &str) -> Vec<u64> {
    open_histogram(reason)
        .map(|h| {
            h.buckets
                .iter()
                .map(|b| b.load(Ordering::Relaxed))
                .collect()
        })
        .unwrap_or_default()
}

pub fn reset_stt_reopen_metrics() {
    REOPEN_IDLE_ERROR.store(0, Ordering::Relaxed);
    REOPEN_RELOCATE.store(0, Ordering::Relaxed);
    REOPEN_GOAWAY.store(0, Ordering::Relaxed);
    UTTERANCE_STREAMS.store(0, Ordering::Relaxed);
    OPEN_MS_SESSION_START.reset();
    OPEN_MS_UTTERANCE.reset();
    OPEN_MS_REOPEN.reset();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_ms_adds_subtracts_and_saturates() {
        sub_stt_queued_bytes(usize::MAX);
        add_stt_queued_bytes(64_000);
        sub_stt_queued_bytes(32_000);
        assert_eq!(stt_queued_ms_total(), 1000);
        sub_stt_queued_bytes(1_000_000);
        assert_eq!(stt_queued_ms_total(), 0);
    }

    #[test]
    fn open_histogram_buckets_sum_and_unknown_reason() {
        reset_stt_reopen_metrics();
        record_stt_stream_open_ms("utterance", 5);
        record_stt_stream_open_ms("utterance", 100);
        record_stt_stream_open_ms("utterance", 60_000);
        record_stt_stream_open_ms("bogus", 7);
        assert_eq!(stt_stream_open_ms_stats("utterance"), (3, 60_105));
        let b = stt_stream_open_ms_buckets("utterance");
        assert_eq!(b.len(), STREAM_OPEN_BUCKETS_MS.len() + 1);
        assert_eq!((b[0], b[3], *b.last().unwrap()), (1, 1, 1));
        assert_eq!(stt_stream_open_ms_stats("bogus"), (0, 0));
        assert_eq!(stt_stream_open_ms_stats("reopen"), (0, 0));
    }
}
