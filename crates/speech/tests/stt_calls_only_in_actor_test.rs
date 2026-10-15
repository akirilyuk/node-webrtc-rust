//! Guard for the error class `rt.serial-queue-head-of-line`: no vendor await on the per-frame
//! inbound path.
//!
//! `VoiceAgent::process_inbound_pcm` runs once per 20 ms frame. If it awaited an STT vendor call
//! (push, finalize, poll, ready-wait), one slow vendor would hold up every later frame of the
//! session behind it. Only the per-agent STT actor (`src/stt_actor.rs`) and the provider adapters
//! in `src/pipeline.rs` may call the provider methods; everything else sends commands to the actor.
//!
//! This test scans `src/**/*.rs` for those calls and fails with every offending `file:line`.

use std::fs;
use std::path::{Path, PathBuf};

/// Provider methods that may be awaited on a vendor.
const PROVIDER_CALLS: &[&str] = &[
    ".finalize_utterance(",
    ".request_finalize(",
    ".push_audio(",
    ".push_and_poll(",
    ".poll_transcript(",
    ".poll_output(",
    ".wait_ready(",
    ".discard_queued_transcripts(",
];

/// Files (relative to `src/`) allowed to call the provider.
const ALLOWED_FILES: &[&str] = &["stt_actor.rs", "pipeline.rs"];

/// One provider call found outside the allowed files.
#[derive(Debug, PartialEq, Eq)]
struct Offence {
    file: String,
    line: usize,
    call: &'static str,
}

/// The offences in `source`, which is the content of `file` (relative to `src/`). Comment lines
/// are ignored; files in [`ALLOWED_FILES`] have none.
fn provider_calls_outside_actor(file: &str, source: &str) -> Vec<Offence> {
    if ALLOWED_FILES.contains(&file) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        for call in PROVIDER_CALLS {
            if line.contains(call) {
                out.push(Offence {
                    file: file.to_string(),
                    line: idx + 1,
                    call,
                });
            }
        }
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn stt_provider_is_only_called_from_the_actor() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(
        files.iter().any(|f| f.ends_with("agent.rs")),
        "scan must cover src/agent.rs"
    );

    let mut offences = Vec::new();
    for path in &files {
        let relative = path
            .strip_prefix(&src)
            .expect("under src")
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        offences.extend(provider_calls_outside_actor(&relative, &source));
    }

    let report: Vec<String> = offences
        .iter()
        .map(|o| format!("src/{}:{}: {}", o.file, o.line, o.call))
        .collect();
    assert!(
        offences.is_empty(),
        "STT provider calls outside src/stt_actor.rs and src/pipeline.rs \
         (rt.serial-queue-head-of-line: no vendor await on the inbound path):\n{}",
        report.join("\n")
    );
}

#[test]
fn scan_flags_a_provider_call_outside_the_actor() {
    let snippet = "async fn process_inbound_pcm(&self) -> SpeechResult<()> {\n    stt.finalize_utterance().await?;\n    Ok(())\n}\n";
    assert_eq!(
        provider_calls_outside_actor("agent.rs", snippet),
        vec![Offence {
            file: "agent.rs".into(),
            line: 2,
            call: ".finalize_utterance(",
        }]
    );
    // The same call in the actor, or in a comment, is fine.
    assert!(provider_calls_outside_actor("stt_actor.rs", snippet).is_empty());
    assert!(provider_calls_outside_actor(
        "agent.rs",
        "    // stt.finalize_utterance().await? is the actor's job\n"
    )
    .is_empty());
}
