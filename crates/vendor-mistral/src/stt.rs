//! Mistral Voxtral realtime STT provider (`FinalizeMode::Deferred`).
//!
//! Protocol and docs: see `stt_matrix.rs`. One WebSocket session per utterance:
//!
//! - A session task connects, sends `session.update` once, and then serves commands from the
//!   provider over an unbounded channel. Audio pushed while it connects waits in that channel,
//!   in order, and is never dropped.
//! - After an utterance ends (`transcription.done` or an error) the next session is opened in
//!   the background (the "spare"), so the next utterance finds a ready socket. A spare that the
//!   server closes before use is dropped quietly; the next push opens a new one.
//! - `request_finalize` flushes the buffered audio and sends `input_audio.end`. The session's
//!   `transcription.done` is the final. Every finalize ends with exactly one
//!   [`SttOutput::FinalizeDone`], in request order, whether the answer is the final, an error,
//!   a closed socket or the done timeout. A finalize for an utterance without audio is
//!   answered at once, without the network.
//! - A socket that closes or fails with a claimed utterance (audio pushed) or a pending
//!   finalize surfaces exactly one named error. The next push opens a new session.
//!
//! Transcript text is never logged or put into an error message.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use base64::Engine;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{SpeechTransport, SttConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{FinalizeMode, SttOutput, SttProvider, SttTranscript};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue, Message};

use crate::error::vendor_error;
use crate::stt_matrix::{
    append_json, parse_server_event, realtime_stt_url, session_update_json, validate_stt_model,
    ServerEvent, SttTimeouts, APPEND_MAX_BYTES, APPEND_TARGET_BYTES, DEFAULT_STT_MODEL, END_JSON,
};

/// What the provider tells a session task.
#[derive(Clone)]
enum Cmd {
    /// The provider adopted this session for the next utterance. `after` is the id of the last
    /// finalize registered so far: this utterance's partials wait for it.
    Claim {
        after: u64,
    },
    Audio(Bytes),
    End {
        ticket: u64,
    },
    Close,
}

enum Item {
    Output(SttOutput),
    /// A vendor error, surfaced as `Err` from the next `poll_output`.
    Failure(String),
}

struct Slot {
    id: u64,
    answer: Option<Vec<Item>>,
}

struct Session {
    tx: mpsc::UnboundedSender<Cmd>,
    handle: JoinHandle<()>,
}

#[derive(Default)]
struct State {
    running: bool,
    next_generation: u64,
    /// Sessions older than this never write output (after a stop / restart).
    stale_before: u64,
    queue: VecDeque<Item>,
    /// Output of a later utterance that must not overtake an unanswered finalize.
    held: VecDeque<(u64, Item)>,
    tickets: VecDeque<Slot>,
    last_ticket: u64,
    spare: Option<Session>,
    last_transport: Option<String>,
}

impl State {
    /// Finalize ids below which everything is answered; `u64::MAX` when none is outstanding.
    fn blocking(&self) -> u64 {
        self.tickets.front().map_or(u64::MAX, |slot| slot.id)
    }

    fn emit(&mut self, after: u64, item: Item) {
        if after < self.blocking() {
            self.queue.push_back(item);
        } else {
            self.held.push_back((after, item));
        }
    }

    /// Releases answered finalizes in request order, then the held output they unblocked.
    fn flush(&mut self) {
        while self
            .tickets
            .front()
            .is_some_and(|slot| slot.answer.is_some())
        {
            let slot = self.tickets.pop_front().expect("front checked");
            self.queue.extend(slot.answer.into_iter().flatten());
            self.queue.push_back(Item::Output(SttOutput::FinalizeDone));
        }
        let blocking = self.blocking();
        let held = std::mem::take(&mut self.held);
        for (after, item) in held {
            if after < blocking {
                self.queue.push_back(item);
            } else {
                self.held.push_back((after, item));
            }
        }
    }

    fn register_ticket(&mut self) -> u64 {
        self.last_ticket += 1;
        self.tickets.push_back(Slot {
            id: self.last_ticket,
            answer: None,
        });
        self.last_ticket
    }

    fn answer(&mut self, ticket: u64, items: Vec<Item>) {
        if let Some(slot) = self.tickets.iter_mut().find(|slot| slot.id == ticket) {
            slot.answer = Some(items);
            self.flush();
        }
    }
}

