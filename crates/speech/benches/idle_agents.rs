//! Layer A bench for finding L10: CPU cost of idle started agents.
//!
//! Run: `cargo bench -p node-webrtc-rust-speech --bench idle_agents`
//!
//! Creates 100 started `VoiceAgent`s (mock vendor, no frames, no TTS) on a multi-thread runtime
//! with 2 workers, lets them idle for 5 s and measures process CPU time over that window with
//! `getrusage(RUSAGE_SELF)`. Prints one `perf_probe` with `idle_agent_cpu_us_per_agent_s`.
//! Plain `harness = false` main (not criterion).

#[path = "support/probe.rs"]
mod probe;

use std::sync::Arc;
use std::time::Duration;

use node_webrtc_rust_speech::VoiceAgent;

const AGENTS: usize = 100;
const IDLE_SECS: u64 = 5;

fn process_cpu_us() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    assert_eq!(rc, 0, "getrusage failed");
    let usage = unsafe { usage.assume_init() };
    let user = usage.ru_utime.tv_sec as u64 * 1_000_000 + usage.ru_utime.tv_usec as u64;
    let sys = usage.ru_stime.tv_sec as u64 * 1_000_000 + usage.ru_stime.tv_usec as u64;
    user + sys
}

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    let cpu_us = rt.block_on(async {
        let mut agents: Vec<Arc<VoiceAgent>> = Vec::with_capacity(AGENTS);
        for _ in 0..AGENTS {
            agents.push(probe::started_agent(probe::inbound_config()).await);
        }
        // Let start-up work settle before the measured window.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let before = process_cpu_us();
        tokio::time::sleep(Duration::from_secs(IDLE_SECS)).await;
        let after = process_cpu_us();

        for agent in &agents {
            agent.stop().await.unwrap();
        }
        after - before
    });

    let per_agent_s = cpu_us as f64 / AGENTS as f64 / IDLE_SECS as f64;
    probe::print_probe(
        "idle_agents",
        &[("idle_agent_cpu_us_per_agent_s", per_agent_s)],
    );
}
