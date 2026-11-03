//! Guard for the error class `rt.serial-queue-head-of-line`: the TTS send path never locks a
//! provider or vendor client.
//!
//! `VoiceAgent::send_text_to_tts*` only queues a job; the synthesis worker owns the TTS provider.
//! When `send_text_to_tts_reserved` once locked `self.tts`, every send waited behind the lock the
//! worker holds for a whole synthesis. This test reads `src/agent.rs` and fails when a function on
//! the send path locks a `VoiceAgent` field that holds a provider or vendor client.
//!
//! What is scanned:
//! - Roots: every `fn send_text_to_tts*` in `src/agent.rs`.
//! - Closure: every function in `agent.rs` those roots call as `self.<fn>(` or `Self::<fn>(`,
//!   transitively.
//! - Code inside `spawn(` / `spawn_blocking(` arguments is not on the send path (it runs on the
//!   worker). It is excluded from the call closure and from the lock scan.
//! - Forbidden fields: `VoiceAgent` struct fields whose type contains `Provider`, `Client` or a
//!   name in [`EXTRA_FORBIDDEN_TYPES`].
//! - A finding is `.lock(`, `.read(`, `.write(` or `.try_lock(` on `self.<forbidden field>`, or on
//!   a local assigned from `self.<field>`, `self.<field>.clone()` or `Arc::clone(&self.<field>)`
//!   in the same body.
//!
//! Limits (text scan): a lock reached through a free function, another module, a parameter alias
//! (`fn helper(a: &Arc<...>)`) or a macro is not seen. Only `VoiceAgent` methods in `agent.rs`
//! are followed. That covers the whole send path today.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// Extra type names (besides `Provider` / `Client`) that mark a forbidden field.
const EXTRA_FORBIDDEN_TYPES: &[&str] = &["SpeakerEmbedder"];

/// Methods that take a lock on a field.
const LOCK_METHODS: &[&str] = &["lock", "read", "write", "try_lock"];

/// `(fn name, field, reason)`. Each entry needs a reason and must match a finding.
const ALLOWLIST: &[(&str, &str, &str)] = &[];

#[derive(Debug, PartialEq, Eq)]
struct Finding {
    func: String,
    field: String,
    line: usize,
}

