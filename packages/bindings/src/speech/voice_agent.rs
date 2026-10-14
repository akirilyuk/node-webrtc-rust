//! VoiceAgent NAPI bindings.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use napi::bindgen_prelude::*;
use napi::JsFunction;
use napi_derive::napi;
use node_webrtc_rust_mixer::{Frame, FRAME_BYTES};
use node_webrtc_rust_speech::{
    PcmReader, PcmWriter, SendTextToTtsOptions, SpeechEvent, UpdateTtsConfigOptions, VoiceAgent,
};
use tokio::sync::{broadcast, Mutex};

use crate::media::{JsLocalAudioTrack, JsMediaStreamTrack};
use crate::mixer::JsMixGraph;
use crate::speech::events::{speech_event_to_js, wire_speech_callback};
use crate::speech::registry::default_vendor_registry;
use crate::speech::types::{
    speech_err, JsBeginSttHoldOptions, JsReleaseSttHoldOptions, JsSpeechEvent, JsSttConfig,
    JsSttHoldMode, JsTtsConfig, JsUpdateTtsOptions, JsVoiceAgentConfig, JsVoiceSessionContext,
};

/// `VOICE_DEBUG=1|true|yes`, read once per process.
pub(crate) fn voice_debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("VOICE_DEBUG").ok().as_deref(),
            Some("1") | Some("true") | Some("yes")
        )
    })
}

struct VoiceAgentState {
    pull_rx: Option<broadcast::Receiver<SpeechEvent>>,
    callback_wired: bool,
    outbound: Option<Arc<node_webrtc_rust_core::LocalAudioTrack>>,
}

/// Voice agent with VAD, STT/TTS orchestration for one peer connection session.
#[napi]
pub struct JsVoiceAgent {
    inner: Arc<VoiceAgent>,
    state: Arc<Mutex<VoiceAgentState>>,
    /// Native inbound PCM loop task (see `start_native_inbound`).
    native_inbound: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Receiver-stopped errors that mean "wait for the next RTP burst" (same set as the TS loop).
pub(crate) fn is_stream_end(message: &str) -> bool {
    message.contains("DataChannel is not opened")
        || message.contains("ErrClosedPipe")
        || message.contains("closed pipe")
}

/// Injected after a stream end so the agent's VAD/STT sees the end of the utterance.
const INBOUND_SILENCE_TAIL_FRAMES: u32 = 75;
const INBOUND_FRAME: Duration = Duration::from_millis(20);

impl Drop for JsVoiceAgent {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.native_inbound.lock() {
            if let Some(handle) = slot.take() {
                handle.abort();
            }
        }
    }
}

#[napi]
impl JsVoiceAgent {
    #[napi(constructor)]
    pub fn new(config: Option<JsVoiceAgentConfig>) -> Result<Self> {
        if voice_debug_enabled() {
            eprintln!("[voice-debug] JsVoiceAgent native module loaded (rebuild with npm run build:native if missing pipeline logs)");
        }
        let config = config.unwrap_or_default().into();
        let registry = default_vendor_registry();
        let agent = VoiceAgent::new(config, registry).map_err(speech_err)?;
        let pull_rx = agent.subscribe_events();
        Ok(Self {
            inner: agent,
            state: Arc::new(Mutex::new(VoiceAgentState {
                pull_rx: Some(pull_rx),
                callback_wired: false,
                outbound: None,
            })),
            native_inbound: std::sync::Mutex::new(None),
        })
    }

