//! Every `pub fn record_*` metric function in `otel/enabled.rs` has a production caller.
//!
//! The 0.11.0 STT actor move deleted the only call of `record_stt_latency_ms`, and the
//! `voice_stt_latency_ms` histogram stopped being exported without any test noticing. A metric
//! recording function that nothing calls is a silent metric outage.

use std::fs;
use std::path::{Path, PathBuf};

/// (function name, reason). A stale entry (now called, or gone) fails the test.
const RETIRED_OR_EXTERNAL: &[(&str, &str)] = &[];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .to_path_buf()
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if name != "target" && name != "node_modules" {
                rs_files(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

fn record_fn_names(src: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in src.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("pub fn record_") {
            let tail: String = rest
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
                .collect();
            if !tail.is_empty() {
                names.push(format!("record_{tail}"));
            }
        }
    }
    names
}

fn is_production_file(path: &Path, speech_otel: &Path, speech_otel_dir: &Path) -> bool {
    let s = path.to_string_lossy();
    path != speech_otel
        && !path.starts_with(speech_otel_dir)
        && !s.contains("/tests/")
        && !s.ends_with("_test.rs")
}

#[test]
fn every_otel_record_function_has_a_production_caller() {
    let crates = crates_dir();
    let speech_otel = crates.join("speech/src/otel.rs");
    let speech_otel_dir = crates.join("speech/src/otel");
    let enabled = fs::read_to_string(speech_otel_dir.join("enabled.rs")).unwrap();
    let names = record_fn_names(&enabled);
    assert!(!names.is_empty(), "no pub fn record_* found in enabled.rs");

    let mut files = Vec::new();
    rs_files(&crates, &mut files);
    let production: Vec<(PathBuf, String)> = files
        .into_iter()
        .filter(|p| is_production_file(p, &speech_otel, &speech_otel_dir))
        .map(|p| {
            let text = fs::read_to_string(&p).unwrap_or_default();
            (p, text)
        })
        .collect();

    let has_caller = |name: &str| {
        let needle = format!("{name}(");
        production.iter().any(|(_, text)| text.contains(&needle))
    };

    let allowed: Vec<&str> = RETIRED_OR_EXTERNAL.iter().map(|(n, _)| *n).collect();
    let missing: Vec<&String> = names
        .iter()
        .filter(|n| !allowed.contains(&n.as_str()) && !has_caller(n))
        .collect();
    assert!(
        missing.is_empty(),
        "otel record functions without a production call site: {missing:?}"
    );

    for (name, reason) in RETIRED_OR_EXTERNAL {
        assert!(!reason.is_empty(), "allowlist entry {name} needs a reason");
        assert!(
            names.iter().any(|n| n == name),
            "stale allowlist entry {name}: no longer defined in enabled.rs"
        );
        assert!(
            !has_caller(name),
            "stale allowlist entry {name}: it now has a production caller"
        );
    }
}
