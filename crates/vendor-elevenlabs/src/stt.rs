//! ElevenLabs Scribe v2 Realtime STT provider (`FinalizeMode::Deferred`).
//!
//! The socket uses `commit_strategy=manual`: "you control when to commit transcript segments".
//! A `commit` ends a segment and the server answers with a `committed_transcript`, "the final
//! results of the transcription segment that are sent when a 'commit' message is received". That
//! answer arrives some time after the commit, so the provider cannot end the voice agent's
//! utterance when it sends the commit (the old `Inline` provider did, and the agent closed the
//! utterance as `user_stt_not_found` before the transcript came):
//!
//! - `partial_transcript` is shown as a partial (together with the segments so far).
//! - `request_finalize` sends the commit and counts one pending finalize.
//! - The `committed_transcript` that answers it is the utterance's one `Final` (when non-empty),
//!   followed by `FinalizeDone`. One that arrives with no finalize pending is the model's own
//!   commit ("the model automatically commits after approximately 36 seconds of accumulated
//!   audio"); it joins the utterance in progress.
//! - The reference does not say whether a commit over no usable speech is answered, so a
//!   finalize with no answer within [`FINALIZE_WAIT_MS`] is closed by the provider with what it
//!   has. A `commit_throttled` error answers the oldest pending commit.
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

use crate::stt_matrix::{
    default_stt_model, resolve_stt_transport, validate_stt_model, ServerMessage,
};

#[cfg(feature = "live")]
use crate::stt_matrix::{input_audio_chunk_json, parse_server_message, realtime_websocket_url_at};
#[cfg(feature = "live")]
use futures_util::{SinkExt, StreamExt};
#[cfg(feature = "live")]
use tokio_tungstenite::tungstenite::Message;

/// Longest wait for the `committed_transcript` that answers one commit.
pub const FINALIZE_WAIT_MS: u64 = 5_000;

enum Outbound {
    Audio(Bytes),
    Commit,
}

struct State {
    alive: bool,
    outbound: Option<mpsc::UnboundedSender<Outbound>>,
    finals: DeferredFinals,
    /// Audio pushed since the last commit.
    uncommitted: bool,
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
                uncommitted: false,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn on_message(&self, message: ServerMessage) {
        let mut st = self.lock();
        if !st.alive {
            return;
        }
        match message {
            ServerMessage::Partial(text) => st.finals.interim(&text),
            ServerMessage::Committed(text) => {
                st.finals.segment(&text);
                // Without a pending finalize this is the model's own commit: the text stays
                // with the utterance in progress.
                st.finals.answer();
            }
            ServerMessage::Error {
                answers_commit,
                message,
            } => {
                // Every documented error is surfaced; only a throttled commit is known to be the
                // answer to a commit.
                if answers_commit {
                    st.finals.fail(message);
                } else {
                    st.finals.fail_without_answer(message);
                }
            }
            ServerMessage::Other => {}
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

pub struct ElevenLabsStt {
    api_key: Option<String>,
    model: String,
    language: Option<String>,
    endpoint: Option<String>,
    shared: Arc<Shared>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    running: bool,
    /// `ws` once the realtime socket is open.
    last_transport: std::sync::Mutex<Option<String>>,
}

impl ElevenLabsStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        // The only STT transport is the WebSocket; an unsupported value fails the config here.
        resolve_stt_transport(config.transport)?;
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| default_stt_model().to_string());
        validate_stt_model(&model).map_err(SpeechError::Config)?;
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("ELEVENLABS_API_KEY").ok()),
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
            .or_else(|| std::env::var("ELEVENLABS_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing ELEVENLABS_API_KEY".into()))
    }

    fn abort_tasks(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

#[async_trait]
impl SttProvider for ElevenLabsStt {
    fn vendor_name(&self) -> &'static str {
        "elevenlabs"
    }

    fn last_transport(&self) -> Option<String> {
        self.last_transport.lock().ok().and_then(|g| g.clone())
    }

    fn finalize_mode(&self) -> FinalizeMode {
        FinalizeMode::Deferred
    }

    fn finalize_wait_ms(&self) -> u64 {
        FINALIZE_WAIT_MS
    }

    async fn start(&mut self) -> SpeechResult<()> {
        #[cfg(feature = "live")]
        {
            let api_key = self.api_key()?;
            let url = realtime_websocket_url_at(
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
                st.uncommitted = false;
            }

            let reader_shared = self.shared.clone();
            let reader = tokio::spawn(async move {
                loop {
                    match ws_rx.next().await {
                        Some(Ok(Message::Text(text))) => {
                            reader_shared.on_message(parse_server_message(&text));
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            reader_shared.connection_lost("elevenlabs stt socket closed".into());
                            return;
                        }
                        Some(Ok(_)) => {}
                        Some(Err(err)) => {
                            reader_shared.connection_lost(format!(
                                "elevenlabs stt socket read failed: {err}"
                            ));
                            return;
                        }
                    }
                }
            });

            let sender_shared = self.shared.clone();
            let sender = tokio::spawn(async move {
                use base64::Engine;
                while let Some(outbound) = outbound_rx.recv().await {
                    let json = match outbound {
                        Outbound::Audio(pcm) => {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(pcm);
                            input_audio_chunk_json(&b64, false)
                        }
                        Outbound::Commit => input_audio_chunk_json("", true),
                    };
                    if ws_tx.send(Message::Text(json.into())).await.is_err() {
                        sender_shared.connection_lost("elevenlabs stt socket send failed".into());
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
                vendor: "elevenlabs".into(),
                message: "live ElevenLabs STT requires `--features live` on vendor-elevenlabs"
                    .into(),
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
        if let Some(tx) = &st.outbound {
            if tx.send(Outbound::Audio(pcm)).is_ok() {
                st.uncommitted = true;
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
            return Err(SpeechError::Stt("elevenlabs stt stream closed".into()));
        }
        let mut st = self.shared.lock();
        if !st.alive {
            return Err(SpeechError::Stt("elevenlabs stt stream closed".into()));
        }
        st.finals.request(Instant::now());
        if !st.uncommitted && !st.finals.has_segments() {
            // Nothing was pushed since the last commit: there is no segment to commit.
            st.finals.answer();
            return Ok(());
        }
        let sent = st
            .outbound
            .as_ref()
            .is_some_and(|tx| tx.send(Outbound::Commit).is_ok());
        if !sent {
            st.finals.answer();
            return Err(SpeechError::Stt("elevenlabs stt stream closed".into()));
        }
        st.uncommitted = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stt_matrix::STT_PCM_SAMPLE_RATE;
    use node_webrtc_rust_speech::config::SttVendor;

    #[test]
    fn rejects_undocumented_model_at_new() {
        assert!(matches!(
            ElevenLabsStt::new(&SttConfig {
                provider: SttVendor::Elevenlabs,
                model: Some("scribe_v1".into()),
                model_path: None,
                language: None,
                api_key: Some("k".into()),
                endpoint: None,
                transport: None,
            }),
            Err(SpeechError::Config(_))
        ));
    }

    #[test]
    fn stt_sample_rate_matches_matrix() {
        assert_eq!(STT_PCM_SAMPLE_RATE, 16_000);
    }
}
