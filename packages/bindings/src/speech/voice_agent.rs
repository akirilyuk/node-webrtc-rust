//! VoiceAgent NAPI bindings.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use napi::bindgen_prelude::*;
use napi::JsFunction;
use napi_derive::napi;
use node_webrtc_rust_mixer::{Frame, FRAME_BYTES};
use node_webrtc_rust_speech::inbound_pump::{spawn_inbound_pump, InboundPumpHandles, InboundRead};
use node_webrtc_rust_speech::{
    PcmReader, PcmWriter, SendTextToTtsOptions, SpeechEvent, UpdateTtsConfigOptions, VoiceAgent,
};
use tokio::sync::{broadcast, mpsc, Mutex, Notify};

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
    callback_wired: bool,
    outbound: Option<Arc<node_webrtc_rust_core::LocalAudioTrack>>,
}

/// Voice agent with VAD, STT/TTS orchestration for one peer connection session.
#[napi]
pub struct JsVoiceAgent {
    inner: Arc<VoiceAgent>,
    state: Arc<Mutex<VoiceAgentState>>,
    /// Events forwarded from the broadcast bus; drained by `next_speech_event`.
    events_rx: Arc<Mutex<mpsc::UnboundedReceiver<SpeechEvent>>>,
    /// Sender half and bus receiver, consumed by the forwarder spawned on the first `start`.
    forwarder_init: std::sync::Mutex<
        Option<(
            broadcast::Receiver<SpeechEvent>,
            mpsc::UnboundedSender<SpeechEvent>,
        )>,
    >,
    /// Broadcast events lost before the forwarder (never expected: the forwarder is native).
    events_lagged: Arc<AtomicU64>,
    /// Woken by `stop()` so a pending `next_speech_event` resolves `None`.
    stop_notify: Arc<Notify>,
    /// Set by `stop()`, cleared by `start()`: a late `next_speech_event` must not wait forever.
    stopped: AtomicBool,
    /// Native inbound PCM loop task (see `start_native_inbound`).
    native_inbound: std::sync::Mutex<Option<InboundPumpHandles>>,
}

