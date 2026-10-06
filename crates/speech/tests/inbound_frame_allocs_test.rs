//! Allocation guard for the inbound audio path: allocations per `process_inbound_pcm` call must
//! not grow. `BASELINE` is the value measured on unmodified code (phase 0 of the speech perf
//! plan); later phases only ratchet it down.
//!
//! The counter is thread-local and the agent runs on a `current_thread` runtime, so only the
//! measuring thread is counted and the number does not depend on scheduler timing.

#[path = "../benches/support/probe.rs"]
mod probe;

/// Allocations per voiced inbound frame (mock STT, energy VAD, gate open), measured at
/// origin/main c11d373 (0.9.26). 50 separate test-process runs all gave exactly 7.06
/// (3530 allocations over 500 frames), so no headroom is added.
const BASELINE: f64 = 7.06;

#[test]
fn inbound_frame_allocs_do_not_exceed_baseline() {
    let allocs_per_frame = probe::measure_inbound_allocs_per_frame();
    println!("inbound_frame_allocs={allocs_per_frame}");
    assert!(
        allocs_per_frame <= BASELINE,
        "inbound allocations per frame regressed: {allocs_per_frame} > baseline {BASELINE}"
    );
}