struct Shared {
    api_key: Option<String>,
    url: String,
    language: Option<String>,
    timeouts: SttTimeouts,
    state: Mutex<State>,
    /// Sessions still connecting (connect + `session.update`).
    connecting: watch::Sender<usize>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn api_key(&self) -> SpeechResult<String> {
        self.api_key
            .clone()
            .filter(|key| !key.is_empty())
            .or_else(|| std::env::var("MISTRAL_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing MISTRAL_API_KEY".into()))
    }

    fn spawn_session(self: &Arc<Self>) -> SpeechResult<Session> {
        let api_key = self.api_key()?;
        let (tx, rx) = mpsc::unbounded_channel();
        let generation = {
            let mut state = self.lock();
            state.next_generation += 1;
            state.next_generation
        };
        self.connecting.send_modify(|n| *n += 1);
        let handle = tokio::spawn(run_session(self.clone(), generation, api_key, rx));
        Ok(Session { tx, handle })
    }

    /// Opens the spare session for the next utterance when none is alive.
    fn ensure_spare(self: &Arc<Self>) -> SpeechResult<()> {
        {
            let state = self.lock();
            if !state.running || state.spare.as_ref().is_some_and(|s| !s.tx.is_closed()) {
                return Ok(());
            }
        }
        let session = self.spawn_session()?;
        let mut state = self.lock();
        if state.running {
            state.spare = Some(session);
        } else {
            let _ = session.tx.send(Cmd::Close);
        }
        Ok(())
    }

    fn emit(&self, generation: u64, after: u64, item: Item) {
        let mut state = self.lock();
        if generation >= state.stale_before {
            state.emit(after, item);
        }
    }

    fn answer(&self, generation: u64, ticket: u64, items: Vec<Item>) {
        let mut state = self.lock();
        if generation >= state.stale_before {
            state.answer(ticket, items);
        }
    }
}

/// Per-session bookkeeping inside the task.
#[derive(Default)]
struct Local {
    claimed: bool,
    after: u64,
    ticket: Option<u64>,
}

impl Local {
    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Claim { after } => {
                self.claimed = true;
                self.after = after;
            }
            Cmd::Audio(_) => self.claimed = true,
            Cmd::End { ticket } => self.ticket = Some(ticket),
            Cmd::Close => {}
        }
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(shared: &Shared, api_key: &str) -> Result<Ws, String> {
    let mut request = shared
        .url
        .clone()
        .into_client_request()
        .map_err(|err| format!("mistral stt request invalid: {err}"))?;
    let value = HeaderValue::from_str(&format!("Bearer {api_key}"))
        .map_err(|_| "mistral stt api key has invalid characters".to_string())?;
    request.headers_mut().insert("Authorization", value);
    match tokio::time::timeout(
        shared.timeouts.connect,
        tokio_tungstenite::connect_async(request),
    )
    .await
    {
        Err(_) => Err(format!(
            "mistral stt connect timed out after {} ms",
            shared.timeouts.connect.as_millis()
        )),
        Ok(Err(err)) => Err(format!("mistral stt connect failed: {err}")),
        Ok(Ok((ws, _))) => Ok(ws),
    }
}

/// Ends a session on a failure. The command channel is closed first and drained, so every
/// command the provider managed to send is accounted for (provider sends after the close fail,
/// and the provider answers those itself). An unused spare fails quietly; a claimed session or a
/// pending finalize gets exactly one error, and the finalize its answer.
fn die(
    shared: &Shared,
    generation: u64,
    rx: &mut mpsc::UnboundedReceiver<Cmd>,
    local: &mut Local,
    message: String,
) {
    rx.close();
    while let Ok(cmd) = rx.try_recv() {
        local.apply(cmd);
    }
    if let Some(ticket) = local.ticket {
        shared.answer(generation, ticket, vec![Item::Failure(message)]);
    } else if local.claimed {
        shared.emit(generation, local.after, Item::Failure(message));
    }
}

async fn send_text(ws_tx: &mut WsTx, shared: &Shared, text: String) -> Result<(), String> {
    match tokio::time::timeout(
        shared.timeouts.connect,
        ws_tx.send(Message::Text(text.into())),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err("mistral stt websocket send failed".to_string()),
        Err(_) => Err("mistral stt websocket send timed out".to_string()),
    }
}

