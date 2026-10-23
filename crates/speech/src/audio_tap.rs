//! Test harness audio tap for Sherpa roundtrip failures.
//!
//! Enabled by the env var `SPEECH_TEST_AUDIO_TAP_DIR`. It writes raw audio to disk, so it must
//! never be set in production services. Off when the variable is unset or empty.
//!
//! Two taps exist:
//! - [`ContinuousTap`]: the listener's inbound mono 16 kHz PCM right after it arrives, written
//!   as `inbound-<pid>-a<agent>-seg<NNNN>.wav` (a segment per VAD speech end, or every 10 s).
//! - [`UtteranceTap`]: the PCM the STT actor pushes to the provider, written as
//!   `stt-in-<pid>-a<agent>-utt<key>.wav` (one file per utterance).
//!
//! Files are written on a short-lived thread so the audio path never waits for the disk.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

use crate::session_recorder::encode_pcm16le_wav;

/// Env var naming the directory the tap writes to.
pub const AUDIO_TAP_DIR_ENV: &str = "SPEECH_TEST_AUDIO_TAP_DIR";

/// Most files one process writes; further writes are skipped.
const MAX_TAP_FILES_PER_PROCESS: u64 = 2000;
/// Longest utterance kept by an [`UtteranceTap`] (60 s mono 16 kHz s16le).
const UTTERANCE_TAP_MAX_BYTES: usize = 60 * 16_000 * 2;
/// Segment size at which a [`ContinuousTap`] rotates (10 s mono 16 kHz s16le).
const CONTINUOUS_TAP_SEGMENT_BYTES: usize = 10 * 16_000 * 2;

static FILES_WRITTEN: AtomicU64 = AtomicU64::new(0);
static LIMIT_LOGGED: AtomicBool = AtomicBool::new(false);
static NEXT_AGENT_ID: AtomicU64 = AtomicU64::new(1);

/// `SPEECH_TEST_AUDIO_TAP_DIR` (test-only). Read once; the directory is created.
pub fn audio_tap_dir_from_env() -> Option<&'static Path> {
    static VALUE: OnceLock<Option<&'static Path>> = OnceLock::new();
    *VALUE.get_or_init(|| {
        let raw = std::env::var(AUDIO_TAP_DIR_ENV).ok()?;
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let dir = PathBuf::from(raw);
        if let Err(err) = std::fs::create_dir_all(&dir) {
            eprintln!(
                "[speech] audio tap disabled: cannot create {}: {err}",
                dir.display()
            );
            return None;
        }
        let leaked: &'static PathBuf = Box::leak(Box::new(dir));
        Some(leaked.as_path())
    })
}

/// Next agent id for file names (1, 2, ...), unique within the process.
pub fn next_tap_agent_id() -> u64 {
    NEXT_AGENT_ID.fetch_add(1, Ordering::SeqCst)
}

/// RIFF/WAVE (PCM, 16 bit) bytes for s16le `pcm_s16le`.
pub fn wav_bytes(pcm_s16le: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    encode_pcm16le_wav(pcm_s16le, sample_rate, channels)
}

/// Writes `bytes` to `path` on a new thread. Never blocks the caller.
fn write_file_async(path: PathBuf, bytes: Vec<u8>) {
    if FILES_WRITTEN.fetch_add(1, Ordering::SeqCst) >= MAX_TAP_FILES_PER_PROCESS {
        if !LIMIT_LOGGED.swap(true, Ordering::SeqCst) {
            eprintln!(
                "[speech] audio tap file limit reached ({MAX_TAP_FILES_PER_PROCESS}); further writes skipped"
            );
        }
        return;
    }
    std::thread::spawn(move || {
        if let Err(err) = std::fs::write(&path, bytes) {
            eprintln!("[speech] audio tap write failed: {}: {err}", path.display());
        }
    });
}

/// Collects the PCM of one utterance and writes it as one WAV on [`UtteranceTap::flush`].
pub struct UtteranceTap {
    dir: &'static Path,
    agent_id: u64,
    kind: &'static str,
    sample_rate: u32,
    channels: u16,
    buf: Vec<u8>,
}

impl UtteranceTap {
    pub fn new(
        dir: &'static Path,
        agent_id: u64,
        kind: &'static str,
        sample_rate: u32,
        channels: u16,
    ) -> Self {
        Self {
            dir,
            agent_id,
            kind,
            sample_rate,
            channels,
            buf: Vec::new(),
        }
    }

    /// Appends `pcm`, up to [`UTTERANCE_TAP_MAX_BYTES`] per utterance; the rest is ignored.
    pub fn push(&mut self, pcm: &[u8]) {
        let room = UTTERANCE_TAP_MAX_BYTES.saturating_sub(self.buf.len());
        let take = room.min(pcm.len());
        self.buf.extend_from_slice(&pcm[..take]);
    }

