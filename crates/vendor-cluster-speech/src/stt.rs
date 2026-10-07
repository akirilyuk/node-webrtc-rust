use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use node_webrtc_rust_speech::config::{SttConfig, VoiceSessionContext};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use node_webrtc_rust_speech_proto::v1::speech_client::SpeechClient;
use node_webrtc_rust_speech_proto::v1::{
    transcribe_request, ModelRef, SessionContext, SttAudio, SttFinalize, SttStop, TranscribeRequest,
};
use tokio::sync::{mpsc, watch, Mutex, Notify};
use tokio::task::JoinHandle;
use tonic::metadata::MetadataValue;
use tonic::Request;

use crate::channel::{
    resolve_speech_token, resolve_stt_endpoint, stt_channel, MAX_GRPC_MESSAGE_BYTES,
};
use crate::metrics::{
    add_stt_queued_bytes, inc_stt_reopen, inc_stt_utterance_streams, record_stt_stream_open_ms,
    sub_stt_queued_bytes,
};

const COALESCE_MAX_BYTES: usize = 1920;
/// 16 kHz mono s16le: 32 bytes per millisecond.
const STT_BYTES_PER_MS: usize = 32;
/// First queued-audio warning per utterance.
const QUEUED_WARN_MS: usize = 2_000;
/// Error-level queued-audio line per utterance.
const QUEUED_ERROR_MS: usize = 10_000;
/// Upper bound of the finalize wait added for audio still queued behind the pod.
const FINALIZE_QUEUED_WAIT_CAP_MS: usize = 30_000;
const READY_WAIT_MS: u64 = 15_000;
const FINALIZE_WAIT_MS: u64 = 2_000;
/// Longest `finalize_utterance` waits for a refused stream open (a speech pod at its stream
/// cap) to succeed before sending the Finalize anyway. Override with `SPEECH_STT_OPEN_WAIT_MAX_MS`.
const OPEN_WAIT_MAX_MS: u64 = 120_000;
/// Per-utterance mode: an open stream that sees no audio for this long (and has no finalize
/// pending) is closed, which covers a finalize that never came.
const PER_UTTERANCE_IDLE_CLOSE_MS: u64 = 30_000;
/// While the open is denied, print a progress line at most this often.
const DENIED_LOG_INTERVAL: Duration = Duration::from_secs(5);

/// Environment switch for per-utterance Transcribe streams (`1` / `true` / `yes` / `on` = on,
/// default off).
pub const STREAM_PER_UTTERANCE_ENV: &str = "CLUSTER_STT_STREAM_PER_UTTERANCE";

fn parse_switch(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Whether `CLUSTER_STT_STREAM_PER_UTTERANCE` is on. Read once per process.
pub fn stream_per_utterance_from_env() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| parse_switch(std::env::var(STREAM_PER_UTTERANCE_ENV).ok().as_deref()))
}

/// Options for [`ClusterSherpaStt`]. [`ClusterSherpaStt::new`] uses [`ClusterSttOptions::from_env`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClusterSttOptions {
    /// Open one Transcribe gRPC stream per utterance instead of one stream for the whole
    /// session. The stream is closed after each `Finalized` and re-opened on the next audio, so
    /// the load balancer and the pods' admission cap place every turn. The first stream still
    /// opens at `start()`. Default `false` (`Default`); env `CLUSTER_STT_STREAM_PER_UTTERANCE`.
    pub stream_per_utterance: bool,
}

impl ClusterSttOptions {
    /// Options from the process environment (`CLUSTER_STT_STREAM_PER_UTTERANCE`, read once).
    pub fn from_env() -> Self {
        Self {
            stream_per_utterance: stream_per_utterance_from_env(),
        }
    }
}

/// What to log for one denied stream open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeniedLog {
    /// First denial of the streak: print the failure message.
    First,
    /// Periodic progress line (`still denied after {attempts} attempts`).
    Still { attempts: u32 },
    /// Stay quiet.
    Silent,
}

/// Tracks a streak of denied stream opens so the log is not flooded on every 200 ms retry.
#[derive(Debug, Default)]
pub(crate) struct OpenStreak {
    attempts: u32,
    started: Option<Instant>,
    last_log: Option<Instant>,
}

