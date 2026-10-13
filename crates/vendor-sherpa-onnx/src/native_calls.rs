//! Process-wide count of Sherpa native calls running on blocking threads.
//!
//! A tokio blocking thread (TTS `generate`, STT decode, LID compute) can still be inside
//! onnxruntime when the process exits, for example after the agent gave up on a worker
//! (bounded join timed out) or `process.exit` runs while a synthesis is in flight. ONNX
//! static destructors then run under that thread; the kernel hits a destroyed runtime, a
//! C++ exception unwinds into tokio's `catch_unwind`, and Rust aborts with `fatal runtime
//! error: Rust cannot catch foreign exceptions` (exit 134).
//!
//! The exit handler in `pool.rs` waits (bounded) for this count to reach zero before ONNX
//! teardown starts. In a live process nothing waits on it: a slow call just finishes.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Longest the exit handler waits for in-flight native calls. Exit never hangs past this.
pub(crate) const NATIVE_EXIT_DRAIN_BOUND: Duration = Duration::from_secs(10);
/// How often the exit handler re-reads the count.
pub(crate) const NATIVE_EXIT_DRAIN_POLL: Duration = Duration::from_millis(10);

static NATIVE_CALLS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Held for the duration of one native Sherpa call on a blocking thread.
///
/// Decrements on drop, so errors and panics release it too.
pub(crate) struct NativeCallGuard {
    counter: &'static AtomicUsize,
}

impl NativeCallGuard {
    /// Count one native call on the process-wide counter.
    pub(crate) fn enter() -> Self {
        Self::enter_on(&NATIVE_CALLS_IN_FLIGHT)
    }

    fn enter_on(counter: &'static AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self { counter }
    }
}

impl Drop for NativeCallGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Native calls currently running on blocking threads.
pub(crate) fn native_calls_in_flight() -> usize {
    NATIVE_CALLS_IN_FLIGHT.load(Ordering::SeqCst)
}

/// Wait until no native call is in flight. Returns `false` when `bound` expires first.
pub(crate) fn wait_for_native_calls_drained(bound: Duration, poll: Duration) -> bool {
    wait_for_counter_drained(&NATIVE_CALLS_IN_FLIGHT, bound, poll)
}

fn wait_for_counter_drained(counter: &AtomicUsize, bound: Duration, poll: Duration) -> bool {
    let deadline = Instant::now() + bound;
    loop {
        if counter.load(Ordering::SeqCst) == 0 {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        std::thread::sleep(poll.min(deadline - now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drained_immediately_at_zero() {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let started = Instant::now();
        assert!(wait_for_counter_drained(
            &COUNTER,
            Duration::from_secs(5),
            Duration::from_millis(10)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn drained_once_another_thread_releases_its_guard() {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let guard = NativeCallGuard::enter_on(&COUNTER);
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let releaser = std::thread::spawn(move || {
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(50));
            drop(guard);
        });
        held_rx.recv().unwrap();
        assert!(wait_for_counter_drained(
            &COUNTER,
            Duration::from_secs(5),
            Duration::from_millis(10)
        ));
        releaser.join().unwrap();
        assert_eq!(COUNTER.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn never_released_guard_is_given_up_on_within_the_bound() {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let _stuck = NativeCallGuard::enter_on(&COUNTER);
        let started = Instant::now();
        assert!(!wait_for_counter_drained(
            &COUNTER,
            Duration::from_millis(100),
            Duration::from_millis(10)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(COUNTER.load(Ordering::SeqCst), 1);
    }
}
