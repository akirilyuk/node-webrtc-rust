//! In-process tonic `Speech` mock (no ONNX) for cluster-sherpa vendor tests.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech_proto::v1::speech_server::{Speech, SpeechServer};
use node_webrtc_rust_speech_proto::v1::{
    transcribe_request, transcribe_response, PrepareRequest, PrepareResponse, SttAudio, SttError,
    SttFinalize, SttFinalized, SttReady, SttRelocate, SttStop, SttTranscript, SynthesizeRequest,
    SynthesizeResponse, TranscribeRequest, TranscribeResponse,
};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::transport::Server;
use tonic::{Request, Response, Status, Streaming};

/// `(1-based stream index, audio bytes)` per audio message the server read.
pub type AudioLog = Arc<Mutex<Vec<(usize, Vec<u8>)>>>;

#[derive(Clone, Default)]
pub struct MockSpeechState {
    pub transcribe_streams: Arc<AtomicUsize>,
    pub synthesize_calls: Arc<AtomicUsize>,
    pub audio_frames: Arc<AtomicUsize>,
    pub finalize_count: Arc<AtomicUsize>,
    pub relocate_after_finalize: Arc<AtomicBool>,
    pub close_after_finalize: Arc<AtomicBool>,
    pub error_on_audio: Arc<AtomicBool>,
    /// First inbound audio emits leftover empty `is_final` (dedicated speech-service class).
    pub leftover_empty_final_on_first_audio: Arc<AtomicBool>,
    pub leftover_empty_emitted: Arc<AtomicUsize>,
    /// `0` = default behaviour (two small chunks). `> 0` = `synthesize` sends exactly one
    /// `SynthesizeResponse` carrying this many PCM bytes, then a terminal empty `last` message.
    pub synthesize_single_message_bytes: Arc<AtomicUsize>,
    /// While `true`, the default `synthesize` stream ends without a terminal `last` message.
    pub synthesize_omit_last: Arc<AtomicBool>,
    /// Currently open `transcribe` calls (server side). Decremented when the call's work ends.
    pub open_transcribe_streams: Arc<AtomicIsize>,
    /// While `true`, the `transcribe` handler stops reading inbound messages (stalled server).
    pub pause_reading: Arc<AtomicBool>,
    /// Wakes a handler parked on `pause_reading`. Set the flag to `false`, then `notify_waiters`.
    pub resume_reading: Arc<tokio::sync::Notify>,
    /// Sum of `SttAudio.pcm_s16le.len()` over every audio message the server read.
    pub audio_bytes_received: Arc<AtomicUsize>,
    /// The next N `transcribe` opens fail with `RESOURCE_EXHAUSTED` (pod admission cap full).
    /// Rejected opens do not count in `transcribe_streams`.
    pub reject_first_opens: Arc<AtomicUsize>,
    /// How many opens were rejected by `reject_first_opens`.
    pub rejected_opens: Arc<AtomicUsize>,
    /// Milliseconds the mock waits between the final transcript and `Finalized` (0 = none).
    pub finalized_delay_ms: Arc<AtomicUsize>,
    /// When `true`, final transcripts read `final-{n}` with `n` = the finalize count.
    pub numbered_finals: Arc<AtomicBool>,
    /// Every audio message the server read: `(1-based stream index, bytes)` in arrival order.
    pub audio_log: AudioLog,
    /// `synthesize` answers `UNAVAILABLE` for this many ms after the first call, then serves.
    pub tts_unavailable_for_ms: Arc<AtomicUsize>,
    /// While `true`, `synthesize` always answers `UNAVAILABLE`.
    pub tts_unavailable_always: Arc<AtomicBool>,
    /// While `true`, `synthesize` always answers `INVALID_ARGUMENT`.
    pub tts_invalid_argument: Arc<AtomicBool>,
    /// First `synthesize` call time (starts the `tts_unavailable_for_ms` window).
    pub tts_first_call_at: Arc<Mutex<Option<std::time::Instant>>>,
    /// `synthesize` calls answered with an error status.
    pub tts_refused_calls: Arc<AtomicUsize>,
}

/// Decrements `open_transcribe_streams` when dropped (end of the call's spawned task).
struct OpenStreamGuard(Arc<AtomicIsize>);

