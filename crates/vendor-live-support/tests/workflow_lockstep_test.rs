//! Guard (`test.vacuous-pass`): a live test that no workflow runs, or that a workflow runs with
//! a missing key as a skip, is a green check that proves nothing.
//!
//! - Every `crates/vendor-*/tests/live_test.rs` has `.github/workflows/vendor-live-<id>.yml`
//!   with the fork-skip `if`, `_LIVE_REQUIRED: '1'`, the cargo command, `--test-threads=1`, and a
//!   pull_request path filter that covers the vendor crate, this support crate and the workflow.
//! - Every `crates/vendor-*/tests/live_bench.rs` is a vendor option of `vendor-bench.yml`.
//!
//! The checkers are plain functions over the workflow text so the negative tests can feed them
//! a broken workflow.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Vendor ids (`fishaudio`, `deepgram`, ...) of the crates that have `tests/<file>`.
fn vendors_with(file: &str) -> Vec<String> {
    let mut ids: Vec<String> = fs::read_dir(repo_root().join("crates"))
        .expect("crates dir")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = name.strip_prefix("vendor-")?.to_string();
            entry
                .path()
                .join("tests")
                .join(file)
                .is_file()
                .then_some(id)
        })
        .collect();
    ids.sort();
    ids
}

/// The lines under `pull_request:` that belong to it (deeper indent than the next sibling key).
fn pull_request_section(workflow: &str) -> String {
    let mut lines = workflow.lines();
    let mut section = String::new();
    for line in lines.by_ref() {
        if line.trim_end() == "  pull_request:" {
            break;
        }
    }
    for line in lines {
        let indent = line.len() - line.trim_start().len();
        if !line.trim().is_empty() && indent <= 2 {
            break;
        }
        section.push_str(line);
        section.push('\n');
    }
    section
}

/// Everything wrong with the live-test workflow of vendor `id`, empty when it is fine.
fn check_live_workflow(id: &str, workflow: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let mut require = |ok: bool, what: String| {
        if !ok {
            problems.push(what);
        }
    };
    require(
        workflow.contains("github.event.pull_request.head.repo.full_name == github.repository"),
        "missing the fork-skip `if:` (github.event.pull_request.head.repo.full_name == github.repository)"
            .into(),
    );
    require(
        workflow.contains("_LIVE_REQUIRED: '1'"),
        "missing `_LIVE_REQUIRED: '1'` (a missing key must fail the job, not skip)".into(),
    );
    let command =
        format!("cargo test -p node-webrtc-rust-vendor-{id} --features live --test live_test");
    require(workflow.contains(&command), format!("missing `{command}`"));
    require(
        workflow.contains("--test-threads=1"),
        "missing `--test-threads=1`".into(),
    );
    let paths = pull_request_section(workflow);
    for wanted in [
        format!("crates/vendor-{id}/**"),
        "crates/vendor-live-support/**".to_string(),
        format!(".github/workflows/vendor-live-{id}.yml"),
    ] {
        require(
            paths.contains(&wanted),
            format!("pull_request `paths` is missing `{wanted}`"),
        );
    }
    problems
}

/// Everything wrong with `vendor-bench.yml` for vendor `id`.
fn check_bench_workflow(id: &str, workflow: &str) -> Vec<String> {
    let listed = workflow
        .lines()
        .any(|line| line.trim() == format!("- {id}"));
    if listed {
        Vec::new()
    } else {
        vec![format!(
            "`{id}` is not in the vendor options of vendor-bench.yml"
        )]
    }
}

fn read_workflow(name: &str) -> Option<String> {
    fs::read_to_string(repo_root().join(".github/workflows").join(name)).ok()
}

