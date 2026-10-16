//! A TTS synthesis still inside native Sherpa code must not abort the process at exit.
//!
//! Field failure: after the agent abandons a TTS worker (bounded join timed out) the process
//! died with `fatal runtime error: Rust cannot catch foreign exceptions` (exit 134). The
//! blocking thread was still inside `generate_with_config` while ONNX static destructors ran.
//!
//! Each case re-executes this test binary as a child (`NWR_TTS_EXIT_CHILD=<mode>`), because
//! the failure kills the whole process:
//!
//! - `live`: abandon the awaiting task, keep the process running until the sentence in
//!   flight ends. Must exit 0 (a slow synth in a live process just finishes).
//! - `exit`: abandon the task and call `std::process::exit(1)` while that sentence is still
//!   inside native code. Must exit 1 without the abort message.
//!
//! Needs `SHERPA_TTS_MODEL_PATH` (Piper/VITS bundle); skips when it is unset.

use std::process::{Command, Output};
use std::time::{Duration, Instant};

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_sherpa_onnx::{tts_generate_count, SherpaFactory};

const CHILD_ENV: &str = "NWR_TTS_EXIT_CHILD";
const CHILD_TEST: &str = "child_entry";
const FOREIGN_EXCEPTION: &str = "cannot catch foreign exceptions";

fn tts_config(model_path: String) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        voice: Some("0".into()),
        api_key: None,
        endpoint: None,
        sentence_case: Default::default(),
    }
}

/// ~100k characters, split into ~300 character sentences: the engine is inside a native
/// call for almost the whole run, and a long synthesis never finishes before the test acts.
fn long_text() -> String {
    "the quick brown fox jumps over the lazy dog and keeps on running ".repeat(1500)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_entry() {
    let Ok(mode) = std::env::var(CHILD_ENV) else {
        return;
    };
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let tts = std::sync::Arc::new(
        SherpaFactory
            .create_tts(&tts_config(model_path))
            .expect("create TTS"),
    );

    let tts_for_task = std::sync::Arc::clone(&tts);
    let text = long_text();
    let handle = tokio::spawn(async move {
        let began = Instant::now();
        let result = tts_for_task.synthesize(&text).await;
        println!(
            "CHILD_LONG_SYNTH_RETURNED ok={} after_ms={}",
            result.is_ok(),
            began.elapsed().as_millis()
        );
    });

    // Wait until the synthesis holds a slot, then give the blocking thread time to enter
    // native code.
    let started = Instant::now();
    while tts_generate_count() == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "synthesis never started"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("CHILD_SYNTH_RUNNING");

    match mode.as_str() {
        "exit" => {
            // Abandon the awaiting task and exit at once: the sentence being synthesised
            // keeps running on its blocking thread while the process tears down.
            handle.abort();
            std::process::exit(1);
        }
        "live" => {
            // Abandon the awaiting task like the agent does; the blocking thread keeps going.
            handle.abort();
            let _ = handle.await;
            // The pool has one engine here, so this short synthesis queues behind the
            // sentence still in flight: when it returns, the abandoned work is over.
            let waited = Instant::now();
            let _ = tokio::time::timeout(Duration::from_secs(60), tts.synthesize("done.")).await;
            println!("CHILD_SYNTH_DONE after_ms={}", waited.elapsed().as_millis());
            tokio::time::sleep(Duration::from_secs(2)).await;
            println!("LIVE_OK");
        }
        other => panic!("unknown child mode {other}"),
    }
}

fn run_child(mode: &str, model_path: &str, slots: &str) -> Output {
    Command::new(std::env::current_exe().expect("current_exe"))
        .args([CHILD_TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, mode)
        .env("SHERPA_TTS_MODEL_PATH", model_path)
        .env("SHERPA_POOL_MAX_CONCURRENT_TTS", slots)
        .output()
        .expect("spawn child")
}

fn model_path_or_skip() -> Option<String> {
    let path = std::env::var("SHERPA_TTS_MODEL_PATH").ok();
    if path.is_none() {
        eprintln!("skipped: SHERPA_TTS_MODEL_PATH is not set");
    }
    path
}

#[test]
fn live_process_survives_an_abandoned_synthesis() {
    let Some(model_path) = model_path_or_skip() else {
        return;
    };
    let out = run_child("live", &model_path, "1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("LIVE_OK"),
        "live child failed: status={:?}\nstdout tail:\n{}\nstderr tail:\n{}",
        out.status,
        tail(&stdout),
        tail(&stderr)
    );
}

#[test]
fn exit_during_native_synthesis_does_not_abort() {
    let Some(model_path) = model_path_or_skip() else {
        return;
    };
    let out = run_child("exit", &model_path, "2");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("CHILD_SYNTH_RUNNING"),
        "child never reached the in-flight state\nstdout tail:\n{}\nstderr tail:\n{}",
        tail(&stdout),
        tail(&stderr)
    );
    assert!(
        !stderr.contains(FOREIGN_EXCEPTION),
        "process aborted at exit with a foreign exception: status={:?}\nstderr tail:\n{}",
        out.status,
        tail(&stderr)
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "exit child must exit 1 (a signal or 134 means an abort): status={:?}\nstderr tail:\n{}",
        out.status,
        tail(&stderr)
    );
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(15)..].join("\n")
}
