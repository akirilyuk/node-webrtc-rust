//! Per-clip decode and playback session.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use bytes::Bytes;
use node_webrtc_rust_mixer::{Frame, FRAME_BYTES, FRAME_MS};
use symphonia::core::io::MediaSource;

use crate::decoder::{hint_from_bytes, hint_from_path, DecoderSession};
use crate::error::PlayerError;
use crate::mp4_probe::{mp4_ready_for_decode, Mp4Layout, probe_mp4_layout};
use crate::resample::stereo_byte_len_to_ms;
use crate::source::{bytes_source, is_raw_pcm_s16le_48k_stereo, GrowingByteSource, GrowingByteWriter};
use crate::types::{ClipPlayerStatus, ClipStatus, PREROLL_MIN_MS, PREROLL_TARGET_MS};

enum InputKind {
    Path(String),
    Bytes(Vec<u8>),
    Growing(Arc<GrowingByteSource>),
    RawPcm(Vec<u8>),
}

pub struct ClipSession {
    play_id: String,
    state: Arc<SessionState>,
    decode_thread: Option<JoinHandle<()>>,
}

struct SessionState {
    status: Mutex<ClipPlayerStatus>,
    pcm_queue: Mutex<Vec<Bytes>>,
    stop_requested: AtomicBool,
    decode_done: AtomicBool,
    preroll_ms: u64,
}

impl ClipSession {
    pub fn start_from_path(play_id: String, path: impl AsRef<Path>) -> Result<Self, PlayerError> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let ext = path
            .as_ref()
            .extension()
            .map(|e| e.to_string_lossy().to_string());
        if is_raw_pcm_s16le_48k_stereo(ext.as_deref()) {
            let data = std::fs::read(path.as_ref())
                .map_err(|e| PlayerError::Io(e.to_string()))?;
            return Ok(Self::spawn(play_id, InputKind::RawPcm(data)));
        }
        Ok(Self::spawn(play_id, InputKind::Path(path_str)))
    }

    pub fn start_from_bytes(play_id: String, data: Vec<u8>) -> Result<Self, PlayerError> {
        Ok(Self::spawn(play_id, InputKind::Bytes(data)))
    }

    pub fn start_from_growing(play_id: String) -> (Self, GrowingByteWriter) {
        let (source, writer) = GrowingByteSource::pair();
        let source = Arc::new(source);
        let session = Self::spawn(play_id, InputKind::Growing(source));
        (session, writer)
    }

    fn spawn(play_id: String, input: InputKind) -> Self {
        let duration_ms = match &input {
            InputKind::RawPcm(data) => Some(stereo_byte_len_to_ms(data.len())),
            _ => None,
        };

        let state = Arc::new(SessionState {
            status: Mutex::new(ClipPlayerStatus {
                play_id: play_id.clone(),
                status: ClipStatus::Buffering,
                position_ms: 0,
                duration_ms,
                buffered_ms: 0,
                error: None,
            }),
            pcm_queue: Mutex::new(Vec::new()),
            stop_requested: AtomicBool::new(false),
            decode_done: AtomicBool::new(false),
            preroll_ms: PREROLL_TARGET_MS,
        });

        let decode_thread = {
            let state = Arc::clone(&state);
            let play_id = play_id.clone();
            thread::spawn(move || decode_loop(play_id, input, state))
        };

        Self {
            play_id,
            state,
            decode_thread: Some(decode_thread),
        }
    }

    pub fn play_id(&self) -> &str {
        &self.play_id
    }

    pub fn status(&self) -> ClipPlayerStatus {
        self.state.status.lock().expect("status lock").clone()
    }

    pub fn stop(&self) {
        self.state.stop_requested.store(true, Ordering::SeqCst);
        let mut status = self.state.status.lock().expect("status lock");
        if status.status == ClipStatus::Playing || status.status == ClipStatus::Buffering {
            status.status = ClipStatus::Stopped;
        }
    }

    /// Take one 20 ms stereo PCM frame for MixGraph / writeSample routing.
    ///
    /// Returns `None` when buffering, stopped, ended, errored, or on underrun while still playing.
    pub fn take_frame(&self) -> Option<Frame> {
        let mut status = self.state.status.lock().expect("status lock");
        match status.status {
            ClipStatus::Stopped | ClipStatus::Ended | ClipStatus::Error | ClipStatus::Buffering => {
                return None;
            }
            ClipStatus::Playing => {}
        }

        let mut queue = self.state.pcm_queue.lock().expect("pcm queue lock");
        match pop_frame_bytes(&mut queue) {
            Some(pcm) => {
                if status.buffered_ms >= FRAME_MS as u64 {
                    status.buffered_ms -= FRAME_MS as u64;
                } else {
                    status.buffered_ms = 0;
                }
                status.position_ms += FRAME_MS as u64;
                if let Some(dur) = status.duration_ms {
                    status.position_ms = status.position_ms.min(dur);
                }
                Some(Frame::new(pcm, None))
            }
            None => {
                if self.state.decode_done.load(Ordering::SeqCst) {
                    status.status = ClipStatus::Ended;
                }
                None
            }
        }
    }
}