    /// Writes the collected PCM as `<kind>-<pid>-a<agent>-<label>.wav` and clears it.
    /// Nothing is written when no audio was collected.
    pub fn flush(&mut self, label: &str) {
        if self.buf.is_empty() {
            return;
        }
        let name = format!(
            "{}-{}-a{}-{}.wav",
            self.kind,
            std::process::id(),
            self.agent_id,
            label
        );
        let bytes = wav_bytes(&self.buf, self.sample_rate, self.channels);
        self.buf.clear();
        write_file_async(self.dir.join(name), bytes);
    }
}

/// Continuous mono 16 kHz s16le capture, written in segments.
pub struct ContinuousTap {
    dir: &'static Path,
    agent_id: u64,
    buf: Vec<u8>,
    seg: u32,
}

impl ContinuousTap {
    pub fn new(dir: &'static Path, agent_id: u64) -> Self {
        Self {
            dir,
            agent_id,
            buf: Vec::new(),
            seg: 0,
        }
    }

    /// Appends `mono16k`; rotates to a new file once a segment is full.
    pub fn push(&mut self, mono16k: &[u8]) {
        self.buf.extend_from_slice(mono16k);
        if self.buf.len() >= CONTINUOUS_TAP_SEGMENT_BYTES {
            self.flush_segment();
        }
    }

    /// Writes the collected audio as `inbound-<pid>-a<agent>-seg<NNNN>.wav` and starts the next
    /// segment. Nothing is written when no audio was collected.
    pub fn flush_segment(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let name = format!(
            "inbound-{}-a{}-seg{:04}.wav",
            std::process::id(),
            self.agent_id,
            self.seg
        );
        let bytes = wav_bytes(&self.buf, 16_000, 1);
        self.buf.clear();
        self.seg += 1;
        write_file_async(self.dir.join(name), bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn unique_dir(label: &str) -> &'static Path {
        let dir = std::env::temp_dir().join(format!(
            "audio-tap-unit-{label}-{}-{}",
            std::process::id(),
            next_tap_agent_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Box::leak(Box::new(dir)).as_path()
    }

    fn wav_files(dir: &Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        files.sort();
        files
    }

    #[test]
    fn wav_header_fields() {
        let wav = wav_bytes(&[0u8; 640], 16000, 1);
        assert_eq!(wav.len(), 684);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1);
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            16000
        );
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(
            u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]),
            640
        );
    }

    #[test]
    fn utterance_tap_empty_flush_writes_nothing() {
        let dir = unique_dir("empty");
        let mut tap = UtteranceTap::new(dir, 1, "stt-in", 16_000, 1);
        tap.flush("utt1");
        std::thread::sleep(Duration::from_millis(200));
        assert!(wav_files(dir).is_empty());
    }

    #[test]
    fn utterance_tap_caps_at_max_bytes() {
        let dir = unique_dir("cap");
        let mut tap = UtteranceTap::new(dir, 1, "stt-in", 16_000, 1);
        tap.push(&vec![1u8; UTTERANCE_TAP_MAX_BYTES + 1000]);
        assert_eq!(tap.buf.len(), UTTERANCE_TAP_MAX_BYTES);
        tap.push(&[1u8; 640]);
        assert_eq!(tap.buf.len(), UTTERANCE_TAP_MAX_BYTES);
    }

    #[test]
    fn continuous_tap_rotates_at_segment_size() {
        let dir = unique_dir("rotate");
        let mut tap = ContinuousTap::new(dir, 3);
        let chunk = [0u8; 640];
        let pushes = (CONTINUOUS_TAP_SEGMENT_BYTES + 640) / 640;
        for _ in 0..pushes {
            tap.push(&chunk);
        }
        assert_eq!(tap.buf.len(), 640);
        let deadline = Instant::now() + Duration::from_secs(2);
        let files = loop {
            let files = wav_files(dir);
            let complete = files.first().is_some_and(|f| {
                std::fs::metadata(f).map(|m| m.len()).unwrap_or(0)
                    == (44 + CONTINUOUS_TAP_SEGMENT_BYTES) as u64
            });
            if complete || Instant::now() > deadline {
                break files;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(files.len(), 1, "{files:?}");
        let name = files[0].file_name().unwrap().to_string_lossy().to_string();
        assert!(name.ends_with("-seg0000.wav"), "{name}");
        let bytes = std::fs::read(&files[0]).unwrap();
        let data_len = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as usize;
        assert_eq!(data_len, CONTINUOUS_TAP_SEGMENT_BYTES);
        assert_eq!(bytes.len(), 44 + CONTINUOUS_TAP_SEGMENT_BYTES);
    }
}
