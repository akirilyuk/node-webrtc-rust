//! Deepgram live STT provider (`FinalizeMode::Deferred`).
//!
//! Deepgram sends `is_final` segments while the speaker is still talking (a finalized segment
//! of the audio so far, "long utterances may have multiple `is_final: true` responses before
//! `speech_final: true`") and `speech_final` at its own endpoint. Neither is the end of the
//! agent's utterance, which the voice agent decides (VAD hangover). So:
//!
//! - `is_final` segments are concatenated into the utterance in progress and shown as
//!   partials, together with the interim text.
//! - The agent's `request_finalize` sends the documented `Finalize` message (flush: "forces the
//!   server to immediately process any unprocessed audio data and return the final
//!   transcription results"). The answer carries `from_finalize: true`; it closes the
//!   utterance: one `Final` with every segment in order, then `FinalizeDone`.
//! - Deepgram does not guarantee that answer ("not guaranteed if there is no significant
//!   amount of audio data to process"), so a finalize that gets none within
//!   [`FINALIZE_WAIT_MS`] is closed by the provider with what it has.
//! - A `Results` message that ends before the audio pushed at the last finalize, arriving when
//!   no finalize is pending, belongs to a closed utterance and is dropped (`start` and
//!   `duration` are seconds of audio since the stream began).
//!
//! Transcript text is never logged or put into an error message.
#![cfg_attr(not(feature = "live"), allow(dead_code))]

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{FinalizeMode, SttOutput, SttProvider, SttTranscript};
use node_webrtc_rust_speech::stt_deferred::DeferredFinals;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::matrix::{
    default_listen_model, listen_websocket_url_at, resolve_stt_transport, ListenResults,
};

#[cfg(feature = "live")]
use crate::matrix::{parse_listen_results, FINALIZE_MESSAGE};
#[cfg(feature = "live")]
use futures_util::{SinkExt, StreamExt};
#[cfg(feature = "live")]
use tokio_tungstenite::tungstenite::Message;

/// Longest wait for the `from_finalize` answer to one finalize.
pub const FINALIZE_WAIT_MS: u64 = 5_000;
/// 16 kHz mono s16le.
const PCM_BYTES_PER_SECOND: f64 = 32_000.0;
/// Tolerance when comparing a result's end with the audio pushed at a finalize.
const BOUNDARY_EPS_S: f64 = 0.05;

enum Outbound {
    Audio(Bytes),
    Finalize,
}

struct State {
    alive: bool,
    outbound: Option<mpsc::UnboundedSender<Outbound>>,
    finals: DeferredFinals,
    /// Bytes pushed on this connection.
    pushed_bytes: u64,
    /// Seconds of audio pushed when the last finalize was requested.
    closed_boundary_s: f64,
    /// Results dropped as late (tests).
    late_dropped: u64,
}

pub(crate) struct Shared {
    state: Mutex<State>,
}

