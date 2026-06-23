//! VoiceAgent NAPI bindings.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use napi::bindgen_prelude::*;
use napi::JsFunction;
use napi_derive::napi;
use node_webrtc_rust_speech::{PcmReader, PcmWriter, SendTextToTtsOptions, SpeechEvent, VoiceAgent};
use tokio::sync::{broadcast, Mutex};

use crate::media::JsLocalAudioTrack;
use crate::speech::events::{speech_event_to_js, wire_speech_callback};
use crate::speech::registry::default_vendor_registry;
use crate::speech::types::{speech_err, JsSpeechEvent, JsVoiceAgentConfig};

fn voice_debug_enabled() -> bool {
    matches!(
        std::env::var("VOICE_DEBUG").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
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
        })
    }

    /// Attaches outbound local audio track for TTS injection.
    #[napi]
    pub async fn attach(&self, outbound_track: &JsLocalAudioTrack) -> Result<()> {
        let outbound = outbound_track.inner();
        let pcm_writer: PcmWriter = {
            let track = Arc::clone(&outbound);
            Arc::new(move |pcm, duration_ms| {
                let track = Arc::clone(&track);
                let bytes = pcm;
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
        self.inner.attach(pcm_reader, pcm_writer).await.map_err(speech_err)?;
        self.state.lock().await.outbound = Some(outbound);
        Ok(())
    }

    #[napi]
    pub async fn start(&self) -> Result<()> {
        self.inner.start().await.map_err(speech_err)
    }

    #[napi]
    pub async fn stop(&self) -> Result<()> {
        self.inner.stop().await.map_err(speech_err)
    }

    #[napi]
    pub async fn send_text_to_tts(&self, text: String, non_blocking: Option<bool>) -> Result<()> {
        let options = SendTextToTtsOptions {
            non_blocking: non_blocking.unwrap_or(false),
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
}
