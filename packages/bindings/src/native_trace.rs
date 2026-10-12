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
use std::path::PathBuf;

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

/// Append-only file capped at two segments (see module docs).
struct RotatingFile {
    path: PathBuf,
    file: File,
    written: u64,
    segment_bytes: u64,
}

impl RotatingFile {
    fn open(path: PathBuf, segment_bytes: u64) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)?;
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path,
            file,
            written,
            segment_bytes,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        let mut older = self.path.clone().into_os_string();
        older.push(".1");
        fs::rename(&self.path, PathBuf::from(older))?;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.written = 0;
        Ok(())
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written > 0 && self.written + buf.len() as u64 > self.segment_bytes {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.written += n as u64;
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
}
