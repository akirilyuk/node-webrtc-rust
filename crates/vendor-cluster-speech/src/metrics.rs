use std::sync::atomic::{AtomicU64, Ordering};

static REOPEN_IDLE_ERROR: AtomicU64 = AtomicU64::new(0);
static REOPEN_RELOCATE: AtomicU64 = AtomicU64::new(0);
static REOPEN_GOAWAY: AtomicU64 = AtomicU64::new(0);

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