/// Sends `buf` as appends of at most [`APPEND_MAX_BYTES`].
async fn close_ws(ws_tx: &mut WsTx, shared: &Shared) {
    let _ = tokio::time::timeout(shared.timeouts.close, ws_tx.send(Message::Close(None))).await;
}

type WsTx = futures_util::stream::SplitSink<Ws, Message>;
type WsRx = futures_util::stream::SplitStream<Ws>;

/// Connects and sends the one `session.update`. The caller has counted this attempt in
/// `connecting` and uncounts it afterwards.
async fn open(shared: &Shared, api_key: &str) -> Result<(WsTx, WsRx), String> {
    let ws = connect(shared, api_key).await?;
    let (mut ws_tx, ws_rx) = ws.split();
    send_text(
        &mut ws_tx,
        shared,
        session_update_json(shared.language.as_deref()),
    )
    .await?;
    shared.lock().last_transport = Some("websocket".into());
    Ok((ws_tx, ws_rx))
}

/// How one socket's run ended.
enum Exit {
    /// The session is over (final, error event, timeout, close request).
    Finished,
    /// The socket closed or a read or write failed.
    Lost(String),
}

/// One utterance's state, kept across a replay on a second socket.
struct Run {
    shared: Arc<Shared>,
    generation: u64,
    rx: mpsc::UnboundedReceiver<Cmd>,
    local: Local,
    /// Audio received and not yet sent.
    buf: Vec<u8>,
    text: String,
    deadline: Option<Instant>,
    rx_open: bool,
    /// Every PCM byte sent for this utterance, kept until the first server event that belongs
    /// to it arrives: if the socket dies before that, the server never answered anything and
    /// the utterance can be sent again on a new socket, once.
    replay: Vec<u8>,
    event_seen: bool,
    replayed: bool,
    /// Commands taken off the channel while deciding on a replay; handled first on the next
    /// socket.
    pending: VecDeque<Cmd>,
}

impl Run {
    fn drain_pending(&mut self) {
        while let Ok(cmd) = self.rx.try_recv() {
            self.local.apply(cmd.clone());
            self.pending.push_back(cmd);
        }
    }

    fn fail(&mut self, message: String) {
        die(
            &self.shared,
            self.generation,
            &mut self.rx,
            &mut self.local,
            message,
        );
    }

    /// Sends the buffered audio as appends of at most [`APPEND_MAX_BYTES`].
    async fn flush(&mut self, ws_tx: &mut WsTx) -> Result<(), String> {
        while !self.buf.is_empty() {
            let take = self.buf.len().min(APPEND_MAX_BYTES);
            let chunk: Vec<u8> = self.buf.drain(..take).collect();
            if !self.event_seen {
                self.replay.extend_from_slice(&chunk);
            }
            let encoded = base64::engine::general_purpose::STANDARD.encode(chunk);
            send_text(ws_tx, &self.shared, append_json(&encoded)).await?;
        }
        Ok(())
    }

    /// New socket after a loss: the saved audio again, then `input_audio.end` when the
    /// finalize was already sent.
    async fn restore(&mut self, ws_tx: &mut WsTx) -> Result<(), String> {
        for chunk in self.replay.chunks(APPEND_MAX_BYTES) {
            let encoded = base64::engine::general_purpose::STANDARD.encode(chunk);
            send_text(ws_tx, &self.shared, append_json(&encoded)).await?;
        }
        if self.local.ticket.is_some() {
            send_text(ws_tx, &self.shared, END_JSON.to_string()).await?;
            self.deadline = Some(Instant::now() + self.shared.timeouts.done);
        }
        Ok(())
    }

    /// `Ok(true)` ends the session task.
    async fn command(&mut self, ws_tx: &mut WsTx, cmd: Option<Cmd>) -> Result<bool, String> {
        match cmd {
            // The provider drops its end of the channel once it has sent `End`; the session
            // then still waits for its `transcription.done`.
            None if self.local.ticket.is_some() => self.rx_open = false,
            None | Some(Cmd::Close) => {
                close_ws(ws_tx, &self.shared).await;
                return Ok(true);
            }
            Some(Cmd::Audio(pcm)) => {
                self.local.claimed = true;
                self.buf.extend_from_slice(&pcm);
                if self.buf.len() >= APPEND_TARGET_BYTES {
                    self.flush(ws_tx).await?;
                }
            }
            Some(Cmd::End { ticket }) => {
                self.local.ticket = Some(ticket);
                self.flush(ws_tx).await?;
                send_text(ws_tx, &self.shared, END_JSON.to_string()).await?;
                self.deadline = Some(Instant::now() + self.shared.timeouts.done);
            }
            Some(other) => self.local.apply(other),
        }
        Ok(false)
    }

