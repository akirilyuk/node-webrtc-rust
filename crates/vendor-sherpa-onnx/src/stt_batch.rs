//! One decode thread per shared STT recognizer. Sessions hand over their session lock (an owned
//! guard) together with what to do after the decode; the thread decodes every ready stream of a
//! round with one `decode_multiple_streams` call. No window, no timer: a request that arrives
//! while the thread is idle is decoded at once, requests that arrive during a round join the next.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Weak;
use std::time::Instant;

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::otel;
use node_webrtc_rust_speech::pipeline::SttTranscript;
use sherpa_onnx::OnlineStream;

use crate::loader::voice_debug;
use crate::native_calls::NativeCallGuard;
use crate::pool::SharedSttRecognizer;
use crate::stt::SherpaSttState;

const MAX_STEPS: u32 = 64;
const THREAD_STOPPED: &str = "sherpa decode thread stopped";

static SHERPA_BATCH_ROUNDS: AtomicU64 = AtomicU64::new(0);
static SHERPA_BATCH_STREAM_STEPS: AtomicU64 = AtomicU64::new(0);

/// Number of `decode_multiple_streams` rounds the decode threads ran (perf probe / tests).
pub fn sherpa_batch_rounds() -> u64 {
    SHERPA_BATCH_ROUNDS.load(Ordering::SeqCst)
}

/// Sum of the streams decoded over all rounds (perf probe / tests).
pub fn sherpa_batch_stream_steps() -> u64 {
    SHERPA_BATCH_STREAM_STEPS.load(Ordering::SeqCst)
}

pub fn reset_sherpa_batch_counters() {
    SHERPA_BATCH_ROUNDS.store(0, Ordering::SeqCst);
    SHERPA_BATCH_STREAM_STEPS.store(0, Ordering::SeqCst);
}

/// What the decode thread does with a session once its stream has no ready chunk left.
pub(crate) enum AfterDecode {
    /// `push_audio`: only mark the session (`decoded_since_result`, `unread`) when a step ran.
    MarkOnly,
    /// `push_and_poll`: drain `pending`, then `read_transcript` when a step ran; `unread = false`.
    ReadTranscripts,
    /// `finalize_utterance`: after the last step read the result, queue a non-empty text as
    /// `Final` in `pending`, replace the stream with a fresh one, reset the per-utterance fields.
    Finalize,
}

pub(crate) struct DecodeRequest {
    pub guard: tokio::sync::OwnedMutexGuard<SherpaSttState>,
    pub after: AfterDecode,
    pub submitted_at: Instant,
    pub done: tokio::sync::oneshot::Sender<SpeechResult<Vec<SttTranscript>>>,
}

pub(crate) struct DecodeBatcher {
    tx: Sender<DecodeRequest>,
}

impl DecodeBatcher {
    pub(crate) fn spawn(shared: Weak<SharedSttRecognizer>) -> Self {
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("sherpa-stt-decode".into())
            .spawn(move || run(rx, shared))
            .expect("spawn sherpa-stt-decode thread");
        Self { tx }
    }

    /// Err when the thread is gone.
    pub(crate) fn submit(&self, request: DecodeRequest) -> SpeechResult<()> {
        self.tx
            .send(request)
            .map_err(|_| SpeechError::Internal(THREAD_STOPPED.into()))
    }
}

struct Active {
    request: DecodeRequest,
    steps: u32,
    wait_recorded: bool,
}

fn stream_of(request: &DecodeRequest) -> Option<&OnlineStream> {
    request
        .guard
        .session
        .as_ref()
        .filter(|_| request.guard.running)
        .map(|session| &session.stream)
}

fn run(rx: Receiver<DecodeRequest>, shared: Weak<SharedSttRecognizer>) {
    while let Ok(first) = rx.recv() {
        let mut active: Vec<Active> = vec![Active {
            request: first,
            steps: 0,
            wait_recorded: false,
        }];
        loop {
            while let Ok(request) = rx.try_recv() {
                active.push(Active {
                    request,
                    steps: 0,
                    wait_recorded: false,
                });
            }
            // Stopped sessions: nothing to decode, answer at once.
            let mut index = 0;
            while index < active.len() {
                if stream_of(&active[index].request).is_none() {
                    let item = active.swap_remove(index);
                    let _ = item.request.done.send(Ok(Vec::new()));
                } else {
                    index += 1;
                }
            }
            if active.is_empty() {
                break;
            }
            let Some(shared) = shared.upgrade() else {
                for item in active.drain(..) {
                    let _ = item
                        .request
                        .done
                        .send(Err(SpeechError::Internal(THREAD_STOPPED.into())));
                }
                return;
            };
            for item in active.iter_mut() {
                if !item.wait_recorded {
                    item.wait_recorded = true;
                    otel::record_sherpa_pool_wait_ms(
                        item.request.submitted_at.elapsed().as_secs_f64() * 1000.0,
                        None,
                    );
                }
            }
            let _native = NativeCallGuard::enter();
            shared.with_recognizer(|recognizer| {
                let mut decoded: Vec<usize> = Vec::new();
                for (i, item) in active.iter().enumerate() {
                    if item.steps < MAX_STEPS {
                        if let Some(stream) = stream_of(&item.request) {
                            if recognizer.is_ready(stream) {
                                decoded.push(i);
                            }
                        }
                    }
                }
                if !decoded.is_empty() {
                    let refs: Vec<&OnlineStream> = decoded
                        .iter()
                        .filter_map(|&i| stream_of(&active[i].request))
                        .collect();
                    recognizer.decode_multiple_streams(&refs);
                    SHERPA_BATCH_ROUNDS.fetch_add(1, Ordering::SeqCst);
                    SHERPA_BATCH_STREAM_STEPS.fetch_add(refs.len() as u64, Ordering::SeqCst);
                    drop(refs);
                    for &i in &decoded {
                        active[i].steps += 1;
                    }
                }
                let mut index = 0;
                while index < active.len() {
                    let finished = {
                        let item = &active[index];
                        match stream_of(&item.request) {
                            None => true,
                            Some(stream) => {
                                if item.steps >= MAX_STEPS {
                                    if recognizer.is_ready(stream) {
                                        voice_debug(
                                            "sherpa decode loop capped at 64 steps (possible is_ready stuck)",
                                        );
                                    }
                                    true
                                } else {
                                    !recognizer.is_ready(stream)
                                }
                            }
                        }
                    };
                    if !finished {
                        index += 1;
                        continue;
                    }
                    let mut item = active.swap_remove(index);
                    let steps = item.steps;
                    let guard = &mut *item.request.guard;
                    let transcripts = match guard.session.as_mut() {
                        Some(session) if guard.running => {
                            session.finish_after(recognizer, &item.request.after, steps)
                        }
                        _ => Vec::new(),
                    };
                    let _ = item.request.done.send(Ok(transcripts));
                    // guard dropped here, releasing the session lock
                }
            });
        }
    }
}
