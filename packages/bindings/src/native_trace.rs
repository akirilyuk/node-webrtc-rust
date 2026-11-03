//! Opt-in native trace log for CI failures (`WNR_LOG_FILE`).
//!
//! Off unless `WNR_LOG_FILE` is set: nothing is registered, no file is opened and the `log`
//! macros in the transport crates stay at their default no-op level.
//!
//! When it is set:
//! - `{pid}` in the path is replaced by the process id, so every vitest worker process writes
//!   its own file.
//! - Filters come from `RUST_LOG` (env_logger syntax). With `RUST_LOG` unset only `warn` is kept.
//! - Only records from the transport crates in [`ALLOWED_TARGET_PREFIXES`] are written. These
//!   log connection metadata (states, flights, candidate pairs). Targets of our own crates
//!   (speech, mixer, vendors, ...) are never written, whatever `RUST_LOG` says, because they can
//!   carry transcripts or text.
//! - The file is bounded: at most two segments of `WNR_LOG_SEGMENT_BYTES` (default 20 MiB). When
//!   the current segment is full it becomes `<path>.1` (replacing the previous one) and a new
//!   one starts, so the newest records always survive.
//!
//! See `development/node-webrtc-rust/features/` (DTLS connect stall) for why this exists.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use log::{Log, Metadata, Record};

const DEFAULT_SEGMENT_BYTES: u64 = 20 * 1024 * 1024;

/// Log targets (module path prefixes) that may be written. Transport crates only.
pub(crate) const ALLOWED_TARGET_PREFIXES: &[&str] = &[
    "dtls",
    "webrtc_dtls",
    "webrtc",
    "webrtc_ice",
    "webrtc_mdns",
    "webrtc_sctp",
    "webrtc_data",
    "webrtc_srtp",
    "webrtc_util",
    "rtp",
    "rtcp",
    "stun",
    "turn",
    "sdp",
];

/// True when `target` equals an allowed prefix or is a child module of one (`webrtc::peer_connection`).
pub(crate) fn target_allowed(target: &str) -> bool {
    ALLOWED_TARGET_PREFIXES
        .iter()
        .any(|p| target == *p || (target.starts_with(p) && target[p.len()..].starts_with("::")))
}

/// Replaces every `{pid}` in `template` with `pid`.
pub(crate) fn expand_path(template: &str, pid: u32) -> PathBuf {
    PathBuf::from(template.replace("{pid}", &pid.to_string()))
}

/// Opens (creating if needed) a segment for appending. Seam so tests can inject open failures.
type SegmentOpener = fn(&Path) -> io::Result<File>;

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// Append-only file capped at two segments (see module docs). A failed rotation never fails a
/// write: the sink keeps writing within `2 * segment_bytes` plus one record, retries the rotation
/// after another segment, and warns once per episode on stderr.
struct RotatingFile {
    path: PathBuf,
    file: File,
    /// Bytes written since the last rotation attempt.
    written: u64,
    /// Length of the current handle's file.
    handle_len: u64,
    segment_bytes: u64,
    open_segment: SegmentOpener,
    /// True after a rename succeeded but the following open failed: `file` is the renamed `.1`
    /// segment, so the retry only needs to open `path`.
    renamed_pending: bool,
    /// True while a rotation failure episode is ongoing (stderr line already printed).
    degraded: bool,
}

impl RotatingFile {
    fn open(path: PathBuf, segment_bytes: u64) -> io::Result<Self> {
        Self::open_with(path, segment_bytes, open_append)
    }