    /// Attaches outbound local audio track for TTS injection.
    #[napi]
    pub async fn attach(&self, outbound_track: &JsLocalAudioTrack) -> Result<()> {
        let outbound = outbound_track.inner();
        // Live Arc so `setWriteSampleTee` after attach still receives TTS frames.
        let write_tee = outbound_track.write_tee_handle();
        let tts_sink = outbound_track.tts_sink_handle();
        let pcm_writer: PcmWriter = {
            let track = Arc::clone(&outbound);
            Arc::new(move |pcm, duration_ms| {
                let track = Arc::clone(&track);
                let write_tee = Arc::clone(&write_tee);
                let bytes = pcm;
                crate::media::notify_write_tee_handle(&write_tee, bytes.as_ref(), duration_ms);
                // Native mix pump sink: feed its queue directly, no Opus encode on the sidecar.
                let sink = tts_sink.lock().ok().and_then(|g| g.clone());
                if let Some(queue) = sink {
                    queue.push_pcm(bytes.as_ref());
                    return Ok(());
                }
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async move {
                        track
                            .write_sample(bytes, Duration::from_millis(duration_ms as u64))
                            .await
                            .map_err(|e| node_webrtc_rust_speech::SpeechError::Tts(e.to_string()))
                    })
                })
            })
        };
        let pcm_reader: PcmReader = Arc::new(|| Ok(None));
        self.inner
            .attach(pcm_reader, pcm_writer)
            .await
            .map_err(speech_err)?;
        self.state.lock().await.outbound = Some(outbound);
        Ok(())
    }

    #[napi]
    pub async fn start(&self, session_context: Option<JsVoiceSessionContext>) -> Result<()> {
        self.inner
            .start(session_context.map(Into::into))
            .await
            .map_err(speech_err)
    }

    #[napi]
    pub async fn stop(&self) -> Result<()> {
        if let Ok(mut slot) = self.native_inbound.lock() {
            if let Some(handle) = slot.take() {
                handle.abort();
            }
        }
        self.inner.stop().await.map_err(speech_err)
    }

    /// Runs the inbound PCM loop natively: reads `track`, pushes frames into `mix_graph` for
    /// `participant_id` (unless globally muted), notifies the track's read tee and feeds the
    /// agent. Call after `start`.
    #[napi]
    pub fn start_native_inbound(
        &self,
        track: &JsMediaStreamTrack,
        mix_graph: Option<&JsMixGraph>,
        participant_id: Option<String>,
    ) -> Result<()> {
        let remote = track
            .remote_track()
            .ok_or_else(|| Error::from_reason("startNativeInbound requires a remote track"))?;
        let read_tee = track.read_tee_handle();
        let graph = mix_graph.map(JsMixGraph::graph_handle);
        let mut slot = self
            .native_inbound
            .lock()
            .map_err(|_| Error::from_reason("native inbound lock poisoned"))?;
        if slot.as_ref().is_some_and(|h| !h.is_finished()) {
            return Err(Error::from_reason("native inbound loop already running"));
        }
        if voice_debug_enabled() {
            eprintln!(
                "[voice-debug] native inbound loop started participant={}",
                participant_id.as_deref().unwrap_or("none")
            );
        }
        let agent = Arc::clone(&self.inner);
        *slot = Some(napi::bindgen_prelude::spawn(async move {
            let silent = Bytes::from(vec![0u8; FRAME_BYTES]);
            let mut awaiting_next_burst = false;
            while agent.is_running() {
                match remote.read_sample().await {
                    Ok(sample) => {
                        awaiting_next_burst = false;
                        let dur = sample.duration.as_millis() as u32;
                        if let (Some(graph), Some(pid)) = (&graph, &participant_id) {
                            let mut g = graph.lock().unwrap_or_else(|e| e.into_inner());
                            if !g.is_globally_muted(pid) {
                                #[allow(clippy::chunks_exact_to_as_chunks)]
                                for chunk in sample.pcm.chunks_exact(FRAME_BYTES) {
                                    g.push_frame(
                                        pid.clone(),
                                        Frame::new(Bytes::copy_from_slice(chunk), None),
                                    );
                                }
                            }
                        }
                        crate::media::notify_write_tee_handle(&read_tee, sample.pcm.as_ref(), dur);
                        if let Err(err) = agent.process_inbound_pcm(sample.pcm, dur).await {
                            if voice_debug_enabled() {
                                eprintln!(
                                    "[voice-debug] native inbound processInboundPcm error: {err}"
                                );
                            }
                        }
                    }
                    Err(err) if is_stream_end(&err.to_string()) => {
                        if !awaiting_next_burst {
                            awaiting_next_burst = true;
                            for _ in 0..INBOUND_SILENCE_TAIL_FRAMES {
                                if !agent.is_running() {
                                    break;
                                }
                                let _ = agent.process_inbound_pcm(silent.clone(), 20).await;
                                tokio::time::sleep(INBOUND_FRAME).await;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    Err(err) => {
                        if voice_debug_enabled() {
                            eprintln!("[voice-debug] native inbound readSample error: {err}");
                        }
                        tokio::time::sleep(INBOUND_FRAME).await;
                    }
                }
            }
        }));
        Ok(())
    }

    #[napi]
    pub async fn send_text_to_tts(
        &self,
        text: String,
        non_blocking: Option<bool>,
        interruptible: Option<bool>,
    ) -> Result<()> {
        let options = SendTextToTtsOptions {
            non_blocking: non_blocking.unwrap_or(false),
            interruptible: interruptible.unwrap_or(true),
        };
        self.inner
            .send_text_to_tts_with_options(&text, options)
            .await
            .map_err(speech_err)
    }

    #[napi]
    pub async fn flush_tts(&self) -> Result<()> {
        self.inner.flush_tts().await.map_err(speech_err)
    }

    /// Wait until outbound TTS playback finishes (synthesis queue drained and agent not speaking).
    #[napi]
    pub async fn wait_tts_playback_idle(&self) -> Result<()> {
        self.inner
            .wait_tts_playback_idle()
            .await
            .map_err(speech_err)
    }

    /// Pull the next speech event for async stream consumption.
    #[napi]
    pub async fn pull_speech_event(&self) -> Result<Option<JsSpeechEvent>> {
        let mut state = self.state.lock().await;
        let rx = state
            .pull_rx
            .as_mut()
            .ok_or_else(|| Error::from_reason("speech event stream unavailable"))?;
        loop {
            match rx.try_recv() {
                Ok(event) => return Ok(Some(speech_event_to_js(event))),
                Err(broadcast::error::TryRecvError::Empty) => return Ok(None),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => return Ok(None),
            }
        }
    }

    #[napi]
    pub fn set_on_speech_event(&self, env: Env, callback: JsFunction) -> Result<()> {
        let mut state = self.state.blocking_lock();
        if state.callback_wired {
            return Ok(());
        }
        state.callback_wired = true;
        let rx = self.inner.subscribe_events();
        wire_speech_callback(&env, callback, rx)
    }

    /// Processes one inbound PCM frame (48 kHz stereo) through VAD/STT.
    #[napi]
    pub async fn process_inbound_pcm(&self, data: Buffer, duration_ms: u32) -> Result<()> {
        self.inner
            .process_inbound_pcm(Bytes::copy_from_slice(data.as_ref()), duration_ms)
            .await
            .map_err(speech_err)
    }

    /// When `false`, inbound PCM still runs VAD but skips STT and user speech events.
    #[napi]
    pub async fn set_stt_enabled(&self, enabled: bool) -> Result<()> {
        self.inner.set_stt_enabled(enabled).await;
        Ok(())
    }

    #[napi]
    pub async fn stt_enabled(&self) -> Result<bool> {
        Ok(self.inner.stt_enabled().await)
    }

    /// Apply a new STT config after the current utterance finalizes (or immediately when idle).
    #[napi]
    pub async fn update_stt(&self, config: JsSttConfig) -> Result<()> {
        self.inner
            .update_stt_config(config.into())
            .await
            .map_err(speech_err)
    }

    /// Apply a new TTS config before the next synthesis job.
    #[napi]
    pub async fn update_tts(
        &self,
        config: JsTtsConfig,
        options: Option<JsUpdateTtsOptions>,
    ) -> Result<()> {
        let opts = options.unwrap_or_default();
        self.inner
            .update_tts_config(
                config.into(),
                UpdateTtsConfigOptions {
                    cancel_inflight: opts.cancel_inflight.unwrap_or(false),
                },
            )
            .await
            .map_err(speech_err)
    }

    /// Start a host STT hold: stop feeding/polling the current STT and buffer user PCM.
    #[napi]
    pub async fn begin_stt_hold(&self, options: JsBeginSttHoldOptions) -> Result<()> {
        use node_webrtc_rust_speech::stt_hold::{BeginSttHoldOptions, SttHoldMode};
        self.inner
            .begin_stt_hold(BeginSttHoldOptions {
                mode: match options.mode {
                    JsSttHoldMode::BufferReplay => SttHoldMode::BufferReplay,
                    JsSttHoldMode::FirstUtterance => SttHoldMode::FirstUtterance,
                },
                max_buffer_ms: options.max_buffer_ms,
            })
            .await
            .map_err(speech_err)
    }

    /// End the hold after `update_stt`; `replay` decodes held audio through the new STT.
    #[napi]
    pub async fn release_stt_hold(&self, options: JsReleaseSttHoldOptions) -> Result<()> {
        use node_webrtc_rust_speech::stt_hold::ReleaseSttHoldOptions;
        self.inner
            .release_stt_hold(ReleaseSttHoldOptions {
                replay: options.replay,
            })
            .await
            .map_err(speech_err)
    }

    /// Abort the hold: the old STT resumes and receives the buffered audio.
    #[napi]
    pub async fn cancel_stt_hold(&self) -> Result<()> {
        self.inner.cancel_stt_hold().await.map_err(speech_err)
    }

    /// Re-transcribe the last utterance PCM on the current STT provider (after `update_stt`).
    #[napi]
    pub async fn replay_last_utterance(&self) -> Result<()> {
        self.inner.replay_last_utterance().await.map_err(speech_err)
    }
}

#[cfg(test)]
mod tests {
    use super::is_stream_end;

    #[test]
    fn stream_end_messages_match() {
        assert!(is_stream_end("read failed: DataChannel is not opened"));
        assert!(is_stream_end("io: ErrClosedPipe"));
        assert!(is_stream_end("write on closed pipe"));
        assert!(!is_stream_end("opus decode failed"));
    }
}