impl Shared {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(State {
                alive: false,
                outbound: None,
                finals: DeferredFinals::new(),
                pushed_bytes: 0,
                closed_boundary_s: 0.0,
                late_dropped: 0,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One `Results` message.
    pub(crate) fn on_results(&self, r: ListenResults) {
        let mut st = self.lock();
        if !st.alive {
            return;
        }
        if r.from_finalize {
            if st.finals.pending() == 0 {
                // The answer to a finalize the provider already closed.
                st.late_dropped += 1;
                return;
            }
            st.finals.segment(&r.text);
            st.finals.answer();
            return;
        }
        if r.text.is_empty() {
            return;
        }
        if st.finals.pending() == 0 && r.end_s <= st.closed_boundary_s + BOUNDARY_EPS_S {
            st.late_dropped += 1;
            return;
        }
        if r.is_final {
            st.finals.segment(&r.text);
        } else {
            st.finals.interim(&r.text);
        }
    }

    pub(crate) fn connection_lost(&self, message: String) {
        let mut st = self.lock();
        if !st.alive {
            return;
        }
        st.alive = false;
        st.outbound = None;
        st.finals.lost(message);
    }
}

pub struct DeepgramStt {
    api_key: Option<String>,
    model: String,
    language: Option<String>,
    endpoint: Option<String>,
    shared: Arc<Shared>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    running: bool,
    /// `ws` once the listen socket is open.
    last_transport: std::sync::Mutex<Option<String>>,
}

impl DeepgramStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        // The only STT transport is the WebSocket; an unsupported value fails the config here.
        resolve_stt_transport(config.transport)?;
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| default_listen_model().to_string());
        crate::matrix::validate_listen_model(&model).map_err(SpeechError::Config)?;
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("DEEPGRAM_API_KEY").ok()),
            model,
            language: config.language.clone(),
            endpoint: config.endpoint.clone(),
            shared: Arc::new(Shared::new()),
            tasks: Vec::new(),
            running: false,
            last_transport: std::sync::Mutex::new(None),
        })
    }

    fn api_key(&self) -> SpeechResult<String> {
        self.api_key
            .clone()
            .filter(|key| !key.is_empty())
            .or_else(|| std::env::var("DEEPGRAM_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing DEEPGRAM_API_KEY".into()))
    }

    fn abort_tasks(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

#[async_trait]
impl SttProvider for DeepgramStt {
    fn vendor_name(&self) -> &'static str {
        "deepgram"
    }

    fn last_transport(&self) -> Option<String> {
        self.last_transport.lock().ok().and_then(|g| g.clone())
    }

    fn finalize_mode(&self) -> FinalizeMode {
        FinalizeMode::Deferred
    }

    /// The vendor closes a socket that gets no audio for a while; the agent gates audio away
    /// between utterances. `false` after such a close, so the STT actor reopens the stream
    /// before the next audio (Deepgram would otherwise drop it and the turn ends with no final).
    fn stream_alive(&self) -> bool {
        !self.running || self.shared.lock().alive
    }

    fn finalize_wait_ms(&self) -> u64 {
        FINALIZE_WAIT_MS
    }

    async fn start(&mut self) -> SpeechResult<()> {
        #[cfg(feature = "live")]
        {
            let api_key = self.api_key()?;
            let url = listen_websocket_url_at(
                self.endpoint.as_deref(),
                &self.model,
                self.language.as_deref(),
            )
            .map_err(SpeechError::Config)?;

            self.abort_tasks();
            let ws = crate::ws_connect::connect(url, &api_key, "stt").await?;
            if let Ok(mut guard) = self.last_transport.lock() {
                *guard = Some("ws".into());
            }
            let (mut ws_tx, mut ws_rx) = ws.split();
            let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<Outbound>();
            {
                let mut st = self.shared.lock();
                st.alive = true;
                st.outbound = Some(outbound_tx);
                st.finals.clear();
                st.pushed_bytes = 0;
                st.closed_boundary_s = 0.0;
            }

            let reader_shared = self.shared.clone();
            let reader = tokio::spawn(async move {
                loop {
                    match ws_rx.next().await {
                        Some(Ok(Message::Text(text))) => {
                            if let Some(results) = parse_listen_results(&text) {
                                reader_shared.on_results(results);
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            reader_shared.connection_lost("deepgram stt socket closed".into());
                            return;
                        }
                        Some(Ok(_)) => {}
                        Some(Err(err)) => {
                            reader_shared
                                .connection_lost(format!("deepgram stt socket read failed: {err}"));
                            return;
                        }
                    }
                }
            });

            let sender_shared = self.shared.clone();
            let sender = tokio::spawn(async move {
                while let Some(outbound) = outbound_rx.recv().await {
                    let message = match outbound {
                        Outbound::Audio(chunk) => Message::Binary(chunk),
                        Outbound::Finalize => Message::Text(FINALIZE_MESSAGE.into()),
                    };
                    if ws_tx.send(message).await.is_err() {
                        sender_shared.connection_lost("deepgram stt socket send failed".into());
                        return;
                    }
                }
                let _ = ws_tx.send(Message::Close(None)).await;
            });
            self.tasks = vec![reader, sender];
            self.running = true;
            return Ok(());
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = (self.api_key()?, &self.endpoint);
            Err(SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: "live Deepgram STT requires `--features live` on vendor-deepgram".into(),
            })
        }
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        self.running = false;
        {
            let mut st = self.shared.lock();
            st.alive = false;
            st.outbound = None;
            st.finals.clear();
        }
        self.abort_tasks();
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        if !self.running {
            return Ok(());
        }
        let mut st = self.shared.lock();
        let len = pcm.len() as u64;
        if let Some(tx) = &st.outbound {
            if tx.send(Outbound::Audio(pcm)).is_ok() {
                st.pushed_bytes += len;
            }
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let mut st = self.shared.lock();
        st.finals
            .expire(Instant::now(), Duration::from_millis(FINALIZE_WAIT_MS));
        st.finals.next_transcript()
    }

    async fn poll_output(&mut self) -> SpeechResult<Option<SttOutput>> {
        let mut st = self.shared.lock();
        st.finals
            .expire(Instant::now(), Duration::from_millis(FINALIZE_WAIT_MS));
        st.finals.next_output()
    }

    fn discard_queued_transcripts(&mut self) -> usize {
        self.shared.lock().finals.discard_transcripts()
    }

    async fn request_finalize(&mut self) -> SpeechResult<()> {
        if !self.running {
            return Err(SpeechError::Stt("deepgram stt stream closed".into()));
        }
        let mut st = self.shared.lock();
        if !st.alive {
            return Err(SpeechError::Stt("deepgram stt stream closed".into()));
        }
        let pushed_s = st.pushed_bytes as f64 / PCM_BYTES_PER_SECOND;
        let had_audio = pushed_s > st.closed_boundary_s + BOUNDARY_EPS_S;
        st.closed_boundary_s = pushed_s;
        st.finals.request(Instant::now());
        if !had_audio && !st.finals.has_segments() {
            // Nothing was pushed since the last finalize: no flush to wait for.
            st.finals.answer();
            return Ok(());
        }
        let sent = st
            .outbound
            .as_ref()
            .is_some_and(|tx| tx.send(Outbound::Finalize).is_ok());
        if !sent {
            st.finals.answer();
            return Err(SpeechError::Stt("deepgram stt stream closed".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::SttVendor;

    #[test]
    fn rejects_undocumented_model_at_new() {
        assert!(matches!(
            DeepgramStt::new(&SttConfig {
                provider: SttVendor::Deepgram,
                model: Some("flux-general".into()),
                model_path: None,
                language: None,
                api_key: Some("k".into()),
                endpoint: None,
                transport: None,
            }),
            Err(SpeechError::Config(_))
        ));
    }
}
