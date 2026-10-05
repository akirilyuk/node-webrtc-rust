use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
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

use crate::channel::{resolve_speech_token, resolve_stt_endpoint, stt_channel};
use crate::metrics::inc_stt_reopen;

const COALESCE_MAX_BYTES: usize = 1920;
const AUDIO_QUEUE_CAP: usize = 256;
const READY_WAIT_MS: u64 = 15_000;
const FINALIZE_WAIT_MS: u64 = 2_000;
/// While the open is denied, print a progress line at most this often.
const DENIED_LOG_INTERVAL: Duration = Duration::from_secs(5);

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
        let out = self.started.map(|t| {
            (
                self.attempts,
                now.duration_since(t).as_millis() as u64,
            )
        });
        *self = Self::default();
        out
    }
}

enum StreamCommand {
    Audio(Bytes),
    Finalize {
        done: Arc<Notify>,
    },
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

pub(crate) fn auth_metadata(token: &Option<String>) -> SpeechResult<MetadataValue<tonic::metadata::Ascii>> {
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
}

struct ClusterSherpaSttInner {
    running: bool,
    transcript_rx: Option<mpsc::UnboundedReceiver<SttTranscript>>,
    cmd_tx: Option<mpsc::Sender<StreamCommand>>,
    stream_task: Option<JoinHandle<()>>,
    /// `true` once the current stream received `Ready`; `false` while (re)opening or after exit.
    ready_rx: Option<watch::Receiver<bool>>,
    pending_audio: Vec<u8>,
    utterance_active: bool,
    mid_utterance_failure: Option<String>,
}

impl ClusterSherpaStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
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
                pending_audio: Vec::new(),
                utterance_active: false,
                mid_utterance_failure: None,
            })),
        })
    }

    async fn spawn_stream_task(&self) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        if inner.stream_task.is_some() {
            return Ok(());
        }
        let (transcript_tx, transcript_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::channel(AUDIO_QUEUE_CAP);
        let cfg = self.cfg.clone();
        let endpoint = self.endpoint.clone();
        let token = self.token.clone();
        let session_ctx = Arc::clone(&self.session_ctx);
        let (ready_tx, ready_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            stream_worker(
                cfg,
                endpoint,
                token,
                session_ctx,
                cmd_rx,
                transcript_tx,
                &ready_tx,
            )
            .await;
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
            Bytes::copy_from_slice(&inner.pending_audio)
        };
        {
            let mut inner = self.inner.lock().await;
            inner.pending_audio.clear();
            let Some(cmd_tx) = inner.cmd_tx.as_ref() else {
                return Ok(());
            };
            match cmd_tx.try_send(StreamCommand::Audio(pcm)) {
                Ok(()) => Ok(()),
                Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
                Err(mpsc::error::TrySendError::Closed(_)) => Err(SpeechError::Vendor {
                    vendor: "cluster-sherpa".into(),
                    message: "STT stream closed".into(),
                }),
            }
        }
    }
}

async fn stream_worker(
    cfg: SttConfig,
    endpoint: String,
    token: Option<String>,
    session_ctx: Arc<Mutex<Option<VoiceSessionContext>>>,
    mut cmd_rx: mpsc::Receiver<StreamCommand>,
    transcript_tx: mpsc::UnboundedSender<SttTranscript>,
    ready_tx: &watch::Sender<bool>,
) {
    let mut pending_finalize: Option<Arc<Notify>> = None;
    let mut reopen = true;
    let mut streak = OpenStreak::default();
    while reopen {
        reopen = false;
        ready_tx.send_replace(false);
        let channel = match stt_channel(&endpoint).await {
            Ok(c) => c,
            Err(e) => {
                let _ = transcript_tx.send(SttTranscript::Final(String::new()));
                eprintln!("[cluster-sherpa] connect failed: {e}");
                break;
            }
        };
        let mut client = SpeechClient::new(channel);
        let (mut req_tx, req_rx) = mpsc::channel(32);
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
            msg: Some(transcribe_request::Msg::Start(node_webrtc_rust_speech_proto::v1::SttStart {
                model: Some(ModelRef {
                    model_path,
                    catalog_id: String::new(),
                }),
                language,
                ctx: Some(ctx),
            })),
        };
        if req_tx.send(start).await.is_err() {
            break;
        }

        let mut request = Request::new(tokio_stream::wrappers::ReceiverStream::new(req_rx));
        if let Ok(auth) = auth_metadata(&token) {
            request.metadata_mut().insert("authorization", auth);
        }
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
                tokio::time::sleep(Duration::from_millis(200)).await;
                reopen = true;
                continue;
            }
        };

        let ready_deadline = tokio::time::sleep(Duration::from_millis(READY_WAIT_MS));
        tokio::pin!(ready_deadline);
        let mut ready = false;
        let mut utterance_open = false;

        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(StreamCommand::Audio(pcm)) => {
                            utterance_open = true;
                            let audio = TranscribeRequest {
                                msg: Some(transcribe_request::Msg::Audio(SttAudio {
                                    pcm_s16le: pcm.to_vec(),
                                })),
                            };
                            if req_tx.send(audio).await.is_err() {
                                break;
                            }
                        }
                        Some(StreamCommand::Finalize { done }) => {
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
                                }
                                Some(RMsg::Relocate(_)) => {
                                    inc_stt_reopen("relocate");
                                    reopen = true;
                                    break;
                                }
                                Some(RMsg::Error(e)) => {
                                    if utterance_open {
                                        let _ = transcript_tx.send(SttTranscript::Final(String::new()));
                                    } else {
                                        inc_stt_reopen("idle_error");
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
            let _ = tx.send(StreamCommand::Stop).await;
        }
        if let Some(task) = inner.stream_task.take() {
            let _ = task.await;
        }
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

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        self.flush_pending_audio(true).await?;
        let notify = Arc::new(Notify::new());
        let cmd_tx = self.inner.lock().await.cmd_tx.clone();
        if let Some(tx) = cmd_tx {
            let _ = tx
                .send(StreamCommand::Finalize {
                    done: Arc::clone(&notify),
                })
                .await;
        }
        let wait_ms = std::env::var("SPEECH_STT_FINALIZE_WAIT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(FINALIZE_WAIT_MS);
        let waited = tokio::time::timeout(
            Duration::from_millis(wait_ms),
            notify.notified(),
        )
        .await
        .is_ok();
        if !waited {
            eprintln!("[cluster-sherpa] speech_finalize_timeout");
        }
        {
            let mut inner = self.inner.lock().await;
            inner.utterance_active = false;
        }
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
        assert_eq!(s.mark_denied(t0 + Duration::from_millis(200)), DeniedLog::Silent);
        assert_eq!(s.mark_denied(t0 + Duration::from_secs(4)), DeniedLog::Silent);
        assert_eq!(
            s.mark_denied(t0 + Duration::from_secs(5)),
            DeniedLog::Still { attempts: 4 }
        );
        assert_eq!(s.mark_denied(t0 + Duration::from_secs(6)), DeniedLog::Silent);
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
        assert_eq!(s.mark_ready(t0 + Duration::from_millis(450)), Some((2, 450)));
        assert_eq!(s.mark_ready(t0 + Duration::from_secs(1)), None);
        assert_eq!(s.mark_denied(t0 + Duration::from_secs(2)), DeniedLog::First);
    }

    #[test]
    fn ready_without_denials_is_silent() {
        let mut s = OpenStreak::default();
        assert_eq!(s.mark_ready(Instant::now()), None);
    }
}
