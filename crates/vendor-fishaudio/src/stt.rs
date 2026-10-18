//! Fish Audio realtime STT provider (`FinalizeMode::Deferred`).
//!
//! Protocol and docs: see `stt_matrix.rs`. Fish returns a transcript only after an explicit
//! commit and sends no partials, so the provider is a commit/answer machine:
//!
//! - `push_audio` forwards 16 kHz mono PCM over an unbounded channel and never drops it while
//!   the socket is connected.
//! - `request_finalize` sends `commit: true` and counts one pending finalize.
//! - Every pending finalize ends with exactly one [`SttOutput::FinalizeDone`], in FIFO order:
//!   after its `committed_transcript` (preceded by `Transcript(Final)` only when the text is
//!   non-empty), after a vendor error, or after the socket closed.
//! - A closed socket with a finalize pending or uncommitted audio sent surfaces one error,
//!   answers every pending finalize and marks the connection dead. An idle close (nothing
//!   lost) only marks it dead. Either way the next `push_audio` / `request_finalize`
//!   reconnects and replays nothing.
//!
//! Transcript text is never logged or put into an error message.
#![cfg_attr(not(feature = "live"), allow(dead_code))]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{FinalizeMode, SttOutput, SttProvider, SttTranscript};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::client::vendor_error;
use crate::stt_matrix::{
    default_stt_model, validate_stt_model, ServerMessage, STT_CLOSE_TIMEOUT,
    STT_PROACTIVE_RECONNECT,
};

#[cfg(feature = "live")]
use crate::matrix::CONNECT_TIMEOUT;
#[cfg(feature = "live")]
use crate::stt_matrix::{input_audio_chunk_json, parse_server_message, realtime_stt_url};

/// What the sender task writes to the socket.
enum Outbound {
    Audio(Bytes),
    Commit,
}

/// One queued item for `poll_output`.
enum Item {
    Output(SttOutput),
    /// A vendor error, surfaced as `Err` from the next `poll_output`.
    Failure(String),
}

#[derive(Default)]
struct State {
    /// Bumped on every connect and close; tasks of an older generation are ignored.
    generation: u64,
    alive: bool,
    outbound: Option<mpsc::UnboundedSender<Outbound>>,
    queue: VecDeque<Item>,
    /// Finalizes sent and not yet answered, oldest first.
    pending: usize,
    connected_at: Option<Instant>,
    /// Audio pushed on the current connection since the last commit. A socket close loses it
    /// (so it is an error), and a proactive reconnect never splits that utterance.
    uncommitted: bool,
}

struct Shared {
    state: Mutex<State>,
    /// `true` from the start of a connect until `session_started` (or a failure).
    open_pending: AtomicBool,
    /// `true` once the current connection received `session_started`.
    ready: watch::Sender<bool>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_ready(&self, ready: bool) {
        self.ready.send_replace(ready);
    }

    fn on_message(&self, generation: u64, message: ServerMessage) {
        let mut state = self.lock();
        if state.generation != generation || !state.alive {
            return;
        }
        match message {
            ServerMessage::SessionStarted => {
                self.open_pending.store(false, Ordering::SeqCst);
                self.set_ready(true);
            }
            ServerMessage::Committed(text) => {
                // A commit answer without a pending commit cannot be attributed: drop it.
                if state.pending == 0 {
                    return;
                }
                state.pending -= 1;
                let text = text.trim();
                if !text.is_empty() {
                    state.queue.push_back(Item::Output(SttOutput::Transcript(
                        SttTranscript::Final(text.to_string()),
                    )));
                }
                state.queue.push_back(Item::Output(SttOutput::FinalizeDone));
            }
            ServerMessage::Error(message) => {
                self.open_pending.store(false, Ordering::SeqCst);
                state.queue.push_back(Item::Failure(message));
                // The error answers the oldest pending commit, so it still gets its terminal.
                if state.pending > 0 {
                    state.pending -= 1;
                    state.queue.push_back(Item::Output(SttOutput::FinalizeDone));
                }
            }
            ServerMessage::Other => {}
        }
    }

    /// The socket closed or failed. With a finalize pending or uncommitted audio sent, something
    /// was lost: one error, then one `FinalizeDone` per pending finalize. An idle close (nothing
    /// pending, nothing uncommitted) only marks the connection dead; the next push reconnects.
    fn connection_lost(&self, generation: u64, message: String) {
        let mut state = self.lock();
        if state.generation != generation || !state.alive {
            return;
        }
        state.alive = false;
        state.outbound = None;
        state.connected_at = None;
        self.open_pending.store(false, Ordering::SeqCst);
        self.set_ready(false);
        if state.pending == 0 && !state.uncommitted {
            return;
        }
        state.queue.push_back(Item::Failure(message));
        for _ in 0..state.pending {
            state.queue.push_back(Item::Output(SttOutput::FinalizeDone));
        }
        state.pending = 0;
        state.uncommitted = false;
    }
}