impl OpenStreak {
    /// Record a denied open and decide what to print.
    pub(crate) fn mark_denied(&mut self, now: Instant) -> DeniedLog {
        self.attempts += 1;
        if self.started.is_none() {
            self.started = Some(now);
            self.last_log = Some(now);
            return DeniedLog::First;
        }
        let due = self
            .last_log
            .map(|t| now.duration_since(t) >= DENIED_LOG_INTERVAL)
            .unwrap_or(true);
        if due {
            self.last_log = Some(now);
            DeniedLog::Still {
                attempts: self.attempts,
            }
        } else {
            DeniedLog::Silent
        }
    }

    /// The stream became ready: returns `(denied attempts, elapsed ms)` when the streak had
    /// at least one denial, and resets the streak.
    pub(crate) fn mark_ready(&mut self, now: Instant) -> Option<(u32, u64)> {
        let out = self
            .started
            .map(|t| (self.attempts, now.duration_since(t).as_millis() as u64));
        *self = Self::default();
        out
    }
}

enum StreamCommand {
    Audio(Bytes),
    Finalize { done: Arc<Notify> },
    Stop,
}

pub(crate) fn session_context_proto(ctx: &VoiceSessionContext) -> SessionContext {
    SessionContext {
        session_id: ctx.session_id.clone().unwrap_or_default(),
        project_id: ctx.project_id.clone().unwrap_or_default(),
        org_id: ctx.org_id.clone().unwrap_or_default(),
        trace_id: ctx.trace_id.clone().unwrap_or_default(),
        build_id: ctx.build_id.clone().unwrap_or_default(),
        traceparent: ctx.traceparent.clone().unwrap_or_default(),
    }
}

pub(crate) fn auth_metadata(
    token: &Option<String>,
) -> SpeechResult<MetadataValue<tonic::metadata::Ascii>> {
    let t = token
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| SpeechError::Config("missing SPEECH_SERVICE_TOKEN".into()))?;
    format!("Bearer {t}")
        .parse()
        .map_err(|_| SpeechError::Config("invalid SPEECH_SERVICE_TOKEN".into()))
}

pub struct ClusterSherpaStt {
    cfg: SttConfig,
    endpoint: String,
    token: Option<String>,
    session_ctx: Arc<Mutex<Option<VoiceSessionContext>>>,
    inner: Arc<Mutex<ClusterSherpaSttInner>>,
    /// Bytes handed to the stream worker and not yet written to the pod (this session).
    queued_bytes: Arc<AtomicUsize>,
    queued_warned: AtomicBool,
    queued_error_logged: AtomicBool,
    stream_per_utterance: bool,
    /// `true` from the start of a stream open until the server answers Ready, including while a
    /// refused open is retried (e.g. a speech pod at its stream cap).
    open_pending: Arc<AtomicBool>,
}

/// Subtract `n` from `counter` without wrapping; returns what was actually removed.
fn sub_queued_saturating(counter: &AtomicUsize, n: usize) -> usize {
    let mut removed = 0;
    let _ = counter.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |cur| {
        removed = n.min(cur);
        Some(cur - removed)
    });
    removed
}

/// Drop whatever this session still had queued (stream ended or stopped).
fn clear_queued(counter: &AtomicUsize) {
    let left = counter.swap(0, Ordering::SeqCst);
    sub_stt_queued_bytes(left);
}

struct ClusterSherpaSttInner {
    running: bool,
    transcript_rx: Option<mpsc::UnboundedReceiver<SttTranscript>>,
    cmd_tx: Option<mpsc::UnboundedSender<StreamCommand>>,
    stream_task: Option<JoinHandle<()>>,
    /// `true` once the current stream received `Ready`; `false` while (re)opening or after exit.
    ready_rx: Option<watch::Receiver<bool>>,
    pending_audio: BytesMut,
    utterance_active: bool,
    mid_utterance_failure: Option<String>,
}

