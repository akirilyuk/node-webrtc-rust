//! MediaStream and MediaStreamTrack NAPI stubs.

use napi_derive::napi;
use node_webrtc_rust_core::{RemoteTrack, TrackKind};

/// Media stream track exposed to JavaScript (stub).
#[napi]
pub struct JsMediaStreamTrack {
    id: String,
    kind: String,
    stream_id: String,
    enabled: bool,
}

impl JsMediaStreamTrack {
    pub(crate) fn from_remote(track: RemoteTrack) -> Self {
        Self {
            id: track.id().to_string(),
            kind: track_kind_to_string(track.kind()),
            stream_id: track.stream_id().to_string(),
            enabled: true,
        }
    }
}

#[napi]
impl JsMediaStreamTrack {
    #[napi(getter)]
    pub fn id(&self) -> String {
        self.id.clone()
    }

    #[napi(getter)]
    pub fn kind(&self) -> String {
        self.kind.clone()
    }

    #[napi(getter)]
    pub fn stream_id(&self) -> String {
        self.stream_id.clone()
    }

    #[napi(getter)]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    #[napi(setter)]
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

/// Media stream exposed to JavaScript (stub).
#[napi]
pub struct JsMediaStream {
    id: String,
}

#[napi]
impl JsMediaStream {
    #[napi(constructor)]
    pub fn new(id: String) -> Self {
        Self { id }
    }

    #[napi(getter)]
    pub fn id(&self) -> String {
        self.id.clone()
    }
}

fn track_kind_to_string(kind: TrackKind) -> String {
    match kind {
        TrackKind::Audio => "audio".to_string(),
        TrackKind::Video => "video".to_string(),
    }
}