struct Tasks {
    reader: JoinHandle<()>,
    sender: JoinHandle<()>,
}

pub struct FishAudioStt {
    api_key: Option<String>,
    model: String,
    endpoint: Option<String>,
    shared: Arc<Shared>,
    tasks: Option<Tasks>,
    running: bool,
}

impl FishAudioStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        // The only STT transport is the WebSocket; an unsupported value fails the config here.
        crate::stt_matrix::resolve_stt_transport(config.transport)?;
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| default_stt_model().to_string());
        validate_stt_model(&model).map_err(SpeechError::Config)?;
        let (ready, _) = watch::channel(false);
        Ok(Self {
            api_key: config.api_key.clone(),
            model,
            endpoint: config.endpoint.clone(),
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                open_pending: AtomicBool::new(false),
                ready,
            }),
            tasks: None,
            running: false,
        })
    }

    fn api_key(&self) -> SpeechResult<String> {
        self.api_key
            .clone()
            .filter(|key| !key.is_empty())
            .or_else(|| std::env::var("FISH_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing FISH_API_KEY".into()))
    }

    /// Closes the current connection: a polite close frame under [`STT_CLOSE_TIMEOUT`], never
    /// waiting for the server's answer. Late events of the old connection are ignored.
    async fn close_connection(&mut self) {
        {
            let mut state = self.shared.lock();
            state.generation += 1;
            state.alive = false;
            // Dropping the sender ends the sender task, which then sends the close frame.
            state.outbound = None;
            state.connected_at = None;
        }
        self.shared.open_pending.store(false, Ordering::SeqCst);
        self.shared.set_ready(false);
        if let Some(tasks) = self.tasks.take() {
            let mut sender = tasks.sender;
            if tokio::time::timeout(STT_CLOSE_TIMEOUT, &mut sender)
                .await
                .is_err()
            {
                sender.abort();
            }
            tasks.reader.abort();
        }
    }

    #[cfg(feature = "live")]
    async fn connect(&mut self) -> SpeechResult<()> {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::{
            connect_async,
            tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
        };

        let api_key = self.api_key()?;
        self.close_connection().await;

        let url = realtime_stt_url(self.endpoint.as_deref(), &self.model);
        let mut request = url
            .into_client_request()
            .map_err(|err| vendor_error(format!("fishaudio stt request invalid: {err}")))?;
        request.headers_mut().insert(
            "xi-api-key",
            HeaderValue::from_str(&api_key).map_err(|_| {
                SpeechError::Config("fishaudio header value has invalid characters".into())
            })?,
        );

        let generation = self.shared.lock().generation;
        self.shared.open_pending.store(true, Ordering::SeqCst);
        self.shared.set_ready(false);
        let ws = match tokio::time::timeout(CONNECT_TIMEOUT, connect_async(request)).await {
            Err(_) => {
                self.shared.open_pending.store(false, Ordering::SeqCst);
                return Err(vendor_error(format!(
                    "fishaudio stt connect timed out after {} ms",
                    CONNECT_TIMEOUT.as_millis()
                )));
            }
            Ok(Err(err)) => {
                self.shared.open_pending.store(false, Ordering::SeqCst);
                return Err(vendor_error(format!("fishaudio stt connect failed: {err}")));
            }
            Ok(Ok((ws, _))) => ws,
        };
        let (mut ws_tx, mut ws_rx) = ws.split();
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<Outbound>();

        {
            let mut state = self.shared.lock();
            state.alive = true;
            state.outbound = Some(outbound_tx);
            state.connected_at = Some(Instant::now());
            state.pending = 0;
            state.uncommitted = false;
        }

        let reader_shared = self.shared.clone();
        let reader = tokio::spawn(async move {
            loop {
                match ws_rx.next().await {
                    Some(Ok(Message::Text(text))) => {
                        reader_shared.on_message(generation, parse_server_message(&text));
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        reader_shared
                            .connection_lost(generation, "fishaudio stt socket closed".into());
                        return;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        reader_shared.connection_lost(
                            generation,
                            format!("fishaudio stt socket read failed: {err}"),
                        );
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
                    Outbound::Audio(pcm) => input_audio_chunk_json(
                        &base64::engine::general_purpose::STANDARD.encode(pcm),
                        false,
                    ),
                    Outbound::Commit => input_audio_chunk_json("", true),
                };
                if ws_tx.send(Message::Text(json.into())).await.is_err() {
                    sender_shared
                        .connection_lost(generation, "fishaudio stt socket send failed".into());
                    return;
                }
            }
            // The provider dropped its sender (stop / reconnect): say goodbye, don't wait.
            let _ = tokio::time::timeout(STT_CLOSE_TIMEOUT, ws_tx.send(Message::Close(None))).await;
        });

        self.tasks = Some(Tasks { reader, sender });
        Ok(())
    }

    #[cfg(not(feature = "live"))]
    async fn connect(&mut self) -> SpeechResult<()> {
        let _ = (&self.endpoint, &self.model, self.api_key()?);
        Err(vendor_error(
            "live Fish Audio STT requires `--features live` on vendor-fishaudio",
        ))
    }

    fn connection_state(&self) -> (bool, usize, bool, Option<Instant>) {
        let state = self.shared.lock();
        (
            state.alive,
            state.pending,
            state.uncommitted,
            state.connected_at,
        )
    }

    /// Reconnects a dead connection, and a connection that reached [`STT_PROACTIVE_RECONNECT`]
    /// while nothing is pending and no utterance is in progress.
    async fn ensure_connected(&mut self, allow_proactive: bool) -> SpeechResult<()> {
        let (alive, pending, uncommitted, connected_at) = self.connection_state();
        let aged = allow_proactive
            && alive
            && pending == 0
            && !uncommitted
            && connected_at.is_some_and(|at| at.elapsed() >= STT_PROACTIVE_RECONNECT);
        if !alive || aged {
            self.connect().await?;
        }
        Ok(())
    }
}