#[test]
fn every_live_test_has_a_complete_workflow() {
    let ids = vendors_with("live_test.rs");
    assert!(
        !ids.is_empty(),
        "no crates/vendor-*/tests/live_test.rs found"
    );
    let mut failures = Vec::new();
    for id in &ids {
        let name = format!("vendor-live-{id}.yml");
        match read_workflow(&name) {
            None => failures.push(format!(
                "{name}: missing (crates/vendor-{id}/tests/live_test.rs exists)"
            )),
            Some(workflow) => {
                for problem in check_live_workflow(id, &workflow) {
                    failures.push(format!("{name}: {problem}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn every_live_bench_is_a_vendor_bench_option() {
    let ids = vendors_with("live_bench.rs");
    if ids.is_empty() {
        return;
    }
    let workflow = read_workflow("vendor-bench.yml")
        .expect("vendor-bench.yml must exist once a live_bench.rs exists");
    let failures: Vec<String> = ids
        .iter()
        .flat_map(|id| check_bench_workflow(id, &workflow))
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

const FORK_SKIP: &str = "github.event.pull_request.head.repo.full_name == github.repository";

/// The `- id` items under `options:` of the workflow_dispatch `vendor` input.
fn dispatch_options(workflow: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut in_options = false;
    for line in workflow.lines() {
        let trimmed = line.trim();
        if trimmed == "options:" {
            in_options = true;
        } else if in_options {
            match trimmed.strip_prefix("- ") {
                Some(id) => ids.push(id.to_string()),
                None => break,
            }
        }
    }
    ids.sort();
    ids
}

/// The quoted ids of the fixed list in the `matrix: vendor:` expression.
fn matrix_vendors(workflow: &str) -> Vec<String> {
    let Some(line) = workflow
        .lines()
        .find(|l| l.trim_start().starts_with("vendor: ${{ fromJSON("))
    else {
        return Vec::new();
    };
    let Some(list) = line.rsplit_once("|| '[").map(|(_, rest)| rest) else {
        return Vec::new();
    };
    let list = list.split(']').next().unwrap_or("");
    let mut ids: Vec<String> = list
        .split(',')
        .map(|v| v.trim().trim_matches('"').to_string())
        .filter(|v| !v.is_empty())
        .collect();
    ids.sort();
    ids
}

/// Everything wrong with the trigger, fork-skip and vendor lists of `vendor-bench.yml`.
fn check_bench_trigger(workflow: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if !workflow.contains("workflow_dispatch:") {
        problems.push("missing the workflow_dispatch trigger".to_string());
    }
    let pr = pull_request_section(workflow);
    let pr_lines: Vec<&str> = pr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if pr_lines != ["types: [labeled]"] {
        problems.push(format!(
            "the pull_request trigger must be exactly `types: [labeled]` (no paths, no other types), found {pr_lines:?}"
        ));
    }
    let guarded = workflow.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("if:")
            && l.contains("github.event.label.name == 'vendor-bench'")
            && l.contains(FORK_SKIP)
    });
    if !guarded {
        problems.push(format!(
            "the job `if:` needs both `github.event.label.name == 'vendor-bench'` and the fork-skip `{FORK_SKIP}`"
        ));
    }
    if !workflow.contains("_LIVE_REQUIRED") {
        problems.push("the bench must set <VENDOR>_LIVE_REQUIRED=1".to_string());
    }
    let options = dispatch_options(workflow);
    let matrix = matrix_vendors(workflow);
    for id in &options {
        if !matrix.contains(id) {
            problems.push(format!(
                "matrix vendor list is missing dispatch option `{id}`"
            ));
        }
    }
    for id in &matrix {
        if !options.contains(id) {
            problems.push(format!("matrix vendor `{id}` is not a dispatch option"));
        }
    }
    problems
}

#[test]
fn vendor_bench_trigger_if_and_vendor_lists_are_in_lockstep() {
    let workflow = read_workflow("vendor-bench.yml").expect("vendor-bench.yml");
    let problems = check_bench_trigger(&workflow);
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
    let expected = [
        "deepgram",
        "elevenlabs",
        "cartesia",
        "assemblyai",
        "google",
        "groq",
        "azure",
        "aws",
        "openai",
        "fishaudio",
    ];
    let options = dispatch_options(&workflow);
    for id in expected {
        assert!(
            options.iter().any(|o| o == id),
            "dispatch options lack {id}"
        );
        assert!(
            repo_root()
                .join("crates")
                .join(format!("vendor-{id}"))
                .is_dir(),
            "no crates/vendor-{id}"
        );
    }
    // Every crate that has a bench is in both lists.
    for id in vendors_with("live_bench.rs") {
        assert!(options.contains(&id), "{id} is not a dispatch option");
        assert!(
            matrix_vendors(&workflow).contains(&id),
            "{id} is not in the matrix"
        );
    }
}

fn good_bench() -> String {
    let all = "deepgram,groq";
    let quoted: Vec<String> = all.split(',').map(|v| format!("\"{v}\"")).collect();
    format!(
        "on:
  pull_request:
    types: [labeled]
  workflow_dispatch:
    inputs:
      vendor:
        type: choice
        options:
          - deepgram
          - groq
      runs:
        default: '5'
jobs:
  bench:
    if: github.event_name == 'workflow_dispatch' || (github.event.label.name == 'vendor-bench' && {FORK_SKIP})
    strategy:
      matrix:
        vendor: ${{{{ fromJSON(github.event_name == 'workflow_dispatch' && format('[\"{{0}}\"]', inputs.vendor) || '[{}]') }}}}
    steps:
      - run: echo \"$VENDOR_LIVE_REQUIRED\"
",
        quoted.join(",")
    )
}

#[test]
fn bench_checker_accepts_a_complete_workflow() {
    assert_eq!(check_bench_trigger(&good_bench()), Vec::<String>::new());
}

#[test]
fn bench_workflow_with_pull_request_paths_fails() {
    let bad = good_bench().replace(
        "    types: [labeled]\n",
        "    types: [labeled]\n    paths:\n      - 'crates/**'\n",
    );
    let problems = check_bench_trigger(&bad);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("no paths"), "{problems:?}");
}

#[test]
fn bench_workflow_without_fork_skip_on_label_path_fails() {
    let bad = good_bench().replace(&format!(" && {FORK_SKIP}"), "");
    let problems = check_bench_trigger(&bad);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("fork-skip"), "{problems:?}");
}

#[test]
fn bench_matrix_missing_a_dispatch_option_fails() {
    let bad = good_bench().replace(",\"groq\"", "");
    let problems = check_bench_trigger(&bad);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("missing dispatch option `groq`"),
        "{problems:?}"
    );
}

// --- negative tests: the checker must reject a broken workflow and say what is missing ---

fn good_workflow(id: &str) -> String {
    format!(
        "on:
  pull_request:
    paths:
      - 'crates/vendor-{id}/**'
      - 'crates/vendor-live-support/**'
      - '.github/workflows/vendor-live-{id}.yml'
  workflow_dispatch:
jobs:
  live:
    if: github.event_name == 'workflow_dispatch' || github.event.pull_request.head.repo.full_name == github.repository
    steps:
      - env:
          X_LIVE_REQUIRED: '1'
        run: |
          cargo test -p node-webrtc-rust-vendor-{id} --features live --test live_test -- --nocapture --test-threads=1
"
    )
}

#[test]
fn checker_accepts_a_complete_workflow() {
    assert_eq!(
        check_live_workflow("x", &good_workflow("x")),
        Vec::<String>::new()
    );
}

#[test]
fn checker_rejects_a_missing_fork_skip() {
    let bad = good_workflow("x").replace(
        " || github.event.pull_request.head.repo.full_name == github.repository",
        "",
    );
    let problems = check_live_workflow("x", &bad);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("fork-skip"), "{problems:?}");
}

#[test]
fn checker_rejects_a_missing_required_flag() {
    let bad = good_workflow("x").replace("X_LIVE_REQUIRED: '1'", "X_LIVE_REQUIRED: '0'");
    let problems = check_live_workflow("x", &bad);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("_LIVE_REQUIRED: '1'"), "{problems:?}");
}