    /// The first server event that belongs to the utterance: from now on a loss is an error.
    fn note_event(&mut self) {
        if self.local.claimed && !self.event_seen {
            self.event_seen = true;
            self.replay = Vec::new();
        }
    }

    async fn serve(&mut self, ws_tx: &mut WsTx, ws_rx: &mut WsRx) -> Exit {
        while let Some(cmd) = self.pending.pop_front() {
            match self.command(ws_tx, Some(cmd)).await {
                Ok(false) => {}
                Ok(true) => return Exit::Finished,
                Err(message) => return Exit::Lost(message),
            }
        }
        loop {
            let deadline = self.deadline;
            let timer = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            let rx_open = self.rx_open;
            tokio::select! {
                cmd = self.rx.recv(), if rx_open => {
                    match self.command(ws_tx, cmd).await {
                        Ok(false) => {}
                        Ok(true) => return Exit::Finished,
                        Err(message) => return Exit::Lost(message),
                    }
                }
                message = ws_rx.next() => match message {
                    Some(Ok(Message::Text(raw))) => match parse_server_event(&raw) {
                        ServerEvent::Updated => self.note_event(),
                        ServerEvent::Delta(delta) => {
                            self.note_event();
                            if self.local.claimed {
                                self.text.push_str(&delta);
                                let partial = self.text.trim();
                                if !partial.is_empty() {
                                    self.shared.emit(
                                        self.generation,
                                        self.local.after,
                                        Item::Output(SttOutput::Transcript(
                                            SttTranscript::Partial(partial.to_string()),
                                        )),
                                    );
                                }
                            }
                        }
                        ServerEvent::Done(done_text) => {
                            self.note_event();
                            let Some(ticket) = self.local.ticket else { continue };
                            let final_text = done_text.unwrap_or_else(|| self.text.clone());
                            let final_text = final_text.trim();
                            let mut items = Vec::new();
                            if !final_text.is_empty() {
                                items.push(Item::Output(SttOutput::Transcript(
                                    SttTranscript::Final(final_text.to_string()),
                                )));
                            }
                            self.shared.answer(self.generation, ticket, items);
                            close_ws(ws_tx, &self.shared).await;
                            let _ = self.shared.ensure_spare();
                            return Exit::Finished;
                        }
                        ServerEvent::Error(message) => {
                            self.note_event();
                            let used = self.local.claimed || self.local.ticket.is_some();
                            self.fail(message);
                            if used {
                                close_ws(ws_tx, &self.shared).await;
                                let _ = self.shared.ensure_spare();
                            }
                            return Exit::Finished;
                        }
                        ServerEvent::Other => {}
                    },
                    // Ping, Pong, binary and unknown frames never end the reader.
                    Some(Ok(Message::Close(_))) | None => {
                        return Exit::Lost("mistral stt websocket closed".into());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        return Exit::Lost(format!(
                            "mistral stt websocket closed: read failed: {err}"
                        ));
                    }
                },
                () = timer => {
                    let message = format!(
                        "mistral stt transcription.done timed out after {} ms",
                        self.shared.timeouts.done.as_millis()
                    );
                    self.fail(message);
                    close_ws(ws_tx, &self.shared).await;
                    return Exit::Finished;
                }
            }
        }
    }
}

async fn run_session(
    shared: Arc<Shared>,
    generation: u64,
    api_key: String,
    rx: mpsc::UnboundedReceiver<Cmd>,
) {
    let mut run = Run {
        shared: shared.clone(),
        generation,
        rx,
        local: Local::default(),
        buf: Vec::new(),
        text: String::new(),
        deadline: None,
        rx_open: true,
        replay: Vec::new(),
        event_seen: false,
        replayed: false,
        pending: VecDeque::new(),
    };
    let mut conn = open(&shared, &api_key).await;
    shared.connecting.send_modify(|n| *n -= 1);
    loop {
        let (mut ws_tx, mut ws_rx) = match conn {
            Ok(conn) => conn,
            Err(message) => {
                run.fail(message);
                return;
            }
        };
        let message = match run.serve(&mut ws_tx, &mut ws_rx).await {
            Exit::Finished => return,
            Exit::Lost(message) => message,
        };
        // Nothing the server said belongs to this utterance yet: the socket died before it
        // answered anything, so send the utterance again, once, on a fresh socket.
        run.drain_pending();
        if !run.local.claimed || run.event_seen || run.replayed {
            run.fail(message);
            return;
        }
        run.replayed = true;
        shared.connecting.send_modify(|n| *n += 1);
        conn = open(&shared, &api_key).await;
        shared.connecting.send_modify(|n| *n -= 1);
        if let Ok((tx, _)) = &mut conn {
            if let Err(message) = run.restore(tx).await {
                conn = Err(message);
            }
        }
    }
}