impl ClusterSherpaStt {
    /// New STT client; options come from the environment ([`ClusterSttOptions::from_env`]).
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        Self::new_with_options(config, ClusterSttOptions::from_env())
    }

    pub fn new_with_options(config: &SttConfig, options: ClusterSttOptions) -> SpeechResult<Self> {
        let endpoint = resolve_stt_endpoint(config)?;
        let token = resolve_speech_token(&config.api_key);
        Ok(Self {
            cfg: config.clone(),
            endpoint,
            token,
            session_ctx: Arc::new(Mutex::new(None)),
            inner: Arc::new(Mutex::new(ClusterSherpaSttInner {
                running: false,
                transcript_rx: None,
                cmd_tx: None,
                stream_task: None,
                ready_rx: None,
                pending_audio: BytesMut::new(),
                utterance_active: false,
                mid_utterance_failure: None,
            })),
            queued_bytes: Arc::new(AtomicUsize::new(0)),
            queued_warned: AtomicBool::new(false),
            queued_error_logged: AtomicBool::new(false),
            stream_per_utterance: options.stream_per_utterance,
            open_pending: Arc::new(AtomicBool::new(false)),
        })
    }

    async fn spawn_stream_task(&self) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        if inner.stream_task.is_some() {
            return Ok(());
        }
        let (transcript_tx, transcript_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let cfg = self.cfg.clone();
        let endpoint = self.endpoint.clone();
        let token = self.token.clone();
        let session_ctx = Arc::clone(&self.session_ctx);
        let (ready_tx, ready_rx) = watch::channel(false);
        let queued_bytes = Arc::clone(&self.queued_bytes);
        let per_utterance = self.stream_per_utterance;
        let open_pending = Arc::clone(&self.open_pending);
        // Pending from the moment the worker is spawned, so a finalize that runs before the
        // worker's first open attempt still waits for the open.
        open_pending.store(true, Ordering::SeqCst);
        let task = tokio::spawn(async move {
            stream_worker(
                WorkerParams {
                    cfg,
                    endpoint,
                    token,
                    per_utterance,
                    open_pending: Arc::clone(&open_pending),
                },
                session_ctx,
                cmd_rx,
                transcript_tx,
                &ready_tx,
                Arc::clone(&queued_bytes),
            )
            .await;
            clear_queued(&queued_bytes);
            open_pending.store(false, Ordering::SeqCst);
            ready_tx.send_replace(false);
        });
        inner.ready_rx = Some(ready_rx);
        inner.transcript_rx = Some(transcript_rx);
        inner.cmd_tx = Some(cmd_tx);
        inner.stream_task = Some(task);
        Ok(())
    }

    async fn flush_pending_audio(&self, force: bool) -> SpeechResult<()> {
        let pcm = {
            let mut inner = self.inner.lock().await;
            if inner.pending_audio.is_empty() {
                return Ok(());
            }
            if !force && inner.pending_audio.len() < COALESCE_MAX_BYTES {
                return Ok(());
            }
            inner.pending_audio.split().freeze()
        };
        {
            let inner = self.inner.lock().await;
            let Some(cmd_tx) = inner.cmd_tx.as_ref() else {
                return Ok(());
            };
            let len = pcm.len();
            self.queued_bytes.fetch_add(len, Ordering::SeqCst);
            add_stt_queued_bytes(len);
            if cmd_tx.send(StreamCommand::Audio(pcm)).is_err() {
                let removed = sub_queued_saturating(&self.queued_bytes, len);
                sub_stt_queued_bytes(removed);
                return Err(SpeechError::Vendor {
                    vendor: "cluster-sherpa".into(),
                    message: "STT stream closed".into(),
                });
            }
        }
        let queued_ms = self.queued_bytes.load(Ordering::SeqCst) / STT_BYTES_PER_MS;
        if queued_ms >= QUEUED_WARN_MS && !self.queued_warned.swap(true, Ordering::SeqCst) {
            eprintln!(
                "[cluster-sherpa] stt audio queued {queued_ms} ms behind the speech pod; pool needs more capacity"
            );
        }
        if queued_ms >= QUEUED_ERROR_MS && !self.queued_error_logged.swap(true, Ordering::SeqCst) {
            eprintln!(
                "[cluster-sherpa] ERROR stt audio queued {queued_ms} ms behind the speech pod"
            );
        }
        Ok(())
    }
}