#[async_trait]
impl SttProvider for FishAudioStt {
    fn vendor_name(&self) -> &'static str {
        "fishaudio"
    }

    /// `transcribe-1` returns text only on `commit` (`Committed`), never an interim result.
    fn emits_partials(&self) -> bool {
        false
    }

    async fn start(&mut self) -> SpeechResult<()> {
        self.close_connection().await;
        {
            let mut state = self.shared.lock();
            state.queue.clear();
            state.pending = 0;
            state.uncommitted = false;
        }
        self.connect().await?;
        self.running = true;
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        self.running = false;
        self.close_connection().await;
        let mut state = self.shared.lock();
        state.queue.clear();
        state.pending = 0;
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        if !self.running {
            return Ok(());
        }
        self.ensure_connected(true).await?;
        let mut state = self.shared.lock();
        if let Some(tx) = &state.outbound {
            // A send error means the socket just died; the close handler already reported it.
            if tx.send(Outbound::Audio(pcm)).is_ok() {
                state.uncommitted = true;
            }
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        // `FinalizeDone` markers are for `poll_output` callers only.
        loop {
            let item = self.shared.lock().queue.pop_front();
            match item {
                None => return Ok(None),
                Some(Item::Failure(message)) => return Err(vendor_error(message)),
                Some(Item::Output(SttOutput::Transcript(transcript))) => {
                    return Ok(Some(transcript))
                }
                Some(Item::Output(SttOutput::FinalizeDone)) => {}
            }
        }
    }

    async fn poll_output(&mut self) -> SpeechResult<Option<SttOutput>> {
        let item = self.shared.lock().queue.pop_front();
        match item {
            None => Ok(None),
            Some(Item::Failure(message)) => Err(vendor_error(message)),
            Some(Item::Output(output)) => Ok(Some(output)),
        }
    }

    fn finalize_mode(&self) -> FinalizeMode {
        FinalizeMode::Deferred
    }

    async fn wait_ready(&mut self, timeout: std::time::Duration) -> SpeechResult<bool> {
        if !self.running || !self.shared.lock().alive {
            return Ok(false);
        }
        let mut ready = self.shared.ready.subscribe();
        let result = tokio::time::timeout(timeout, ready.wait_for(|ready| *ready)).await;
        Ok(matches!(result, Ok(Ok(_))))
    }

    fn stream_open_pending(&self) -> bool {
        self.shared.open_pending.load(Ordering::SeqCst)
    }

    fn discard_queued_transcripts(&mut self) -> usize {
        let mut state = self.shared.lock();
        let before = state.queue.len();
        state
            .queue
            .retain(|item| !matches!(item, Item::Output(SttOutput::Transcript(_))));
        before - state.queue.len()
    }

    async fn request_finalize(&mut self) -> SpeechResult<()> {
        if !self.running {
            return Err(vendor_error("fishaudio stt stream closed"));
        }
        // Reconnect a dead socket only. A proactive reconnect here would drop the audio of the
        // utterance that this commit ends.
        self.ensure_connected(false).await?;
        let mut state = self.shared.lock();
        // Sending and counting under one lock: either the close handler already ran (error
        // returned, nothing pending) or it runs later and answers this commit.
        let sent = state.alive
            && state
                .outbound
                .as_ref()
                .is_some_and(|tx| tx.send(Outbound::Commit).is_ok());
        if !sent {
            return Err(vendor_error("fishaudio stt stream closed"));
        }
        state.pending += 1;
        state.uncommitted = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::SttVendor;

    fn config(model: Option<&str>) -> SttConfig {
        SttConfig {
            provider: SttVendor::Fishaudio,
            model: model.map(str::to_string),
            model_path: None,
            language: None,
            api_key: Some("k".into()),
            endpoint: None,
            transport: None,
        }
    }

    fn started(shared: &Shared, pending: usize) -> u64 {
        let mut state = shared.lock();
        state.alive = true;
        state.pending = pending;
        state.generation
    }

    fn shared() -> Shared {
        let (ready, _) = watch::channel(false);
        Shared {
            state: Mutex::new(State::default()),
            open_pending: AtomicBool::new(false),
            ready,
        }
    }

    fn drain(shared: &Shared) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(item) = shared.lock().queue.pop_front() {
            out.push(match item {
                Item::Output(SttOutput::Transcript(SttTranscript::Final(_))) => "final".into(),
                Item::Output(SttOutput::Transcript(SttTranscript::Partial(_))) => "partial".into(),
                Item::Output(SttOutput::FinalizeDone) => "done".into(),
                Item::Failure(_) => "error".into(),
            });
        }
        out
    }

    #[test]
    fn new_validates_the_model() {
        assert!(FishAudioStt::new(&config(None)).is_ok());
        assert!(FishAudioStt::new(&config(Some("transcribe-1"))).is_ok());
        for model in ["transcribe-1-pro", "scribe_v2_realtime"] {
            assert!(matches!(
                FishAudioStt::new(&config(Some(model))),
                Err(SpeechError::Config(_))
            ));
        }
    }

    #[test]
    fn provider_never_emits_partials() {
        let stt = FishAudioStt::new(&config(None)).unwrap();
        assert!(!stt.emits_partials());
    }

    #[test]
    fn provider_is_deferred() {
        let stt = FishAudioStt::new(&config(None)).unwrap();
        assert_eq!(stt.finalize_mode(), FinalizeMode::Deferred);
        assert_eq!(stt.vendor_name(), "fishaudio");
        assert!(!stt.stream_open_pending());
    }

    #[test]
    fn commit_answers_in_order_and_skips_empty_text() {
        let shared = shared();
        let generation = started(&shared, 2);
        shared.on_message(generation, ServerMessage::Committed("   ".into()));
        shared.on_message(generation, ServerMessage::Committed("hello".into()));
        assert_eq!(drain(&shared), ["done", "final", "done"]);
        // A third answer has no pending commit and is dropped.
        shared.on_message(generation, ServerMessage::Committed("stray".into()));
        assert!(drain(&shared).is_empty());
    }

    #[test]
    fn error_answers_the_oldest_pending_commit() {
        let shared = shared();
        let generation = started(&shared, 2);
        shared.on_message(generation, ServerMessage::Error("bad".into()));
        assert_eq!(drain(&shared), ["error", "done"]);
        assert_eq!(shared.lock().pending, 1);
    }

    #[test]
    fn lost_connection_answers_every_pending_commit_once() {
        let shared = shared();
        let generation = started(&shared, 3);
        shared.connection_lost(generation, "closed".into());
        shared.connection_lost(generation, "closed again".into());
        assert_eq!(drain(&shared), ["error", "done", "done", "done"]);
        assert!(!shared.lock().alive);
    }

    #[test]
    fn idle_loss_is_quiet_but_uncommitted_audio_is_an_error() {
        let shared = shared();
        let generation = started(&shared, 0);
        shared.connection_lost(generation, "closed".into());
        assert!(drain(&shared).is_empty());
        assert!(!shared.lock().alive);

        let generation = started(&shared, 0);
        shared.lock().uncommitted = true;
        shared.connection_lost(generation, "closed".into());
        assert_eq!(drain(&shared), ["error"]);
        assert!(!shared.lock().uncommitted);
    }

    #[test]
    fn events_of_an_older_generation_are_ignored() {
        let shared = shared();
        let generation = started(&shared, 1);
        shared.lock().generation += 1;
        shared.on_message(generation, ServerMessage::Committed("late".into()));
        shared.connection_lost(generation, "late close".into());
        assert!(drain(&shared).is_empty());
        assert_eq!(shared.lock().pending, 1);
    }
}