pub struct MistralStt {
    shared: Arc<Shared>,
    /// The session serving the utterance in progress (claimed by the first push).
    current: Option<Session>,
}

impl MistralStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        Self::with_timeouts(config, SttTimeouts::default())
    }

    /// Same as [`MistralStt::new`] with explicit bounds (tests shorten them).
    pub fn with_timeouts(config: &SttConfig, timeouts: SttTimeouts) -> SpeechResult<Self> {
        if config.transport == Some(SpeechTransport::Http) {
            return Err(SpeechError::Config(
                "mistral stt supports only the websocket transport".into(),
            ));
        }
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_STT_MODEL.to_string());
        validate_stt_model(&model)?;
        let (connecting, _) = watch::channel(0);
        Ok(Self {
            shared: Arc::new(Shared {
                api_key: config.api_key.clone(),
                url: realtime_stt_url(config.endpoint.as_deref(), &model),
                language: config.language.clone(),
                timeouts,
                state: Mutex::new(State::default()),
                connecting,
            }),
            current: None,
        })
    }

    /// Adopts the spare (or a new session) for the utterance that starts now.
    fn claim_session(&mut self) -> SpeechResult<()> {
        for _ in 0..2 {
            let spare = self.shared.lock().spare.take();
            let session = match spare {
                Some(session) => session,
                None => self.shared.spawn_session()?,
            };
            let after = self.shared.lock().last_ticket;
            if session.tx.send(Cmd::Claim { after }).is_ok() {
                self.current = Some(session);
                return Ok(());
            }
        }
        Err(vendor_error("mistral stt websocket closed"))
    }

    fn has_live_session(&self) -> bool {
        self.current.as_ref().is_some_and(|s| !s.tx.is_closed())
            || self
                .shared
                .lock()
                .spare
                .as_ref()
                .is_some_and(|s| !s.tx.is_closed())
    }
}