impl Drop for ClipSession {
    fn drop(&mut self) {
        self.state.stop_requested.store(true, Ordering::SeqCst);
        if let Some(handle) = self.decode_thread.take() {
            let _ = handle.join();
        }
    }
}

fn decode_loop(_play_id: String, input: InputKind, state: Arc<SessionState>) {
    let result = match input {
        InputKind::RawPcm(data) => decode_raw_pcm(&state, data),
        InputKind::Bytes(data) => {
            let hint = hint_from_bytes(&data);
            decode_from_source(&state, bytes_source(data), Some(hint))
        }
        InputKind::Path(path) => match std::fs::read(&path) {
            Ok(data) => {
                let hint = hint_from_path(&path);
                decode_from_source(&state, bytes_source(data), Some(hint))
            }
            Err(e) => Err(PlayerError::Io(e.to_string())),
        },
        InputKind::Growing(source) => decode_growing(&state, source),
    };

    if let Err(err) = result {
        set_error(&state, err.to_string());
    }

    state.decode_done.store(true, Ordering::SeqCst);
    try_transition_to_playing(&state);
}

fn wait_until_decode_ready(source: &GrowingByteSource) {
    while !source.is_eof() {
        let snap = source.snapshot();
        if snap.is_empty() {
            thread::sleep(std::time::Duration::from_millis(2));
            continue;
        }
        let is_mp4 = probe_mp4_layout(&snap) != Mp4Layout::Unknown
            || snap.len() >= 8 && snap[4..8] == *b"ftyp";
        if is_mp4 {
            if mp4_ready_for_decode(&snap, source.is_eof()) {
                return;
            }
        } else if snap.len() >= 512 {
            return;
        }
        thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn decode_growing(state: &Arc<SessionState>, source: Arc<GrowingByteSource>) -> Result<(), PlayerError> {
    let mut session = None;
    while session.is_none() {
        if state.stop_requested.load(Ordering::SeqCst) {
            return Ok(());
        }
        wait_until_decode_ready(&*source);
        if source.is_eof() && source.snapshot().is_empty() {
            return Err(PlayerError::DecodeFailed("empty stream".into()));
        }

        let snap = source.snapshot();
        let hint = hint_from_bytes(&snap);
        let media = GrowingByteSource::from_shared(&source);
        match DecoderSession::open(media, Some(hint)) {
            Ok(opened) => session = Some(opened),
            Err(err) => {
                if source.is_eof() {
                    return Err(err);
                }
                thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    let mut session = session.expect("decoder opened");
    if let Some(dur) = session.duration_ms() {
        let mut status = state.status.lock().expect("status lock");
        status.duration_ms = Some(dur);
    }

    let mut prev_len = source.len();
    while !state.stop_requested.load(Ordering::SeqCst) {
        match session.decode_available() {
            Ok(out) => {
                let had_pcm = !out.pcm.is_empty();
                if had_pcm {
                    enqueue_pcm(state, out.pcm);
                }
                let buffered_ms = state.status.lock().expect("status lock").buffered_ms;
                if buffered_ms >= state.preroll_ms {
                    return Ok(());
                }
                if source.is_eof() && !had_pcm {
                    return Ok(());
                }
            }
            Err(err) => {
                if source.is_eof() {
                    return Err(err);
                }
            }
        }

        if state.stop_requested.load(Ordering::SeqCst) {
            break;
        }
        if source.is_eof() {
            break;
        }
        wait_for_growing_bytes(&source, prev_len, state);
        if state.stop_requested.load(Ordering::SeqCst) {
            break;
        }
        prev_len = source.len();
    }
    Ok(())
}

fn wait_for_growing_bytes(
    source: &GrowingByteSource,
    prev_len: usize,
    state: &Arc<SessionState>,
) {
    while source.len() == prev_len && !source.is_eof() {
        if state.stop_requested.load(Ordering::SeqCst) {
            return;
        }
        thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn decode_from_source<M: MediaSource + 'static>(
    state: &Arc<SessionState>,
    source: M,
    hint: Option<symphonia::core::probe::Hint>,
) -> Result<(), PlayerError> {
    let mut session = DecoderSession::open(source, hint)?;
    if let Some(dur) = session.duration_ms() {
        let mut status = state.status.lock().expect("status lock");
        status.duration_ms = Some(dur);
    }
    while !state.stop_requested.load(Ordering::SeqCst) {
        match session.decode_available() {
            Ok(out) => {
                let had_pcm = !out.pcm.is_empty();
                if had_pcm {
                    enqueue_pcm(state, out.pcm);
                }
                let buffered_ms = state.status.lock().expect("status lock").buffered_ms;
                if buffered_ms >= state.preroll_ms {
                    return Ok(());
                }
                // Static sources: empty output means EOF with no further PCM.
                if !had_pcm {
                    return Ok(());
                }
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn decode_raw_pcm(state: &Arc<SessionState>, data: Vec<u8>) -> Result<(), PlayerError> {
    if data.len() % 4 != 0 {
        return Err(PlayerError::InvalidInput(
            "raw PCM must be s16le stereo (4 bytes per frame)".into(),
        ));
    }
    enqueue_pcm(state, Bytes::from(data));
    Ok(())
}

fn enqueue_pcm(state: &Arc<SessionState>, pcm: Bytes) {
    let added_ms = stereo_byte_len_to_ms(pcm.len());
    {
        let mut queue = state.pcm_queue.lock().expect("pcm queue lock");
        queue.push(pcm);
    }
    let mut status = state.status.lock().expect("status lock");
    status.buffered_ms += added_ms;
    drop(status);
    try_transition_to_playing(state);
}

fn try_transition_to_playing(state: &Arc<SessionState>) {
    let done = state.decode_done.load(Ordering::SeqCst);
    let mut status = state.status.lock().expect("status lock");
    if status.status != ClipStatus::Buffering {
        return;
    }
    let ready = status.buffered_ms >= state.preroll_ms
        || (done && status.buffered_ms >= PREROLL_MIN_MS)
        || (done && status.buffered_ms > 0 && status.buffered_ms < PREROLL_MIN_MS);
    if ready {
        status.status = ClipStatus::Playing;
    }
}

fn pop_frame_bytes(queue: &mut Vec<Bytes>) -> Option<Bytes> {
    let available = queue.iter().map(|chunk| chunk.len()).sum::<usize>();
    if available < FRAME_BYTES {
        return None;
    }

    let mut frame = Vec::with_capacity(FRAME_BYTES);
    let mut need = FRAME_BYTES;
    while need > 0 {
        let front = &mut queue[0];
        if front.len() <= need {
            need -= front.len();
            frame.extend_from_slice(front);
            queue.remove(0);
        } else {
            frame.extend_from_slice(&front[..need]);
            *front = front.slice(need..);
            need = 0;
        }
    }
    Some(Bytes::from(frame))
}

fn set_error(state: &Arc<SessionState>, message: String) {
    let mut status = state.status.lock().expect("status lock");
    status.status = ClipStatus::Error;
    status.error = Some(message);
}

/// Split stereo PCM into 20 ms frames (utility for tests and batch export).
pub fn split_frames(pcm: &[u8]) -> Vec<Frame> {
    let mut frames = Vec::new();
    for chunk in pcm.chunks(FRAME_BYTES) {
        if chunk.len() == FRAME_BYTES {
            frames.push(Frame::new(Bytes::copy_from_slice(chunk), None));
        }
    }
    frames
}
