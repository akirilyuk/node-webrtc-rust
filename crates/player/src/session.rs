//! Per-clip decode and playback session.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_mixer::{Frame, FRAME_BYTES};
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
    play_thread: Option<JoinHandle<()>>,
}

struct SessionState {
    status: Mutex<ClipPlayerStatus>,
    pcm_queue: Mutex<Vec<Bytes>>,
    stop_requested: AtomicBool,
    decode_done: AtomicBool,
    preroll_ms: u64,
    playing_since: Mutex<Option<Instant>>,
    playhead_base_ms: Mutex<u64>,
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
            playing_since: Mutex::new(None),
            playhead_base_ms: Mutex::new(0),
        });

        let decode_thread = {
            let state = Arc::clone(&state);
            let play_id = play_id.clone();
            thread::spawn(move || decode_loop(play_id, input, state))
        };

        let play_thread = {
            let state = Arc::clone(&state);
            thread::spawn(move || play_loop(state))
        };

        Self {
            play_id,
            state,
            decode_thread: Some(decode_thread),
            play_thread: Some(play_thread),
        }
    }

    pub fn play_id(&self) -> &str {
        &self.play_id
    }

    pub fn status(&self) -> ClipPlayerStatus {
        let mut status = self.state.status.lock().expect("status lock");
        update_position_locked(&self.state, &mut status);
        status.clone()
    }

    pub fn stop(&self) {
        self.state.stop_requested.store(true, Ordering::SeqCst);
        let mut status = self.state.status.lock().expect("status lock");
        if status.status == ClipStatus::Playing || status.status == ClipStatus::Buffering {
            update_position_locked(&self.state, &mut status);
            status.status = ClipStatus::Stopped;
        }
    }
}

impl Drop for ClipSession {
    fn drop(&mut self) {
        self.state.stop_requested.store(true, Ordering::SeqCst);
        if let Some(handle) = self.decode_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.play_thread.take() {
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
            thread::sleep(Duration::from_millis(2));
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
        thread::sleep(Duration::from_millis(2));
    }
}

fn decode_growing(state: &Arc<SessionState>, source: Arc<GrowingByteSource>) -> Result<(), PlayerError> {
    while !state.stop_requested.load(Ordering::SeqCst) {
        wait_until_decode_ready(&*source);
        if source.is_eof() && source.snapshot().is_empty() {
            return Err(PlayerError::DecodeFailed("empty stream".into()));
        }

        let snap = source.snapshot();
        let hint = hint_from_bytes(&snap);
        let media = GrowingByteSource::from_shared(&source);

        match DecoderSession::open(media, Some(hint)) {
            Ok(mut session) => {
                if let Some(dur) = session.duration_ms() {
                    let mut status = state.status.lock().expect("status lock");
                    status.duration_ms = Some(dur);
                }
                if state.stop_requested.load(Ordering::SeqCst) {
                    return Ok(());
                }
                match session.decode_available() {
                    Ok(out) => {
                        if !out.pcm.is_empty() {
                            enqueue_pcm(state, out.pcm);
                        }
                        return Ok(());
                    }
                    Err(err) => {
                        if source.is_eof() {
                            return Err(err);
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                }
            }
            Err(err) => {
                if source.is_eof() {
                    return Err(err);
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
    Ok(())
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
    let out = session.decode_available()?;
    if !out.pcm.is_empty() {
        enqueue_pcm(state, out.pcm);
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
        let mut playing_since = state.playing_since.lock().expect("playing_since lock");
        *playing_since = Some(Instant::now());
    }
}

fn play_loop(state: Arc<SessionState>) {
    while !state.stop_requested.load(Ordering::SeqCst) {
        let mut status = state.status.lock().expect("status lock");
        update_position_locked(&state, &mut status);

        if status.status == ClipStatus::Playing {
            let mut queue = state.pcm_queue.lock().expect("pcm queue lock");
            if queue.is_empty() && state.decode_done.load(Ordering::SeqCst) {
                status.status = ClipStatus::Ended;
                break;
            }
            if !queue.is_empty() {
                drain_frame(&mut queue);
                if status.buffered_ms >= 20 {
                    status.buffered_ms -= 20;
                }
            }
        } else if status.status == ClipStatus::Ended
            || status.status == ClipStatus::Stopped
            || status.status == ClipStatus::Error
        {
            break;
        }
        drop(status);
        thread::sleep(Duration::from_millis(20));
    }
}

fn drain_frame(queue: &mut Vec<Bytes>) {
    let mut need = FRAME_BYTES;
    while need > 0 && !queue.is_empty() {
        let front = &mut queue[0];
        if front.len() <= need {
            need -= front.len();
            queue.remove(0);
        } else {
            *front = front.slice(need..);
            need = 0;
        }
    }
}

fn update_position_locked(state: &Arc<SessionState>, status: &mut ClipPlayerStatus) {
    if status.status != ClipStatus::Playing {
        return;
    }
    let playing_since = state.playing_since.lock().expect("playing_since lock");
    if let Some(started) = *playing_since {
        let base = *state.playhead_base_ms.lock().expect("playhead_base lock");
        let elapsed = started.elapsed().as_millis() as u64;
        let pos = base + elapsed;
        if let Some(dur) = status.duration_ms {
            status.position_ms = pos.min(dur);
        } else {
            status.position_ms = pos;
        }
    }
}

fn set_error(state: &Arc<SessionState>, message: String) {
    let mut status = state.status.lock().expect("status lock");
    status.status = ClipStatus::Error;
    status.error = Some(message);
}

/// Split stereo PCM into 20 ms frames (utility for future mixer integration).
pub fn split_frames(pcm: &[u8]) -> Vec<Frame> {
    let mut frames = Vec::new();
    for chunk in pcm.chunks(FRAME_BYTES) {
        if chunk.len() == FRAME_BYTES {
            frames.push(Frame::new(Bytes::copy_from_slice(chunk), None));
        }
    }
    frames
}
