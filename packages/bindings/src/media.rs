//! MediaStream and MediaStreamTrack NAPI bindings.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::JsFunction;
use napi::JsUnknown;
use napi_derive::napi;
use node_webrtc_rust_core::{debug_call, LocalAudioTrack, MediaStreamTrack, RemoteTrack, TrackKind};

use crate::config::to_js_unknown;
use crate::events::create_event_callback;

/// Optional JS listener for every PCM frame written (JS `writeSample` + VoiceAgent TTS drain).
type WriteSampleTee = ThreadsafeFunction<(Vec<u8>, u32)>;

/// Media stream track exposed to JavaScript.
#[napi]
pub struct JsMediaStreamTrack {
    id: String,
    kind: String,
    stream_id: String,
    enabled: bool,
    remote: Option<RemoteTrack>,
}

impl JsMediaStreamTrack {
    pub(crate) fn from_remote(track: RemoteTrack) -> Self {
        Self {
            id: track.id().to_string(),
            kind: track_kind_to_string(track.kind()),
            stream_id: track.stream_id().to_string(),
            enabled: true,
            remote: Some(track),
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

    /// Decodes the next inbound Opus RTP packet to stereo PCM (remote audio only).
    #[napi]
    pub async fn read_sample(&self) -> Result<Buffer> {
        let remote = self
            .remote
            .as_ref()
            .ok_or_else(|| Error::from_reason("readSample requires a remote track"))?;
        if self.kind != "audio" {
            return Err(Error::from_reason("readSample supports audio tracks only"));
        }
        let sample = remote.read_sample().await.map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(Buffer::from(sample.pcm.as_ref()))
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
    write_tee: Arc<Mutex<Option<WriteSampleTee>>>,
}

#[napi]
impl JsLocalAudioTrack {
    #[napi(constructor)]
    pub fn new(id: String, stream_id: String) -> Self {
        debug_call!("bindings::media", "LocalAudioTrack::new", "id={id}, stream_id={stream_id}");
        Self {
            inner: Arc::new(LocalAudioTrack::new(&id, &stream_id)),
            write_tee: Arc::new(Mutex::new(None)),
        }
    }

    /// Registers a non-blocking listener for PCM written via {@link writeSample} or VoiceAgent TTS.
    /// Pass `null` to clear. Used by load-test stereo WAV capture (native TTS bypasses JS patches).
    #[napi]
    pub fn set_write_sample_tee(&self, env: Env, callback: Option<JsFunction>) -> Result<()> {
        let mut slot = self
            .write_tee
            .lock()
            .map_err(|e| Error::from_reason(format!("write_sample_tee lock: {e}")))?;
        *slot = match callback {
            Some(cb) => {
                let tsfn = create_event_callback(
                    &env,
                    cb,
                    |ctx| -> Result<Vec<JsUnknown>> {
                        let (data, duration_ms) = ctx.value;
                        let buffer = Buffer::from(data);
                        let buf_js = to_js_unknown(&ctx.env, buffer)?;
                        let dur_js =
                            to_js_unknown(&ctx.env, ctx.env.create_uint32(duration_ms)?)?;
                        Ok(vec![buf_js, dur_js])
                    },
                )?;
                Some(tsfn)
            }
            None => None,
        };
        Ok(())
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

    /// Writes interleaved stereo PCM; encoded to the negotiated RTP codec before send.
    #[napi]
    pub async fn write_sample(&self, data: Buffer, duration_ms: u32) -> Result<()> {
        debug_call!(
            "bindings::media",
            "LocalAudioTrack::write_sample",
            "bytes={}, duration_ms={duration_ms}",
            data.len()
        );
        self.notify_write_tee(data.as_ref(), duration_ms);
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

    pub(crate) fn write_tee_handle(&self) -> Arc<Mutex<Option<WriteSampleTee>>> {
        Arc::clone(&self.write_tee)
    }

    pub(crate) fn notify_write_tee(&self, pcm: &[u8], duration_ms: u32) {
        notify_write_tee_handle(&self.write_tee, pcm, duration_ms);
    }
}

pub(crate) fn notify_write_tee_handle(
    tee: &Mutex<Option<WriteSampleTee>>,
    pcm: &[u8],
    duration_ms: u32,
) {
    let Ok(guard) = tee.lock() else {
        return;
    };
    let Some(tsfn) = guard.as_ref() else {
        return;
    };
    let _ = tsfn.call(
        Ok((pcm.to_vec(), duration_ms)),
        ThreadsafeFunctionCallMode::NonBlocking,
    );
}