/// Moves every bus event into `tx` in order. The bus holds 256 events, so this must keep
/// reading; it runs natively, so a stalled JS thread cannot make it lag.
pub(crate) async fn forward_events(
    mut rx: broadcast::Receiver<SpeechEvent>,
    tx: mpsc::UnboundedSender<SpeechEvent>,
    lagged: Arc<AtomicU64>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                if tx.send(event).is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                lagged.fetch_add(n, Ordering::Relaxed);
                eprintln!("[voice] speech events lagged: {n} dropped before the JS forwarder");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Receiver-stopped errors that mean "wait for the next RTP burst" (same set as the TS loop).
pub(crate) fn is_stream_end(message: &str) -> bool {
    message.contains("DataChannel is not opened")
        || message.contains("ErrClosedPipe")
        || message.contains("closed pipe")
}

impl Drop for JsVoiceAgent {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.native_inbound.lock() {
            if let Some(handles) = slot.take() {
                handles.abort();
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
        let config = config.unwrap_or_default().try_into()?;
        let registry = default_vendor_registry();
        let agent = VoiceAgent::new(config, registry).map_err(speech_err)?;
        let bus_rx = agent.subscribe_events();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        Ok(Self {
            inner: agent,
            state: Arc::new(Mutex::new(VoiceAgentState {
                callback_wired: false,
                outbound: None,
            })),
            events_rx: Arc::new(Mutex::new(events_rx)),
            forwarder_init: std::sync::Mutex::new(Some((bus_rx, events_tx))),
            events_lagged: Arc::new(AtomicU64::new(0)),
            stop_notify: Arc::new(Notify::new()),
            stopped: AtomicBool::new(false),
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
        self.stopped.store(false, Ordering::SeqCst);
        // Spawned here (not in the constructor, which may run outside the runtime).
        let init = self.forwarder_init.lock().ok().and_then(|mut g| g.take());
        if let Some((rx, tx)) = init {
            napi::bindgen_prelude::spawn(forward_events(rx, tx, Arc::clone(&self.events_lagged)));
        }
        self.inner
            .start(session_context.map(Into::into))
            .await
            .map_err(speech_err)
    }

    #[napi]
    pub async fn stop(&self) -> Result<()> {
        if let Ok(mut slot) = self.native_inbound.lock() {
            if let Some(handles) = slot.take() {
                handles.abort();
            }
        }
        let result = self.inner.stop().await.map_err(speech_err);
        self.stopped.store(true, Ordering::SeqCst);
        self.stop_notify.notify_waiters();
        result
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
        let read_agent = Arc::clone(&self.inner);
        let process_agent = Arc::clone(&self.inner);
        // The reader task (read, mix graph, read tee) never waits for the agent: frames reach
        // the agent through an unbounded queue (see `inbound_pump`).
        *slot = Some(spawn_inbound_pump(
            move || {
                let remote = remote.clone();
                async move {
                    match remote.read_sample().await {
                        Ok(sample) => InboundRead::Pcm {
                            duration_ms: sample.duration.as_millis() as u32,
                            pcm: sample.pcm,
                            concealed: sample.concealed,
                        },
                        Err(err) if is_stream_end(&err.to_string()) => InboundRead::StreamEnd,
                        Err(err) => InboundRead::Error(err.to_string()),
                    }
                }
            },
            move |pcm: &Bytes, dur: u32| {
                if let (Some(graph), Some(pid)) = (&graph, &participant_id) {
                    let mut g = graph.lock().unwrap_or_else(|e| e.into_inner());
                    if !g.is_globally_muted(pid) {
                        #[allow(clippy::chunks_exact_to_as_chunks)]
                        for chunk in pcm.chunks_exact(FRAME_BYTES) {
                            g.push_frame(
                                pid.clone(),
                                Frame::new(Bytes::copy_from_slice(chunk), None),
                            );
                        }
                    }
                }
                crate::media::notify_write_tee_handle(&read_tee, pcm.as_ref(), dur);
            },
            move |pcm, dur, concealed| {
                let agent = Arc::clone(&process_agent);
                async move {
                    agent
                        .process_inbound_pcm_with_meta(pcm, dur, concealed)
                        .await
                        .map_err(|e| e.to_string())
                }
            },
            move || read_agent.is_running(),
            napi::bindgen_prelude::spawn,
        ));
        Ok(())
    }

    #[napi]
    pub async fn send_text_to_tts(
        &self,
        text: String,
        non_blocking: Option<bool>,
        interruptible: Option<bool>,
        reply_id: Option<String>,
    ) -> Result<()> {
        let options = SendTextToTtsOptions {
            non_blocking: non_blocking.unwrap_or(false),
            interruptible: interruptible.unwrap_or(true),
            reply_id,
        };
        self.inner
            .send_text_to_tts_with_options(&text, options)
            .await
            .map_err(speech_err)
    }

    /// Forget the enrolled caller voice (speaker gate); the next utterances enroll again.
    #[napi]
    pub fn reset_speaker_enrollment(&self) {
        self.inner.reset_speaker_enrollment();
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

    /// Awaits the next speech event from the native forwarder. Resolves `null` after `stop()`.
    /// Use this from one consumer only.
    #[napi]
    pub async fn next_speech_event(&self) -> Result<Option<JsSpeechEvent>> {
        let mut rx = self.events_rx.lock().await;
        if let Ok(e) = rx.try_recv() {
            return Ok(Some(speech_event_to_js(e)));
        }
        // Register for the stop wake-up before checking the flag, so none can be missed.
        let notified = self.stop_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.stopped.load(Ordering::SeqCst) {
            return Ok(rx.try_recv().ok().map(speech_event_to_js));
        }
        tokio::select! {
            biased;
            event = rx.recv() => Ok(event.map(speech_event_to_js)),
            _ = &mut notified => Ok(rx.try_recv().ok().map(speech_event_to_js)),
        }
    }

    /// Non-blocking variant of `next_speech_event` (kept for compatibility).
    #[napi]
    pub async fn pull_speech_event(&self) -> Result<Option<JsSpeechEvent>> {
        let Ok(mut rx) = self.events_rx.try_lock() else {
            return Ok(None);
        };
        Ok(rx.try_recv().ok().map(speech_event_to_js))
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
            .update_stt_config(config.try_into()?)
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
                config.try_into()?,
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
    use super::{forward_events, is_stream_end};
    use node_webrtc_rust_speech::{SpeechEvent, SpeechEventBus};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn forwarder_delivers_a_burst_beyond_bus_capacity_in_order() {
        let bus = SpeechEventBus::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let lagged = Arc::new(AtomicU64::new(0));
        tokio::spawn(forward_events(bus.subscribe(), tx, Arc::clone(&lagged)));
        // 1,000 events while the consumer reads nothing; batches of 200 stay under the bus
        // capacity (256) and yield so the forwarder keeps draining it.
        for batch in 0..5 {
            for i in 0..200 {
                bus.emit(SpeechEvent::user_speech_partial(
                    format!("{}", batch * 200 + i),
                    None,
                ));
            }
            // The forwarder reads continuously but tokio's cooperative budget makes it yield
            // every 128 events: let it finish this batch before the next one.
            let target = (batch + 1) * 200;
            for _ in 0..1000 {
                if rx.len() >= target {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(rx.len() >= target, "forwarder fell behind at batch {batch}");
        }
        for expected in 0..1000 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                .await
                .expect("event within 5 s")
                .expect("channel open");
            assert_eq!(event.text.as_deref(), Some(expected.to_string().as_str()));
        }
        assert_eq!(lagged.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn stream_end_messages_match() {
        assert!(is_stream_end("read failed: DataChannel is not opened"));
        assert!(is_stream_end("io: ErrClosedPipe"));
        assert!(is_stream_end("write on closed pipe"));
        assert!(!is_stream_end("opus decode failed"));
    }
}