#[test]
fn checker_rejects_a_wrong_path_filter() {
    let bad = good_workflow("x").replace("      - 'crates/vendor-live-support/**'\n", "");
    let problems = check_live_workflow("x", &bad);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("crates/vendor-live-support/**"),
        "{problems:?}"
    );

    // A path that is only mentioned outside the pull_request block does not count.
    let elsewhere = good_workflow("x")
        .replace("      - '.github/workflows/vendor-live-x.yml'\n", "")
        + "# .github/workflows/vendor-live-x.yml\n";
    let problems = check_live_workflow("x", &elsewhere);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("vendor-live-x.yml"), "{problems:?}");
}

#[test]
fn checker_rejects_a_missing_command_or_thread_pin() {
    let bad = good_workflow("x").replace("--test-threads=1", "");
    let problems = check_live_workflow("x", &bad);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("--test-threads=1"), "{problems:?}");

    let wrong_crate = good_workflow("x").replace("vendor-x --features", "vendor-y --features");
    let problems = check_live_workflow("x", &wrong_crate);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("cargo test -p node-webrtc-rust-vendor-x")),
        "{problems:?}"
    );
}

#[test]
fn bench_checker_names_the_vendor_that_is_not_an_option() {
    let workflow = "        options:\n          - deepgram\n          - groq\n";
    assert!(check_bench_workflow("groq", workflow).is_empty());
    let problems = check_bench_workflow("cartesia", workflow);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("cartesia"), "{problems:?}");
}
