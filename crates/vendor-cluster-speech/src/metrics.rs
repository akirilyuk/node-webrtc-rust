use std::sync::atomic::{AtomicU64, Ordering};

static REOPEN_IDLE_ERROR: AtomicU64 = AtomicU64::new(0);
static REOPEN_RELOCATE: AtomicU64 = AtomicU64::new(0);
static REOPEN_GOAWAY: AtomicU64 = AtomicU64::new(0);
/// STT audio accepted by `push_audio` and not yet written to the speech pod, all sessions.
static STT_QUEUED_BYTES: AtomicU64 = AtomicU64::new(0);

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

pub fn reset_stt_reopen_metrics() {
    REOPEN_IDLE_ERROR.store(0, Ordering::Relaxed);
    REOPEN_RELOCATE.store(0, Ordering::Relaxed);
    REOPEN_GOAWAY.store(0, Ordering::Relaxed);
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
}