    fn open_with(
        path: PathBuf,
        segment_bytes: u64,
        open_segment: SegmentOpener,
    ) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)?;
            }
        }
        let file = open_segment(&path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path,
            file,
            written,
            handle_len: written,
            segment_bytes,
            open_segment,
            renamed_pending: false,
            degraded: false,
        })
    }

    fn older_path(&self) -> PathBuf {
        let mut older = self.path.clone().into_os_string();
        older.push(".1");
        PathBuf::from(older)
    }

    /// Rotates, or degrades to bounded writing when the rename or reopen fails. Never errors.
    fn rotate(&mut self, incoming: u64) {
        let older = self.older_path();
        if !self.renamed_pending {
            if let Err(e) = fs::rename(&self.path, &older) {
                self.degrade("rename", &e, incoming);
                return;
            }
            self.renamed_pending = true;
        }
        match (self.open_segment)(&self.path) {
            Ok(file) => {
                self.file = file;
                self.written = 0;
                self.handle_len = 0;
                self.renamed_pending = false;
                self.degraded = false;
            }
            Err(e) => self.degrade("open", &e, incoming),
        }
    }

    /// Keeps the current handle, warns once per episode and trims it so the total stays bounded.
    fn degrade(&mut self, op: &str, err: &io::Error, incoming: u64) {
        if !self.degraded {
            self.degraded = true;
            let bound = self.segment_bytes.saturating_mul(2);
            eprintln!(
                "native trace: rotating {} failed ({op}: {err}); still writing, bounded to {bound} bytes",
                self.path.display()
            );
        }
        // While degraded the next attempt is triggered by the handle itself reaching
        // `segment_bytes`, so the handle must have room for the incoming record afterwards.
        if self.handle_len.saturating_add(incoming) > self.segment_bytes
            && self.file.set_len(0).is_ok()
        {
            self.handle_len = 0;
        }
        self.written = 0;
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let incoming = buf.len() as u64;
        // Healthy: rotate after `segment_bytes` written. Degraded: the handle itself is the
        // bound, so a persistent failure retries once per `segment_bytes` and never grows.
        let full = if self.degraded {
            self.handle_len > 0 && self.handle_len + incoming > self.segment_bytes
        } else {
            self.written > 0 && self.written + incoming > self.segment_bytes
        };
        if full {
            self.rotate(incoming);
        }
        let n = self.file.write(buf)?;
        self.written += n as u64;
        self.handle_len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Wraps the env_logger logger and drops every record whose target is not allowed.
struct AllowlistLogger {
    inner: env_logger::Logger,
}

impl Log for AllowlistLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        target_allowed(metadata.target()) && self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if target_allowed(record.target()) {
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Installs the trace logger when `WNR_LOG_FILE` is set and non-empty. Never panics: a trace log
/// that cannot be opened must not take the addon down.
pub(crate) fn install_from_env() {
    let Ok(template) = std::env::var("WNR_LOG_FILE") else {
        return;
    };
    if template.trim().is_empty() {
        return;
    }
    let segment_bytes = std::env::var("WNR_LOG_SEGMENT_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_SEGMENT_BYTES);
    let Ok(file) = RotatingFile::open(expand_path(&template, std::process::id()), segment_bytes)
    else {
        return;
    };
    let inner = env_logger::Builder::new()
        .parse_env("RUST_LOG")
        .target(env_logger::Target::Pipe(Box::new(file)))
        .format_timestamp_micros()
        .build();
    let max_level = inner.filter();
    // `set_boxed_logger` fails when a logger is already installed (second addon load in the
    // same process); keep the first one.
    if log::set_boxed_logger(Box::new(AllowlistLogger { inner })).is_ok() {
        log::set_max_level(max_level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_transport_targets_are_allowed() {
        assert!(target_allowed("dtls"));
        assert!(target_allowed("dtls::conn"));
        assert!(target_allowed("webrtc::peer_connection"));
        assert!(target_allowed("webrtc::mux"));
        assert!(target_allowed("webrtc_ice::agent::agent_internal"));
        assert!(!target_allowed("dtlsx"));
        assert!(!target_allowed("node_webrtc_rust_speech::stt"));
        assert!(!target_allowed("node_webrtc_rust_core"));
        assert!(!target_allowed("reqwest::connect"));
    }

    #[test]
    fn pid_placeholder_expands() {
        assert_eq!(
            expand_path("/t/native-{pid}.log", 42),
            PathBuf::from("/t/native-42.log")
        );
        assert_eq!(
            expand_path("/t/native.log", 42),
            PathBuf::from("/t/native.log")
        );
    }

    #[test]
    fn rotation_keeps_two_bounded_segments() {
        let dir = std::env::temp_dir().join(format!("wnr-trace-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("n.log");
        let mut f = RotatingFile::open(path.clone(), 100).unwrap();
        for i in 0..50u8 {
            f.write_all(format!("line-{i:02}-xxxxxxxxxx\n").as_bytes())
                .unwrap();
        }
        f.flush().unwrap();
        let cur = fs::metadata(&path).unwrap().len();
        let old = fs::metadata(dir.join("n.log.1")).unwrap().len();
        assert!(cur <= 100 && old <= 100, "cur={cur} old={old}");
        let newest = fs::read_to_string(&path).unwrap();
        assert!(newest.contains("line-49"));
        assert!(!dir.join("n.log.2").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    thread_local! {
        /// Opens that should still fail on this test's thread (`usize::MAX` = always).
        static FAIL_OPENS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    fn failing_opener(path: &Path) -> io::Result<File> {
        let fail = FAIL_OPENS.with(|c| {
            let n = c.get();
            if n > 0 && n != usize::MAX {
                c.set(n - 1);
            }
            n > 0
        });
        if fail {
            return Err(io::Error::other("injected open failure"));
        }
        open_append(path)
    }

    fn test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wnr-trace-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn dir_files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn total_bytes(dir: &Path) -> u64 {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum()
    }

    fn on_disk(dir: &Path, needle: &str) -> bool {
        fs::read_dir(dir).unwrap().any(|e| {
            fs::read_to_string(e.unwrap().path())
                .map(|t| t.contains(needle))
                .unwrap_or(false)
        })
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_dir_after_first_segment_keeps_newest_records() {
        use std::os::unix::fs::PermissionsExt;
        let dir = test_dir("ro");
        // Root ignores directory permissions, so the failure cannot be provoked.
        let probe = dir.join("probe");
        fs::write(&probe, b"x").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let root = fs::write(dir.join("probe2"), b"x").is_ok();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_file(&probe);
        let _ = fs::remove_file(dir.join("probe2"));
        if root {
            eprintln!("skipping: running as root, directory permissions are not enforced");
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        let path = dir.join("n.log");
        let mut f = RotatingFile::open(path.clone(), 64).unwrap();
        let mut locked = false;
        for i in 0..100u32 {
            if !locked && dir.join("n.log.1").exists() {
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
                locked = true;
            }
            let r = f.write_all(format!("r{i:03}\n").as_bytes());
            if r.is_err() {
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
            }
            r.unwrap();
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(locked, "first rotation never happened");
        assert!(on_disk(&dir, "r099"));
        let names = dir_files(&dir);
        assert!(
            names.iter().all(|n| n == "n.log" || n == "n.log.1"),
            "{names:?}"
        );
        assert!(
            total_bytes(&dir) <= 2 * 64 + 5,
            "total={}",
            total_bytes(&dir)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopen_failure_after_rename_keeps_writing_and_recovers() {
        let dir = test_dir("reopen");
        let path = dir.join("n.log");
        let mut f = RotatingFile::open_with(path.clone(), 64, failing_opener).unwrap();
        let mut armed = false;
        for i in 0..100u32 {
            if !armed && dir.join("n.log.1").exists() {
                FAIL_OPENS.with(|c| c.set(3));
                armed = true;
            }
            f.write_all(format!("r{i:03}\n").as_bytes()).unwrap();
            let names = dir_files(&dir);
            assert!(
                names.iter().all(|n| n == "n.log" || n == "n.log.1"),
                "{names:?}"
            );
            assert!(
                total_bytes(&dir) <= 2 * 64 + 5,
                "total={}",
                total_bytes(&dir)
            );
        }
        assert!(armed);
        assert!(on_disk(&dir, "r099"));
        assert_eq!(FAIL_OPENS.with(|c| c.get()), 0, "failures not consumed");
        assert!(path.exists(), "rotation did not recover");
        assert!(fs::read_to_string(&path).unwrap().contains("r099"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn persistent_reopen_failure_stays_bounded() {
        let dir = test_dir("persist");
        let path = dir.join("n.log");
        let mut f = RotatingFile::open_with(path, 64, failing_opener).unwrap();
        let mut armed = false;
        for i in 0..1000u32 {
            if !armed && dir.join("n.log.1").exists() {
                FAIL_OPENS.with(|c| c.set(usize::MAX));
                armed = true;
            }
            f.write_all(format!("r{i:03}\n").as_bytes()).unwrap();
            assert!(
                total_bytes(&dir) <= 2 * 64 + 5,
                "total={}",
                total_bytes(&dir)
            );
        }
        assert!(armed);
        assert!(on_disk(&dir, "r999"));
        assert!(dir_files(&dir).len() <= 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