impl OpenStreamGuard {
    fn new(counter: &Arc<AtomicIsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(counter))
    }
}

impl Drop for OpenStreamGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct MockSpeech {
    state: MockSpeechState,
}

impl MockSpeech {
    pub fn new(state: MockSpeechState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Speech for MockSpeech {
    async fn prepare(
        &self,
        _request: Request<PrepareRequest>,
    ) -> Result<Response<PrepareResponse>, Status> {
        Ok(Response::new(PrepareResponse {
            stt_loaded: 1,
            tts_loaded: 1,
            server_version: "mock".into(),
        }))
    }

    type TranscribeStream = ReceiverStream<Result<TranscribeResponse, Status>>;

    async fn transcribe(
        &self,
        request: Request<Streaming<TranscribeRequest>>,
    ) -> Result<Response<Self::TranscribeStream>, Status> {
        let rejected = self
            .state
            .reject_first_opens
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if rejected {
            self.state.rejected_opens.fetch_add(1, Ordering::SeqCst);
            return Err(Status::resource_exhausted("mock: speech pod full"));
        }
        let stream_idx = self.state.transcribe_streams.fetch_add(1, Ordering::SeqCst) + 1;
        let mut inbound = request.into_inner();
        let (tx, rx) = mpsc::channel(32);
        let state = self.state.clone();
        let open_guard = OpenStreamGuard::new(&self.state.open_transcribe_streams);
        tokio::spawn(async move {
            let _open_guard = open_guard;
            let _ = tx
                .send(Ok(TranscribeResponse {
                    msg: Some(transcribe_response::Msg::Ready(SttReady {
                        model_path: "/models/sherpa/stt/mock".into(),
                    })),
                }))
                .await;
            let mut utterance_open = false;
            loop {
                // Stalled-server mode: do not read the next inbound message while paused.
                // `enable()` registers the waiter before the flag check, so a resume that lands
                // in between is not lost.
                loop {
                    let notified = state.resume_reading.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    if !state.pause_reading.load(Ordering::SeqCst) {
                        break;
                    }
                    notified.await;
                }
                let Ok(Some(req)) = inbound.message().await else {
                    break;
                };
                match req.msg {
                    Some(transcribe_request::Msg::Audio(SttAudio { pcm_s16le })) => {
                        utterance_open = true;
                        state.audio_frames.fetch_add(1, Ordering::SeqCst);
                        state
                            .audio_bytes_received
                            .fetch_add(pcm_s16le.len(), Ordering::SeqCst);
                        state
                            .audio_log
                            .lock()
                            .expect("audio log")
                            .push((stream_idx, pcm_s16le.to_vec()));
                        if state.error_on_audio.load(Ordering::SeqCst) {
                            let _ = tx
                                .send(Ok(TranscribeResponse {
                                    msg: Some(transcribe_response::Msg::Error(SttError {
                                        code: "mock".into(),
                                        message: "mid utterance".into(),
                                    })),
                                }))
                                .await;
                            break;
                        }
                        if state
                            .leftover_empty_final_on_first_audio
                            .load(Ordering::SeqCst)
                            && state.leftover_empty_emitted.load(Ordering::SeqCst) == 0
                        {
                            state.leftover_empty_emitted.fetch_add(1, Ordering::SeqCst);
                            let _ = tx
                                .send(Ok(TranscribeResponse {
                                    msg: Some(transcribe_response::Msg::Transcript(
                                        SttTranscript {
                                            text: String::new(),
                                            is_final: true,
                                        },
                                    )),
                                }))
                                .await;
                            continue;
                        }
                        if !pcm_s16le.is_empty() {
                            let _ = tx
                                .send(Ok(TranscribeResponse {
                                    msg: Some(transcribe_response::Msg::Transcript(
                                        SttTranscript {
                                            text: "partial".into(),
                                            is_final: false,
                                        },
                                    )),
                                }))
                                .await;
                        }
                    }
                    Some(transcribe_request::Msg::Finalize(SttFinalize {})) => {
                        let finalize_n = state.finalize_count.fetch_add(1, Ordering::SeqCst) + 1;
                        let _ = tx
                            .send(Ok(TranscribeResponse {
                                msg: Some(transcribe_response::Msg::Transcript(SttTranscript {
                                    text: if state.numbered_finals.load(Ordering::SeqCst) {
                                        format!("final-{finalize_n}")
                                    } else if state
                                        .leftover_empty_final_on_first_audio
                                        .load(Ordering::SeqCst)
                                    {
                                        "one two three".into()
                                    } else {
                                        "final text".into()
                                    },
                                    is_final: true,
                                })),
                            }))
                            .await;
                        let delay_ms = state.finalized_delay_ms.load(Ordering::SeqCst);
                        if delay_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(delay_ms as u64)).await;
                        }
                        let _ = tx
                            .send(Ok(TranscribeResponse {
                                msg: Some(transcribe_response::Msg::Finalized(SttFinalized {})),
                            }))
                            .await;
                        utterance_open = false;
                        if state.relocate_after_finalize.load(Ordering::SeqCst) {
                            let _ = tx
                                .send(Ok(TranscribeResponse {
                                    msg: Some(transcribe_response::Msg::Relocate(SttRelocate {})),
                                }))
                                .await;
                            break;
                        }
                        if state.close_after_finalize.load(Ordering::SeqCst) {
                            break;
                        }
                    }
                    Some(transcribe_request::Msg::Stop(SttStop {})) => break,
                    _ => {}
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    type SynthesizeStream = ReceiverStream<Result<SynthesizeResponse, Status>>;

    async fn synthesize(
        &self,
        request: Request<SynthesizeRequest>,
    ) -> Result<Response<Self::SynthesizeStream>, Status> {
        self.state.synthesize_calls.fetch_add(1, Ordering::SeqCst);
        if self.state.tts_invalid_argument.load(Ordering::SeqCst) {
            self.state.tts_refused_calls.fetch_add(1, Ordering::SeqCst);
            return Err(Status::invalid_argument("mock: bad synth request"));
        }
        let first_call_at = *self
            .state
            .tts_first_call_at
            .lock()
            .unwrap()
            .get_or_insert_with(std::time::Instant::now);
        let window_ms = self.state.tts_unavailable_for_ms.load(Ordering::SeqCst) as u128;
        if self.state.tts_unavailable_always.load(Ordering::SeqCst)
            || first_call_at.elapsed().as_millis() < window_ms
        {
            self.state.tts_refused_calls.fetch_add(1, Ordering::SeqCst);
            return Err(Status::unavailable("mock: speech models are still loading"));
        }
        let text = request.into_inner().text;
        let (tx, rx) = mpsc::channel(4);
        let single_bytes = self
            .state
            .synthesize_single_message_bytes
            .load(Ordering::SeqCst);
        if single_bytes > 0 {
            let n = single_bytes;
            tokio::spawn(async move {
                let _ = tx
                    .send(Ok(SynthesizeResponse {
                        pcm_s16le: (0..n).map(|i| (i % 251) as u8).collect::<Vec<u8>>().into(),
                        duration_ms: (n / 192) as u32,
                        last: false,
                    }))
                    .await;
                let _ = tx
                    .send(Ok(SynthesizeResponse {
                        pcm_s16le: Bytes::new(),
                        duration_ms: 0,
                        last: true,
                    }))
                    .await;
            });
            return Ok(Response::new(ReceiverStream::new(rx)));
        }
        let last = !self.state.synthesize_omit_last.load(Ordering::SeqCst);
        let pcm_len = text.len().max(4) * 80;
        let _ = tx
            .send(Ok(SynthesizeResponse {
                pcm_s16le: Bytes::from(vec![0_u8; pcm_len]),
                duration_ms: 20,
                last: false,
            }))
            .await;
        let _ = tx
            .send(Ok(SynthesizeResponse {
                pcm_s16le: Bytes::from(vec![0_u8; pcm_len]),
                duration_ms: 20,
                last,
            }))
            .await;
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

/// Bind `127.0.0.1:0` and serve `Speech` until the returned shutdown future completes.
pub async fn spawn_mock_speech(
    state: MockSpeechState,
) -> Result<(String, tokio::sync::oneshot::Sender<()>), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let url = format!("http://{}", addr);
    let svc = SpeechServer::new(MockSpeech::new(state));
    let incoming = TcpListenerStream::new(listener);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(svc)
            .serve_with_incoming_shutdown(incoming, async {
                let _ = shutdown_rx.await;
            })
            .await;
    });
    Ok((url, shutdown_tx))
}
