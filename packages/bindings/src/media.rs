//! MediaStream and MediaStreamTrack NAPI bindings.

use std::sync::Arc;
use std::time::Duration;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use node_webrtc_rust_core::{LocalAudioTrack, MediaStreamTrack, RemoteTrack, TrackKind, debug_call};

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

/// Local audio track for sending media to a peer connection.
#[napi]
pub struct JsLocalAudioTrack {
    inner: Arc<LocalAudioTrack>,
}

#[napi]
impl JsLocalAudioTrack {
    #[napi(constructor)]
    pub fn new(id: String, stream_id: String) -> Self {
        debug_call!("bindings::media", "LocalAudioTrack::new", "id={id}, stream_id={stream_id}");
        Self {
            inner: Arc::new(LocalAudioTrack::new(&id, &stream_id)),
        }
    }

    #[napi(getter)]
    pub fn id(&self) -> String {
        MediaStreamTrack::id(self.inner.as_ref()).to_string()
    }

    #[napi(getter)]
    pub fn kind(&self) -> String {
        "audio".to_string()
    }

    #[napi(getter)]
    pub fn stream_id(&self) -> String {
        MediaStreamTrack::stream_id(self.inner.as_ref()).to_string()
    }

    #[napi(getter)]
    pub fn enabled(&self) -> bool {
        MediaStreamTrack::enabled(self.inner.as_ref())
    }

    #[napi(setter)]
    pub fn set_enabled(&mut self, enabled: bool) {
        debug_call!("bindings::media", "LocalAudioTrack::set_enabled", "enabled={enabled}");
        MediaStreamTrack::set_enabled(self.inner.as_ref(), enabled);
    }

    /// Writes a PCM audio frame to the track.
    #[napi]
    pub async fn write_sample(&self, data: Buffer, duration_ms: u32) -> Result<()> {
        debug_call!(
            "bindings::media",
            "LocalAudioTrack::write_sample",
            "bytes={}, duration_ms={duration_ms}",
            data.len()
        );
        let bytes = bytes::Bytes::copy_from_slice(data.as_ref());
        self.inner
            .write_sample(bytes, Duration::from_millis(duration_ms as u64))
            .await
            .map_err(|e| napi::Error::from_reason(e.to_string()))
    }
}

impl JsLocalAudioTrack {
    pub(crate) fn inner(&self) -> Arc<LocalAudioTrack> {
        Arc::clone(&self.inner)
    }
}
