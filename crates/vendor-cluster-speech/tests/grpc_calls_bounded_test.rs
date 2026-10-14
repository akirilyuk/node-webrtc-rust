//! Guard (`infra.unbounded-external-wait`): every tonic `SpeechClient` call in this crate's
//! `src/` carries an explicit `tokio::time::timeout`. A server that accepts a call and never
//! sends response headers must not park the caller (2026-10 incident: an unbounded
//! `client.transcribe(request).await` stopped a VoiceAgent's whole inbound loop).

use std::path::{Path, PathBuf};

/// Lines are flagged when they call a `SpeechClient` method.
fn is_grpc_call(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") {
        return false;
    }
    line.contains("client.transcribe(")
        || line.contains("client.synthesize(")
        || line.contains(".transcribe(request)")
        || line.contains(".synthesize(request)")
}

/// `(1-based line, text)` of every unbounded gRPC call in `source`: no `timeout(` on the same
/// line or on one of the 3 preceding lines.
fn unbounded_calls(source: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !is_grpc_call(line) {
            continue;
        }
        let from = i.saturating_sub(3);
        let bounded = lines[from..=i].iter().any(|l| l.contains("timeout("));
        if !bounded {
            out.push((i + 1, line.trim().to_string()));
        }
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn every_speech_client_call_in_src_is_bounded() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(!files.is_empty(), "no source files found under {src:?}");

    let mut calls_seen = 0_usize;
    let mut failures = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("read source");
        calls_seen += text.lines().filter(|l| is_grpc_call(l)).count();
        for (line, _) in unbounded_calls(&text) {
            let name = file.strip_prefix(&src).unwrap_or(file).display();
            failures.push(format!(
                "src/{name}:{line}: gRPC call on SpeechClient without tokio::time::timeout (infra.unbounded-external-wait)"
            ));
        }
    }
    assert!(
        calls_seen >= 2,
        "scan found {calls_seen} SpeechClient calls; expected the transcribe and synthesize opens"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn matcher_flags_an_unbounded_call_and_accepts_a_bounded_one() {
    let unbounded = "let r = client.transcribe(request).await;";
    assert_eq!(unbounded_calls(unbounded).len(), 1);

    let bounded = "tokio::time::timeout(d, client.transcribe(request)).await";
    assert!(unbounded_calls(bounded).is_empty());

    let split =
        "let r = tokio::time::timeout(\n    d,\n    client.synthesize(request),\n)\n.await;";
    assert!(unbounded_calls(split).is_empty());

    let comment = "// client.transcribe(request) is bounded elsewhere";
    assert!(unbounded_calls(comment).is_empty());

    let far_timeout = "timeout(\n\n\n\n\nclient.synthesize(request)";
    assert_eq!(unbounded_calls(far_timeout).len(), 1);
}