#[async_trait]
impl SttProvider for MistralStt {
    fn vendor_name(&self) -> &'static str {
        "mistral"
    }

    /// `transcription.text.delta` events are the partials.
    fn emits_partials(&self) -> bool {
        true
    }

    fn finalize_mode(&self) -> FinalizeMode {
        FinalizeMode::Deferred
    }

    fn last_transport(&self) -> Option<String> {
        self.shared.lock().last_transport.clone()
    }

    async fn start(&mut self) -> SpeechResult<()> {
        self.stop().await?;
        // A missing key is a config error now, not after the first utterance.
        self.shared.api_key()?;
        self.shared.lock().running = true;
        // The first socket opens in the background; audio pushed meanwhile waits for it.
        self.shared.ensure_spare()
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let mut sessions: Vec<Session> = self.current.take().into_iter().collect();
        {
            let mut state = self.shared.lock();
            state.running = false;
            state.stale_before = state.next_generation + 1;
            state.queue.clear();
            state.held.clear();
            state.tickets.clear();
            sessions.extend(state.spare.take());
        }
        let close = self.shared.timeouts.close;
        for session in sessions {
            let _ = session.tx.send(Cmd::Close);
            let mut handle = session.handle;
            if tokio::time::timeout(close * 2, &mut handle).await.is_err() {
                handle.abort();
            }
        }
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        if !self.shared.lock().running {
            return Ok(());
        }
        // A finished or failed session (its failure is already queued) is replaced: the next
        // push opens a new one.
        if self.current.as_ref().is_none_or(|s| s.tx.is_closed()) {
            self.current = None;
            self.claim_session()?;
        }
        if let Some(session) = &self.current {
            // A send error means the session just died; its task reported the failure.
            let _ = session.tx.send(Cmd::Audio(pcm));
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

    async fn request_finalize(&mut self) -> SpeechResult<()> {
        let mut state = self.shared.lock();
        if !state.running {
            return Err(vendor_error("mistral stt stream closed"));
        }
        let ticket = state.register_ticket();
        let session = self.current.take();
        // No audio this utterance (or its session already died and reported): answer at once.
        let sent = session
            .as_ref()
            .is_some_and(|s| s.tx.send(Cmd::End { ticket }).is_ok());
        if !sent {
            state.answer(ticket, Vec::new());
        }
        Ok(())
    }

    async fn wait_ready(&mut self, timeout: std::time::Duration) -> SpeechResult<bool> {
        if !self.shared.lock().running {
            return Ok(false);
        }
        if !self.has_live_session() {
            self.shared.ensure_spare()?;
        }
        let mut connecting = self.shared.connecting.subscribe();
        let settled = tokio::time::timeout(timeout, connecting.wait_for(|n| *n == 0)).await;
        Ok(matches!(settled, Ok(Ok(_))) && self.has_live_session())
    }

    fn stream_open_pending(&self) -> bool {
        *self.shared.connecting.borrow() > 0
    }

    fn discard_queued_transcripts(&mut self) -> usize {
        let mut state = self.shared.lock();
        let is_transcript = |item: &Item| matches!(item, Item::Output(SttOutput::Transcript(_)));
        let before = state.queue.len() + state.held.len();
        state.queue.retain(|item| !is_transcript(item));
        state.held.retain(|(_, item)| !is_transcript(item));
        before - state.queue.len() - state.held.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::SttVendor;

    fn config(model: Option<&str>) -> SttConfig {
        SttConfig {
            provider: SttVendor::Mock,
            model: model.map(str::to_string),
            model_path: None,
            language: None,
            api_key: Some("k".into()),
            endpoint: None,
            transport: None,
        }
    }

    fn kinds(state: &mut State) -> Vec<&'static str> {
        state
            .queue
            .drain(..)
            .map(|item| match item {
                Item::Output(SttOutput::Transcript(SttTranscript::Final(_))) => "final",
                Item::Output(SttOutput::Transcript(SttTranscript::Partial(_))) => "partial",
                Item::Output(SttOutput::FinalizeDone) => "done",
                Item::Failure(_) => "error",
            })
            .collect()
    }

    fn partial(text: &str) -> Item {
        Item::Output(SttOutput::Transcript(SttTranscript::Partial(text.into())))
    }

    #[tokio::test]
    async fn new_validates_the_model_and_transport() {
        assert!(MistralStt::new(&config(None)).is_ok());
        assert!(MistralStt::new(&config(Some("voxtral-mini-transcribe-realtime-3"))).is_ok());
        assert!(matches!(
            MistralStt::new(&config(Some("voxtral-mini-transcribe-realtime-latest"))),
            Err(SpeechError::Config(_))
        ));
        let mut http = config(None);
        http.transport = Some(SpeechTransport::Http);
        assert!(matches!(
            MistralStt::new(&http),
            Err(SpeechError::Config(_))
        ));
        let stt = MistralStt::new(&config(None)).unwrap();
        assert!(stt.emits_partials());
        assert_eq!(stt.finalize_mode(), FinalizeMode::Deferred);
        assert_eq!(stt.vendor_name(), "mistral");
    }

    #[test]
    fn finalizes_are_answered_in_request_order() {
        let mut state = State::default();
        let first = state.register_ticket();
        let second = state.register_ticket();
        // The second answers first; nothing is released until the first is answered.
        state.answer(second, vec![]);
        assert!(kinds(&mut state).is_empty());
        state.answer(first, vec![Item::Failure("x".into())]);
        assert_eq!(kinds(&mut state), ["error", "done", "done"]);
    }

    #[test]
    fn output_of_a_later_utterance_waits_for_an_earlier_finalize() {
        let mut state = State::default();
        let first = state.register_ticket();
        // Claimed after the first finalize was registered: waits for it.
        state.emit(first, partial("next"));
        // Claimed before it: delivered at once.
        state.emit(first - 1, partial("same"));
        assert_eq!(kinds(&mut state), ["partial"]);
        state.answer(first, vec![]);
        assert_eq!(kinds(&mut state), ["done", "partial"]);
    }

    #[test]
    fn answering_an_unknown_ticket_is_a_no_op() {
        let mut state = State::default();
        state.answer(7, vec![]);
        assert!(kinds(&mut state).is_empty());
    }
}