struct WorkerParams {
    cfg: SttConfig,
    endpoint: String,
    token: Option<String>,
    /// One Transcribe stream per utterance (see [`ClusterSttOptions::stream_per_utterance`]).
    per_utterance: bool,
    /// `true` from the start of a stream open until the server answers Ready, including while a
    /// refused open is retried (see `SttProvider::stream_open_pending`).
    open_pending: Arc<AtomicBool>,
}

/// Write one audio chunk to the open stream and release its queued-bytes accounting.
/// Returns `false` when the stream's request side is gone.
async fn send_audio(
    req_tx: &mpsc::Sender<TranscribeRequest>,
    pcm: Bytes,
    queued_bytes: &AtomicUsize,
) -> bool {
    let pcm_len = pcm.len();
    let audio = TranscribeRequest {
        msg: Some(transcribe_request::Msg::Audio(SttAudio { pcm_s16le: pcm })),
    };
    let sent = req_tx.send(audio).await;
    let removed = sub_queued_saturating(queued_bytes, pcm_len);
    sub_stt_queued_bytes(removed);
    sent.is_ok()
}

async fn stream_worker(
    params: WorkerParams,
    session_ctx: Arc<Mutex<Option<VoiceSessionContext>>>,
    mut cmd_rx: mpsc::UnboundedReceiver<StreamCommand>,
    transcript_tx: mpsc::UnboundedSender<SttTranscript>,
    ready_tx: &watch::Sender<bool>,
    queued_bytes: Arc<AtomicUsize>,
) {
    let WorkerParams {
        cfg,
        endpoint,
        token,
        per_utterance,
        open_pending,
    } = params;
    ready_tx.send_replace(false);
    // One channel for the whole worker: it multiplexes HTTP/2 streams (one per Transcribe call)
    // and the load balancer places each new stream.
    let channel = match stt_channel(&endpoint).await {
        Ok(c) => c,
        Err(e) => {
            let _ = transcript_tx.send(SttTranscript::Final(String::new()));
            eprintln!("[cluster-sherpa] connect failed: {e}");
            return;
        }
    };
    let mut pending_finalize: Option<Arc<Notify>> = None;
    let mut reopen = true;
    let mut streak = OpenStreak::default();
    // Why the next stream opens: `session_start` | `utterance` | `reopen` (metric attribute).
    let mut open_reason = "session_start";
    // Audio that arrived while idle; sent right after Start once the open succeeded.
    let mut carry: Option<Bytes> = None;
    while reopen {
        reopen = false;
        if open_reason != "utterance" {
            ready_tx.send_replace(false);
        }
        let mut client =
            SpeechClient::new(channel.clone()).max_decoding_message_size(MAX_GRPC_MESSAGE_BYTES);
        let (req_tx, req_rx) = mpsc::channel(32);
        let model_path = cfg
            .model_path
            .clone()
            .unwrap_or_else(|| "/models/sherpa/stt/default".to_string());
        let language = cfg.language.clone().unwrap_or_else(|| "en".to_string());
        let ctx = session_ctx
            .lock()
            .await
            .as_ref()
            .map(session_context_proto)
            .unwrap_or_default();
        let start = TranscribeRequest {
            msg: Some(transcribe_request::Msg::Start(
                node_webrtc_rust_speech_proto::v1::SttStart {
                    model: Some(ModelRef {
                        model_path,
                        catalog_id: String::new(),
                    }),
                    language,
                    ctx: Some(ctx),
                },
            )),
        };
        if req_tx.send(start).await.is_err() {
            break;
        }

        let mut request = Request::new(tokio_stream::wrappers::ReceiverStream::new(req_rx));
        if let Ok(auth) = auth_metadata(&token) {
            request.metadata_mut().insert("authorization", auth);
        }
        open_pending.store(true, Ordering::SeqCst);
        let open_started = Instant::now();
        let mut grpc = match client.transcribe(request).await {
            Ok(resp) => resp.into_inner(),
            Err(status) => {
                inc_stt_reopen("idle_error");
                match streak.mark_denied(Instant::now()) {
                    DeniedLog::First => {
                        eprintln!("[cluster-sherpa] transcribe open: {}", status.message());
                    }
                    DeniedLog::Still { attempts } => {
                        eprintln!("[cluster-sherpa] transcribe open still denied after {attempts} attempts");
                    }
                    DeniedLog::Silent => {}
                }
                open_pending.store(true, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(200)).await;
                reopen = true;
                continue;
            }
        };
        if open_reason == "utterance" {
            inc_stt_utterance_streams();
        }

        let ready_deadline = tokio::time::sleep(Duration::from_millis(READY_WAIT_MS));
        tokio::pin!(ready_deadline);
        let idle_close = Duration::from_millis(PER_UTTERANCE_IDLE_CLOSE_MS);
        let idle_deadline = tokio::time::sleep(idle_close);
        tokio::pin!(idle_deadline);
        let mut ready = false;
        let mut utterance_open = false;
        // Audio sent since the last Finalize was sent (per-utterance close decision).
        let mut audio_since_finalize = false;
        // Close this stream and wait idle for the next audio (per-utterance mode only).
        let mut go_idle = false;

        if let Some(pcm) = carry.take() {
            utterance_open = true;
            audio_since_finalize = true;
            if !send_audio(&req_tx, pcm, &queued_bytes).await {
                break;
            }
        }

        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(StreamCommand::Audio(pcm)) => {
                            utterance_open = true;
                            audio_since_finalize = true;
                            idle_deadline
                                .as_mut()
                                .reset(tokio::time::Instant::now() + idle_close);
                            if !send_audio(&req_tx, pcm, &queued_bytes).await {
                                break;
                            }
                        }
                        Some(StreamCommand::Finalize { done }) => {
                            audio_since_finalize = false;
                            idle_deadline
                                .as_mut()
                                .reset(tokio::time::Instant::now() + idle_close);
                            pending_finalize = Some(done);
                            let fin = TranscribeRequest {
                                msg: Some(transcribe_request::Msg::Finalize(SttFinalize {})),
                            };
                            if req_tx.send(fin).await.is_err() {
                                if let Some(done) = pending_finalize.take() {
                                    done.notify_one();
                                }
                                break;
                            }
                        }
                        Some(StreamCommand::Stop) | None => {
                            let stop = TranscribeRequest {
                                msg: Some(transcribe_request::Msg::Stop(SttStop {})),
                            };
                            let _ = req_tx.send(stop).await;
                            return;
                        }
                    }
                }
                msg = grpc.message() => {
                    match msg {
                        Ok(Some(resp)) => {
                            use node_webrtc_rust_speech_proto::v1::transcribe_response::Msg as RMsg;
                            match resp.msg {
                                Some(RMsg::Ready(_)) => {
                                    ready = true;
                                    open_pending.store(false, Ordering::SeqCst);
                                    record_stt_stream_open_ms(
                                        open_reason,
                                        open_started.elapsed().as_millis() as u64,
                                    );
                                    if let Some((n, ms)) = streak.mark_ready(Instant::now()) {
                                        eprintln!("[cluster-sherpa] transcribe open ok after {n} denied attempts ({ms} ms)");
                                    }
                                    ready_tx.send_replace(true);
                                }
                                Some(RMsg::Transcript(t)) => {
                                    let tr = if t.is_final {
                                        SttTranscript::Final(t.text)
                                    } else {
                                        SttTranscript::Partial(t.text)
                                    };
                                    let _ = transcript_tx.send(tr);
                                }
                                Some(RMsg::Finalized(_)) => {
                                    if let Some(done) = pending_finalize.take() {
                                        done.notify_one();
                                    }
                                    utterance_open = false;
                                    // No audio after the Finalize that was just answered: this
                                    // utterance is over, release the stream.
                                    if per_utterance && !audio_since_finalize {
                                        go_idle = true;
                                        break;
                                    }
                                }
                                Some(RMsg::Relocate(_)) => {
                                    inc_stt_reopen("relocate");
                                    open_reason = "reopen";
                                    reopen = true;
                                    break;
                                }
                                Some(RMsg::Error(e)) => {
                                    if utterance_open {
                                        let _ = transcript_tx.send(SttTranscript::Final(String::new()));
                                    } else {
                                        inc_stt_reopen("idle_error");
                                        open_reason = "reopen";
                                        reopen = true;
                                    }
                                    eprintln!("[cluster-sherpa] stt error: {} {}", e.code, e.message);
                                    break;
                                }
                                None => {}
                            }
                        }
                        Ok(None) => {
                            if !utterance_open {
                                inc_stt_reopen("goaway");
                                open_reason = "reopen";
                                reopen = true;
                            }
                            break;
                        }
                        Err(status) => {
                            if utterance_open {
                                let _ = transcript_tx.send(SttTranscript::Final(String::new()));
                            } else {
                                if status.code() == tonic::Code::Unavailable {
                                    inc_stt_reopen("goaway");
                                } else {
                                    inc_stt_reopen("idle_error");
                                }
                                open_reason = "reopen";
                                reopen = true;
                            }
                            break;
                        }
                    }
                }
                () = &mut ready_deadline, if !ready => {
                    eprintln!("[cluster-sherpa] ready timeout");
                    break;
                }
                // Per-utterance safety net: no audio for a long time and nothing to finalize.
                () = &mut idle_deadline, if per_utterance && ready && pending_finalize.is_none() => {
                    go_idle = true;
                    break;
                }
            }
        }

        if go_idle {
            // End the client stream (Stop + drop) and wait without a stream for the next command.
            let stop = TranscribeRequest {
                msg: Some(transcribe_request::Msg::Stop(SttStop {})),
            };
            let _ = req_tx.try_send(stop);
            drop(req_tx);
            drop(grpc);
            loop {
                match cmd_rx.recv().await {
                    Some(StreamCommand::Audio(pcm)) => {
                        carry = Some(pcm);
                        open_reason = "utterance";
                        reopen = true;
                        break;
                    }
                    // Nothing is open, so there is nothing to finalize.
                    Some(StreamCommand::Finalize { done }) => done.notify_one(),
                    Some(StreamCommand::Stop) | None => return,
                }
            }
        }
    }
}

