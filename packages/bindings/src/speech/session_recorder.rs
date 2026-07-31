//! Session stereo audio recorder NAPI bindings.

use napi::bindgen_prelude::*;
use napi_derive::napi;
use node_webrtc_rust_speech::{
    SessionAudioFormat, SessionFinalizeResult, SessionRecorder, SessionRecorderError,
};

fn recorder_err(err: SessionRecorderError) -> Error {
    Error::from_reason(err.to_string())
}

/// Export format for {@link JsSessionRecorder.finalize}.
#[napi(string_enum)]
pub enum JsSessionAudioFormat {
    Wav,
    Opus,
}

impl From<JsSessionAudioFormat> for SessionAudioFormat {
    fn from(value: JsSessionAudioFormat) -> Self {
        match value {
            JsSessionAudioFormat::Wav => SessionAudioFormat::Wav,
            JsSessionAudioFormat::Opus => SessionAudioFormat::Opus,
        }
    }
}

/// Finalized session audio payload (WAV PCM or Opus-in-Ogg).
#[napi(object)]
pub struct JsSessionFinalizeResult {
    pub format: JsSessionAudioFormat,
    pub data: Buffer,
    pub duration_ms: u32,
    pub outbound_frames: u32,
    pub inbound_frames: u32,
}

impl From<SessionFinalizeResult> for JsSessionFinalizeResult {
    fn from(value: SessionFinalizeResult) -> Self {
        let format = match value.format {
            SessionAudioFormat::Wav => JsSessionAudioFormat::Wav,
            SessionAudioFormat::Opus => JsSessionAudioFormat::Opus,
        };
        Self {
            format,
            data: Buffer::from(value.data),
            duration_ms: value.duration_ms,
            outbound_frames: value.outbound_frames,
            inbound_frames: value.inbound_frames,
        }
    }
}

/// Vendor-agnostic stereo session recorder (L=outbound, R=inbound @ 48 kHz).
#[napi]
pub struct JsSessionRecorder {
    inner: SessionRecorder,
}

#[napi]
impl JsSessionRecorder {
    /// Creates a recorder. `maxDurationMs` defaults to 90_000 (90 s cap).
    #[napi(constructor)]
    pub fn new(max_duration_ms: Option<u32>) -> Self {
        Self {
            inner: SessionRecorder::new(max_duration_ms),
        }
    }

    #[napi(getter)]
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Push client outbound PCM (mic / TTS). Accepts mono or stereo s16le @ 48 kHz.
    #[napi]
    pub fn push_outbound(&mut self, pcm: Buffer) -> Result<()> {
        self.inner
            .push_outbound(pcm.as_ref())
            .map_err(recorder_err)
    }

    /// Push agent inbound PCM (ready TTS + echo). Accepts mono or stereo s16le @ 48 kHz.
    #[napi]
    pub fn push_inbound(&mut self, pcm: Buffer) -> Result<()> {
        self.inner.push_inbound(pcm.as_ref()).map_err(recorder_err)
    }

    /// Build stereo WAV bytes without closing the recorder.
    #[napi]
    pub fn build_wav(&self) -> Result<Buffer> {
        let built = self.inner.build().map_err(recorder_err)?;
        Ok(Buffer::from(node_webrtc_rust_speech::encode_pcm16le_wav(
            &built.pcm_interleaved,
            node_webrtc_rust_speech::SESSION_AUDIO_SAMPLE_RATE,
            node_webrtc_rust_speech::SESSION_AUDIO_CHANNELS,
        )))
    }

    /// Finalize capture. Default format is WAV; pass `Opus` for Ogg/Opus @ 256 kbps.
    #[napi]
    pub fn finalize(&mut self, format: Option<JsSessionAudioFormat>) -> Result<JsSessionFinalizeResult> {
        let format = format.unwrap_or(JsSessionAudioFormat::Wav);
        self.inner
            .finalize(format.into())
            .map(JsSessionFinalizeResult::from)
            .map_err(recorder_err)
    }
}