#[derive(Debug)]
struct Scan {
    roots: Vec<String>,
    closure: BTreeSet<String>,
    findings: Vec<Finding>,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn skip_ws(c: &[char], mut i: usize) -> usize {
    while i < c.len() && c[i].is_whitespace() {
        i += 1;
    }
    i
}

fn read_ident(c: &[char], i: usize) -> (String, usize) {
    let mut j = i;
    while j < c.len() && is_ident(c[j]) {
        j += 1;
    }
    (c[i..j].iter().collect(), j)
}

fn line_of(c: &[char], pos: usize) -> usize {
    c[..pos].iter().filter(|ch| **ch == '\n').count() + 1
}

fn blank_range(c: &mut [char], from: usize, to: usize) {
    for ch in &mut c[from..to] {
        if *ch != '\n' {
            *ch = ' ';
        }
    }
}

/// Source with comments and the contents of string / char literals replaced by spaces
/// (newlines kept, so line numbers hold).
fn blank_comments_and_literals(src: &str) -> Vec<char> {
    let mut c: Vec<char> = src.chars().collect();
    let n = c.len();
    let mut i = 0;
    while i < n {
        let ch = c[i];
        if ch == '/' && i + 1 < n && c[i + 1] == '/' {
            let mut j = i;
            while j < n && c[j] != '\n' {
                j += 1;
            }
            blank_range(&mut c, i, j);
            i = j;
        } else if ch == '/' && i + 1 < n && c[i + 1] == '*' {
            let mut depth = 0;
            let mut j = i;
            while j < n {
                if c[j] == '/' && j + 1 < n && c[j + 1] == '*' {
                    depth += 1;
                    j += 2;
                } else if c[j] == '*' && j + 1 < n && c[j + 1] == '/' {
                    depth -= 1;
                    j += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            blank_range(&mut c, i, j);
            i = j;
        } else if ch == 'r' && (i == 0 || !is_ident(c[i - 1])) && raw_string_start(&c, i).is_some()
        {
            let (hashes, body) = raw_string_start(&c, i).unwrap();
            let mut j = body;
            while j < n {
                if c[j] == '"' && (0..hashes).all(|k| j + 1 + k < n && c[j + 1 + k] == '#') {
                    break;
                }
                j += 1;
            }
            let end = (j + 1 + hashes).min(n);
            blank_range(&mut c, body, j.min(n));
            i = end;
        } else if ch == '"' {
            let mut j = i + 1;
            while j < n && c[j] != '"' {
                j += if c[j] == '\\' { 2 } else { 1 };
            }
            let j = j.min(n);
            blank_range(&mut c, i + 1, j);
            i = j + 1;
        } else if ch == '\'' {
            // Char literal or lifetime.
            if i + 1 < n && c[i + 1] == '\\' {
                let mut j = i + 2;
                while j < n && c[j] != '\'' {
                    j += 1;
                }
                let j = j.min(n);
                blank_range(&mut c, i + 1, j);
                i = j + 1;
            } else if i + 2 < n && c[i + 2] == '\'' {
                blank_range(&mut c, i + 1, i + 2);
                i += 3;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    c
}

/// `r"`, `r#"`, ...: returns (hash count, index after the opening quote).
fn raw_string_start(c: &[char], i: usize) -> Option<(usize, usize)> {
    let mut j = i + 1;
    let mut hashes = 0;
    while j < c.len() && c[j] == '#' {
        hashes += 1;
        j += 1;
    }
    (j < c.len() && c[j] == '"').then_some((hashes, j + 1))
}

/// Index of the delimiter that closes the one at `open` (`{`/`(`), or the end of input.
fn matching(c: &[char], open: usize) -> usize {
    let (o, cl) = if c[open] == '{' {
        ('{', '}')
    } else {
        ('(', ')')
    };
    let mut depth = 0;
    for (k, ch) in c.iter().enumerate().skip(open) {
        if *ch == o {
            depth += 1;
        } else if *ch == cl {
            depth -= 1;
            if depth == 0 {
                return k;
            }
        }
    }
    c.len() - 1
}

/// Blanks the argument list of every `spawn(` / `spawn_blocking(` call: that code runs on the
/// worker, not on the caller's path.
fn blank_spawned_code(c: &mut [char]) {
    let mut i = 0;
    while i < c.len() {
        if is_ident(c[i]) && (i == 0 || !is_ident(c[i - 1])) {
            let (id, end) = read_ident(c, i);
            if id == "spawn" || id == "spawn_blocking" {
                let open = skip_ws(c, end);
                let mut p = i;
                while p > 0 && c[p - 1].is_whitespace() {
                    p -= 1;
                }
                let is_def = p >= 2 && c[p - 2] == 'f' && c[p - 1] == 'n';
                if !is_def && open < c.len() && c[open] == '(' {
                    let close = matching(c, open);
                    blank_range(c, open + 1, close);
                    i = close;
                    continue;
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
}

struct FnDef {
    name: String,
    body: (usize, usize),
}

fn find_fns(c: &[char]) -> Vec<FnDef> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < c.len() {
        if c[i] == 'f'
            && c[i + 1] == 'n'
            && c[i + 2].is_whitespace()
            && (i == 0 || !is_ident(c[i - 1]))
        {
            let (name, mut j) = read_ident(c, skip_ws(c, i + 3));
            if !name.is_empty() {
                let mut depth = 0i32;
                while j < c.len() {
                    match c[j] {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        '{' | ';' if depth == 0 => break,
                        _ => {}
                    }
                    j += 1;
                }
                if j < c.len() && c[j] == '{' {
                    out.push(FnDef {
                        name,
                        body: (j + 1, matching(c, j)),
                    });
                }
            }
        }
        i += 1;
    }
    out
}

/// Names of `VoiceAgent` fields whose type holds a provider or vendor client.
fn forbidden_fields(source: &str) -> Vec<String> {
    let c = blank_comments_and_literals(source);
    let text: String = c.iter().collect();
    let start = text
        .find("struct VoiceAgent {")
        .expect("struct VoiceAgent in source");
    let open = start + text[start..].find('{').unwrap();
    let open_c = text[..open].chars().count();
    let close_c = matching(&c, open_c);
    let mut body: Vec<char> = c[open_c + 1..close_c].to_vec();
    // Drop attributes.
    let mut k = 0;
    while k < body.len() {
        if body[k] == '#' && k + 1 < body.len() && body[k + 1] == '[' {
            let mut depth = 0;
            let mut m = k + 1;
            while m < body.len() {
                if body[m] == '[' {
                    depth += 1;
                } else if body[m] == ']' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                m += 1;
            }
            let last = m.min(body.len() - 1);
            for ch in &mut body[k..=last] {
                *ch = ' ';
            }
        }
        k += 1;
    }
    let mut fields = Vec::new();
    let mut depth = 0i32;
    let mut piece = String::new();
    let mut pieces = Vec::new();
    for ch in body {
        match ch {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            _ => {}
        }
        if ch == ',' && depth == 0 {
            pieces.push(std::mem::take(&mut piece));
        } else {
            piece.push(ch);
        }
    }
    pieces.push(piece);
    for p in pieces {
        let p = p.trim();
        let Some(colon) = p.find(':') else { continue };
        let name = p[..colon]
            .trim()
            .rsplit(' ')
            .next()
            .unwrap_or("")
            .to_string();
        let ty = &p[colon + 1..];
        let hit = ty.contains("Provider")
            || ty.contains("Client")
            || EXTRA_FORBIDDEN_TYPES.iter().any(|t| ty.contains(t));
        if hit && !name.is_empty() {
            fields.push(name);
        }
    }
    fields
}

/// Previous non-whitespace char before `i`.
fn prev_non_ws(c: &[char], i: usize) -> Option<char> {
    c[..i].iter().rev().find(|ch| !ch.is_whitespace()).copied()
}

/// Aliases (`let a = self.f` / `self.f.clone()` / `Arc::clone(&self.f)`) in `body`.
fn aliases(c: &[char], body: (usize, usize), forbidden: &[String]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut i = body.0;
    while i < body.1 {
        if is_ident(c[i]) && (i == 0 || !is_ident(c[i - 1])) {
            let (id, end) = read_ident(c, i);
            if id == "let" {
                let mut j = skip_ws(c, end);
                let (mut name, mut k) = read_ident(c, j);
                if name == "mut" {
                    j = skip_ws(c, k);
                    (name, k) = read_ident(c, j);
                }
                while k < body.1 && c[k] != '=' && c[k] != ';' {
                    k += 1;
                }
                if k < body.1 && c[k] == '=' {
                    let mut m = k + 1;
                    let mut rhs = String::new();
                    while m < body.1 && c[m] != ';' {
                        if !c[m].is_whitespace() {
                            rhs.push(c[m]);
                        }
                        m += 1;
                    }
                    for f in forbidden {
                        if rhs == format!("Arc::clone(&self.{f})")
                            || rhs == format!("self.{f}.clone()")
                            || rhs == format!("self.{f}")
                        {
                            out.insert(name.clone(), f.clone());
                        }
                    }
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

fn scan(source: &str, forbidden: &[String]) -> Scan {
    let mut c = blank_comments_and_literals(source);
    blank_spawned_code(&mut c);
    let fns = find_fns(&c);
    let defined: BTreeSet<&str> = fns.iter().map(|f| f.name.as_str()).collect();

    let roots: Vec<String> = fns
        .iter()
        .filter(|f| f.name.starts_with("send_text_to_tts"))
        .map(|f| f.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    // Calls and lock sites per function.
    let mut calls: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut locks: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    for f in &fns {
        let alias = aliases(&c, f.body, forbidden);
        let mut i = f.body.0;
        while i < f.body.1 {
            if !(is_ident(c[i]) && (i == 0 || !is_ident(c[i - 1]))) {
                i += 1;
                continue;
            }
            let (id, end) = read_ident(&c, i);
            let mut j = skip_ws(&c, end);
            let prev = prev_non_ws(&c, i);
            if id == "Self" && c.get(j) == Some(&':') && c.get(j + 1) == Some(&':') {
                let (name, k) = read_ident(&c, skip_ws(&c, j + 2));
                if c.get(skip_ws(&c, k)) == Some(&'(') && defined.contains(name.as_str()) {
                    calls.entry(f.name.clone()).or_default().insert(name);
                }
            } else if id == "self" && prev != Some('.') && c.get(j) == Some(&'.') {
                j = skip_ws(&c, j + 1);
                let (name, k) = read_ident(&c, j);
                let k = skip_ws(&c, k);
                if c.get(k) == Some(&'(') {
                    if defined.contains(name.as_str()) {
                        calls.entry(f.name.clone()).or_default().insert(name);
                    }
                } else if c.get(k) == Some(&'.') && forbidden.contains(&name) {
                    record_lock(&c, k, &name, &f.name, &mut locks);
                }
            } else if let Some(field) = alias.get(&id) {
                if prev != Some('.') && c.get(j) == Some(&'.') {
                    record_lock(&c, j, field, &f.name, &mut locks);
                }
            }
            i = end;
        }
    }

    // Closure over calls.
    let mut closure: BTreeSet<String> = roots.iter().cloned().collect();
    let mut stack: Vec<String> = roots.clone();
    while let Some(name) = stack.pop() {
        for callee in calls.get(&name).into_iter().flatten() {
            if closure.insert(callee.clone()) {
                stack.push(callee.clone());
            }
        }
    }

    let mut findings = Vec::new();
    for name in &closure {
        for (field, line) in locks.get(name).into_iter().flatten() {
            let finding = Finding {
                func: name.clone(),
                field: field.clone(),
                line: *line,
            };
            if !findings.contains(&finding) {
                findings.push(finding);
            }
        }
    }
    Scan {
        roots,
        closure,
        findings,
    }
}

/// `dot` is the index of the `.` after the field (or alias); records `.lock(` and friends.
fn record_lock(
    c: &[char],
    dot: usize,
    field: &str,
    func: &str,
    locks: &mut BTreeMap<String, Vec<(String, usize)>>,
) {
    let m = skip_ws(c, dot + 1);
    let (method, k) = read_ident(c, m);
    if LOCK_METHODS.contains(&method.as_str()) && c.get(skip_ws(c, k)) == Some(&'(') {
        locks
            .entry(func.to_string())
            .or_default()
            .push((field.to_string(), line_of(c, m)));
    }
}

/// Findings not covered by `allowlist`, and the allowlist entries that cover nothing (or have no
/// reason).
fn apply_allowlist(
    findings: Vec<Finding>,
    allowlist: &[(&str, &str, &str)],
) -> (Vec<Finding>, Vec<String>) {
    let mut stale = Vec::new();
    for (func, field, reason) in allowlist {
        if reason.trim().is_empty() {
            stale.push(format!("{func}/{field}: allowlist entry has no reason"));
        }
        if !findings
            .iter()
            .any(|f| f.func == *func && f.field == *field)
        {
            stale.push(format!(
                "{func}/{field}: stale allowlist entry (matches nothing)"
            ));
        }
    }
    let remaining = findings
        .into_iter()
        .filter(|f| {
            !allowlist
                .iter()
                .any(|(func, field, _)| f.func == *func && f.field == *field)
        })
        .collect();
    (remaining, stale)
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn tts_send_path_never_locks_a_provider() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/agent.rs");
    let source = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));

    let forbidden = forbidden_fields(&source);
    for expected in ["tts", "stt_provider", "language_id", "speaker_embedder"] {
        assert!(
            forbidden.iter().any(|f| f == expected),
            "forbidden fields must include `{expected}`, got {forbidden:?}"
        );
    }

    let result = scan(&source, &forbidden);
    for root in [
        "send_text_to_tts",
        "send_text_to_tts_with_options",
        "send_text_to_tts_reserved",
    ] {
        assert!(
            result.roots.iter().any(|r| r == root),
            "scan must find root `{root}`, got {:?}",
            result.roots
        );
    }
    for callee in ["ensure_tts_synthesis_worker", "reserve_tts_send_for_reply"] {
        assert!(
            result.closure.contains(callee),
            "send path closure must include `{callee}`, got {:?}",
            result.closure
        );
    }

    let (remaining, stale) = apply_allowlist(result.findings, ALLOWLIST);
    let report: Vec<String> = remaining
        .iter()
        .map(|f| format!("src/agent.rs:{}: {} locks self.{}", f.line, f.func, f.field))
        .collect();
    assert!(
        remaining.is_empty(),
        "the TTS send path locks a provider or vendor client \
         (rt.serial-queue-head-of-line: the synthesis worker owns it for a whole synthesis, \
         so every send would wait behind it):\n{}",
        report.join("\n")
    );
    assert!(
        stale.is_empty(),
        "allowlist problems:\n{}",
        stale.join("\n")
    );
}

#[test]
fn scan_flags_a_lock_on_the_send_path() {
    let tts = strings(&["tts"]);

    // Direct lock in a root.
    let direct =
        "pub async fn send_text_to_tts_reserved(&self) {\n    let _g = self.tts.lock().await;\n}\n";
    assert_eq!(
        scan(direct, &tts).findings,
        vec![Finding {
            func: "send_text_to_tts_reserved".into(),
            field: "tts".into(),
            line: 2
        }]
    );

    // Lock split over lines, and try_lock.
    let split =
        "fn send_text_to_tts(&self) {\n    let _g = self\n        .tts\n        .try_lock();\n}\n";
    assert_eq!(scan(split, &tts).findings.len(), 1);

    // Lock in a helper reached by `self.helper()`.
    let via_method = "fn send_text_to_tts(&self) { self.helper().await; }\n\
                      async fn helper(&self) {\n    self.tts.lock().await;\n}\n";
    assert_eq!(
        scan(via_method, &tts).findings,
        vec![Finding {
            func: "helper".into(),
            field: "tts".into(),
            line: 3
        }]
    );

    // Lock in an associated fn reached by `Self::helper(..)` (limit: reached through `&self`).
    let via_assoc = "fn send_text_to_tts(&self) { Self::helper(self).await; }\n\
                     async fn helper(&self) {\n    self.tts.lock().await;\n}\n";
    let found = scan(via_assoc, &tts);
    assert!(found.closure.contains("helper"));
    assert_eq!(found.findings.len(), 1);
    assert_eq!(found.findings[0].func, "helper");

    // Lock through a local alias.
    let alias = "fn send_text_to_tts(&self) {\n    let t = Arc::clone(&self.tts);\n    t.lock().await;\n}\n";
    let found = scan(alias, &tts);
    assert_eq!(found.findings.len(), 1);
    assert_eq!(found.findings[0].line, 3);

    // An alias moved into a spawned task is the worker's.
    let spawned = "fn send_text_to_tts(&self) {\n    let t = Arc::clone(&self.tts);\n    tokio::spawn(async move {\n        let _g = t.lock().await;\n        self.helper();\n    });\n}\n\
                   fn helper(&self) { self.tts.lock(); }\n";
    let found = scan(spawned, &tts);
    assert!(found.findings.is_empty(), "{:?}", found.findings);
    assert!(!found.closure.contains("helper"));

    // Comments and strings are not code.
    let quiet = "fn send_text_to_tts(&self) {\n    // self.tts.lock().await\n    /* self.tts.lock() */\n    let _s = \"self.tts.lock()\";\n    let _c = '\"';\n    self.queue.lock().await;\n}\n";
    assert!(scan(quiet, &tts).findings.is_empty());

    // A function the send path does not call is not scanned.
    let unrelated = "fn send_text_to_tts(&self) {}\nfn worker(&self) { self.tts.lock(); }\n";
    assert!(scan(unrelated, &tts).findings.is_empty());
}

#[test]
fn forbidden_fields_come_from_the_struct_types() {
    let snippet = "pub struct VoiceAgent {\n    inner: Arc<Mutex<AgentInner>>,\n    /// doc\n    #[allow(dead_code)]\n    stt_provider: Mutex<Option<Box<dyn SttProvider>>>,\n    tts: Arc<Mutex<Option<Box<dyn TtsProvider>>>>,\n    speaker_embedder: Mutex<Option<Box<dyn SpeakerEmbedder>>>,\n    http: HttpClient,\n    tts_synthesis_queue: Arc<Mutex<VecDeque<Job>>>,\n}\n";
    assert_eq!(
        forbidden_fields(snippet),
        vec!["stt_provider", "tts", "speaker_embedder", "http"]
    );
}

#[test]
fn scan_rejects_stale_allowlist_entries() {
    let snippet = "fn send_text_to_tts(&self) {\n    self.queue.lock().await;\n}\n";
    let found = scan(snippet, &strings(&["tts"]));
    let (remaining, stale) = apply_allowlist(
        found.findings,
        &[("send_text_to_tts", "tts", "worker hand-off")],
    );
    assert!(remaining.is_empty());
    assert_eq!(stale.len(), 1, "{stale:?}");
    assert!(stale[0].contains("stale"));

    // An entry without a reason is rejected, and one that matches is applied.
    let locked = scan(
        "fn send_text_to_tts(&self) { self.tts.lock(); }\n",
        &strings(&["tts"]),
    );
    let (remaining, stale) = apply_allowlist(locked.findings, &[("send_text_to_tts", "tts", " ")]);
    assert!(remaining.is_empty());
    assert_eq!(stale.len(), 1);
    assert!(stale[0].contains("no reason"));
}
