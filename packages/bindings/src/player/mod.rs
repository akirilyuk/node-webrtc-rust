use napi::bindgen_prelude::*;
use napi_derive::napi;
use node_webrtc_rust_player::{ClipPlayerStatus, ClipStatus};

#[napi(string_enum)]
pub enum JsClipStatus {
    Buffering,
    Playing,
    Stopped,
    Ended,
    Error,
}

impl From<ClipStatus> for JsClipStatus {
    fn from(value: ClipStatus) -> Self {
        match value {
            ClipStatus::Buffering => JsClipStatus::Buffering,
            ClipStatus::Playing => JsClipStatus::Playing,
            ClipStatus::Stopped => JsClipStatus::Stopped,
            ClipStatus::Ended => JsClipStatus::Ended,
            ClipStatus::Error => JsClipStatus::Error,
        }
    }
}

#[napi(object)]
pub struct JsClipPlayerStatus {
    pub play_id: String,
    pub status: JsClipStatus,
    pub position_ms: u32,
    pub duration_ms: Option<u32>,
    pub buffered_ms: u32,
    pub error: Option<String>,
}

impl From<ClipPlayerStatus> for JsClipPlayerStatus {
    fn from(value: ClipPlayerStatus) -> Self {
        Self {
            play_id: value.play_id,
            status: JsClipStatus::from(value.status),
            position_ms: value.position_ms.min(u32::MAX as u64) as u32,
            duration_ms: value
                .duration_ms
                .map(|ms| ms.min(u32::MAX as u64) as u32),
            buffered_ms: value.buffered_ms.min(u32::MAX as u64) as u32,
            error: value.error,
        }
    }
}

#[napi]
pub fn play_clip_from_path(path: String) -> Result<String> {
    node_webrtc_rust_player::play_clip_from_path(&path)
        .map_err(|e| Error::from_reason(e.to_string()))
}

#[napi]
pub fn play_clip_from_bytes(data: Buffer) -> Result<String> {
    node_webrtc_rust_player::play_clip_from_bytes(data.to_vec())
        .map_err(|e| Error::from_reason(e.to_string()))
}

#[napi]
pub struct JsGrowingClipWriter {
    play_id: String,
    writer: node_webrtc_rust_player::GrowingByteWriter,
}

#[napi]
pub fn play_clip_progressive() -> Result<JsGrowingClipWriter> {
    let (play_id, writer) = node_webrtc_rust_player::play_clip_progressive()
        .map_err(|e| Error::from_reason(e.to_string()))?;
    Ok(JsGrowingClipWriter { play_id, writer })
}

#[napi]
impl JsGrowingClipWriter {
    #[napi(getter)]
    pub fn play_id(&self) -> String {
        self.play_id.clone()
    }

    #[napi]
    pub fn append(&self, chunk: Buffer) {
        self.writer.append(&chunk);
    }

    #[napi]
    pub fn mark_eof(&self) {
        self.writer.mark_eof();
    }

    #[napi]
    pub fn is_eof(&self) -> bool {
        self.writer.is_eof()
    }
}

#[napi]
pub fn get_clip_status(play_id: String) -> Result<Option<JsClipPlayerStatus>> {
    Ok(node_webrtc_rust_player::get_clip_status(&play_id).map(JsClipPlayerStatus::from))
}

#[napi]
pub fn stop_clip(play_id: String) -> bool {
    node_webrtc_rust_player::stop_clip(&play_id)
}
