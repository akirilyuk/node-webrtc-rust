//! Byte sources for clip decoding (static, in-memory, and progressively growing).

use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use symphonia::core::io::MediaSource;

/// Handle used to append bytes into a [`GrowingByteSource`].
#[derive(Clone)]
pub struct GrowingByteWriter {
    inner: Arc<GrowingByteSourceInner>,
}

impl GrowingByteWriter {
    /// Append more encoded bytes while the decoder is running.
    pub fn append(&self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        let mut guard = self.inner.data.lock().expect("growing source lock");
        guard.extend_from_slice(chunk);
        self.inner.cv.notify_all();
    }

    /// Mark the stream complete (no further bytes will arrive).
    pub fn mark_eof(&self) {
        self.inner.eof.store(true, Ordering::SeqCst);
        self.inner.cv.notify_all();
    }

    /// Returns true after [`Self::mark_eof`].
    pub fn is_eof(&self) -> bool {
        self.inner.eof.load(Ordering::SeqCst)
    }

    /// Total bytes written so far.
    pub fn len(&self) -> usize {
        self.inner.data.lock().expect("growing source lock").len()
    }
}

struct GrowingByteSourceInner {
    data: Mutex<Vec<u8>>,
    read_pos: AtomicUsize,
    eof: AtomicBool,
    cv: Condvar,
}

/// Media source backed by a growable in-memory buffer.
pub struct GrowingByteSource {
    inner: Arc<GrowingByteSourceInner>,
}

impl GrowingByteSource {
    /// Create a source and its writer handle pair.
    pub fn pair() -> (Self, GrowingByteWriter) {
        let inner = Arc::new(GrowingByteSourceInner {
            data: Mutex::new(Vec::new()),
            read_pos: AtomicUsize::new(0),
            eof: AtomicBool::new(false),
            cv: Condvar::new(),
        });
        let writer = GrowingByteWriter {
            inner: Arc::clone(&inner),
        };
        (Self { inner }, writer)
    }

    /// Share an existing buffer for a new decode pass (read position reset).
    pub fn from_shared(shared: &Arc<Self>) -> Self {
        shared.inner.read_pos.store(0, Ordering::SeqCst);
        Self {
            inner: Arc::clone(&shared.inner),
        }
    }

    /// Snapshot of all bytes currently buffered (for MP4 probing).
    pub fn snapshot(&self) -> Vec<u8> {
        self.inner.data.lock().expect("growing source lock").clone()
    }

    pub fn is_eof(&self) -> bool {
        self.inner.eof.load(Ordering::SeqCst)
    }

    pub fn len(&self) -> usize {
        self.inner.data.lock().expect("growing source lock").len()
    }
}

impl Read for GrowingByteSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut guard = self.inner.data.lock().expect("growing source lock");
        let mut pos = self.inner.read_pos.load(Ordering::SeqCst);

        while pos >= guard.len() {
            if self.inner.eof.load(Ordering::SeqCst) {
                return Ok(0);
            }
            // Progressive streams: return 0 when caught up to buffered bytes so symphonia
            // can decode partial input and retry after more `append` calls.
            return Ok(0);
        }

        let available = guard.len() - pos;
        let to_read = available.min(buf.len());
        buf[..to_read].copy_from_slice(&guard[pos..pos + to_read]);
        pos += to_read;
        self.inner.read_pos.store(pos, Ordering::SeqCst);
        Ok(to_read)
    }
}

impl Seek for GrowingByteSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(0) => {
                self.inner.read_pos.store(0, Ordering::SeqCst);
                Ok(0)
            }
            SeekFrom::Current(0) => Ok(self.inner.read_pos.load(Ordering::SeqCst) as u64),
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "growing source only supports seek to start",
            )),
        }
    }
}

impl MediaSource for GrowingByteSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        if self.is_eof() {
            Some(self.len() as u64)
        } else {
            None
        }
    }
}

/// In-memory source for complete byte slices (symphonia `Cursor`).
pub type BytesSource = Cursor<Vec<u8>>;

pub fn bytes_source(data: Vec<u8>) -> BytesSource {
    Cursor::new(data)
}

/// Raw s16le 48 kHz stereo PCM — no container; frames are split directly.
pub fn is_raw_pcm_s16le_48k_stereo(hint_ext: Option<&str>) -> bool {
    matches!(
        hint_ext.map(str::to_ascii_lowercase),
        Some(ext) if ext == "pcm" || ext == "raw" || ext == "s16le"
    )
}