#[async_trait]
impl SttProvider for ClusterSherpaStt {
    fn vendor_name(&self) -> &'static str {
        "cluster-sherpa"
    }

    fn bind_session_context(&self, ctx: &VoiceSessionContext) {
        if let Ok(mut guard) = self.session_ctx.try_lock() {
            *guard = Some(ctx.clone());
        }
    }

    async fn start(&mut self) -> SpeechResult<()> {
        self.spawn_stream_task().await?;
        let mut inner = self.inner.lock().await;
        inner.running = true;
        inner.pending_audio.clear();
        inner.utterance_active = false;
        inner.mid_utterance_failure = None;
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        inner.running = false;
        if let Some(tx) = inner.cmd_tx.take() {
            let _ = tx.send(StreamCommand::Stop);
        }
        if let Some(task) = inner.stream_task.take() {
            let _ = task.await;
        }
        clear_queued(&self.queued_bytes);
        inner.transcript_rx = None;
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        {
            let mut inner = self.inner.lock().await;
            if !inner.running {
                return Ok(());
            }
            inner.utterance_active = true;
            inner.pending_audio.extend_from_slice(pcm.as_ref());
        }
        self.flush_pending_audio(false).await?;
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let mut inner = self.inner.lock().await;
        if let Some(err) = inner.mid_utterance_failure.take() {
            return Err(SpeechError::Vendor {
                vendor: "cluster-sherpa".into(),
                message: err,
            });
        }
        if let Some(rx) = inner.transcript_rx.as_mut() {
            return Ok(rx.try_recv().ok());
        }
        Ok(None)
    }

    async fn wait_ready(&mut self, timeout: Duration) -> SpeechResult<bool> {
        let rx = self.inner.lock().await.ready_rx.clone();
        let Some(mut rx) = rx else {
            return Ok(false);
        };
        let ready = matches!(
            tokio::time::timeout(timeout, rx.wait_for(|r| *r)).await,
            Ok(Ok(_))
        );
        // Timed out, or the stream task exited (sender dropped) => not ready.
        Ok(ready)
    }

    fn decode_backlog_ms(&self) -> u32 {
        (self.queued_bytes.load(Ordering::SeqCst) / STT_BYTES_PER_MS) as u32
    }

    fn stream_open_pending(&self) -> bool {
        self.open_pending.load(Ordering::SeqCst)
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        self.flush_pending_audio(true).await?;
        // The stream open is being refused (pod at its stream cap): the audio stays queued, so
        // wait for the open instead of letting the Finalize wait below expire.
        if self.open_pending.load(Ordering::SeqCst) {
            let open_wait_max_ms: u64 = std::env::var("SPEECH_STT_OPEN_WAIT_MAX_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(OPEN_WAIT_MAX_MS);
            let started = Instant::now();
            while self.open_pending.load(Ordering::SeqCst) {
                if started.elapsed() >= Duration::from_millis(open_wait_max_ms) {
                    eprintln!(
                        "[cluster-sherpa] stt open still refused after {open_wait_max_ms} ms; finalizing anyway"
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        let queued_ms_at_call = self.queued_bytes.load(Ordering::SeqCst) / STT_BYTES_PER_MS;
        let notify = Arc::new(Notify::new());
        let cmd_tx = self.inner.lock().await.cmd_tx.clone();
        if let Some(tx) = cmd_tx {
            let _ = tx.send(StreamCommand::Finalize {
                done: Arc::clone(&notify),
            });
        }
        let base_wait_ms: u64 = std::env::var("SPEECH_STT_FINALIZE_WAIT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(FINALIZE_WAIT_MS);
        // The Finalize command sits behind any audio still queued for the pod.
        let wait_ms = base_wait_ms + queued_ms_at_call.min(FINALIZE_QUEUED_WAIT_CAP_MS) as u64;
        let waited = tokio::time::timeout(Duration::from_millis(wait_ms), notify.notified())
            .await
            .is_ok();
        if !waited {
            eprintln!("[cluster-sherpa] speech_finalize_timeout");
        }
        {
            let mut inner = self.inner.lock().await;
            inner.utterance_active = false;
        }
        self.queued_warned.store(false, Ordering::SeqCst);
        self.queued_error_logged.store(false, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_streak_logs_first_then_every_five_seconds() {
        let t0 = Instant::now();
        let mut s = OpenStreak::default();
        assert_eq!(s.mark_denied(t0), DeniedLog::First);
        assert_eq!(
            s.mark_denied(t0 + Duration::from_millis(200)),
            DeniedLog::Silent
        );
        assert_eq!(
            s.mark_denied(t0 + Duration::from_secs(4)),
            DeniedLog::Silent
        );
        assert_eq!(
            s.mark_denied(t0 + Duration::from_secs(5)),
            DeniedLog::Still { attempts: 4 }
        );
        assert_eq!(
            s.mark_denied(t0 + Duration::from_secs(6)),
            DeniedLog::Silent
        );
        assert_eq!(
            s.mark_denied(t0 + Duration::from_secs(10)),
            DeniedLog::Still { attempts: 6 }
        );
    }

    #[test]
    fn ready_after_denials_reports_attempts_and_resets() {
        let t0 = Instant::now();
        let mut s = OpenStreak::default();
        s.mark_denied(t0);
        s.mark_denied(t0 + Duration::from_millis(200));
        assert_eq!(
            s.mark_ready(t0 + Duration::from_millis(450)),
            Some((2, 450))
        );
        assert_eq!(s.mark_ready(t0 + Duration::from_secs(1)), None);
        assert_eq!(s.mark_denied(t0 + Duration::from_secs(2)), DeniedLog::First);
    }

    #[test]
    fn ready_without_denials_is_silent() {
        let mut s = OpenStreak::default();
        assert_eq!(s.mark_ready(Instant::now()), None);
    }
}
