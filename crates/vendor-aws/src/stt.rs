//! AWS Transcribe streaming STT (documented SDK / event-stream path).

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use bytes::Bytes;
#[cfg(feature = "live")]
use futures_util::Stream;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use tokio::sync::{mpsc, Mutex};

use crate::matrix::{default_stt_language_code, validate_stt_language_code};

pub struct AwsStt {
    language_code: String,
    state: Arc<Mutex<AwsSttInner>>,
    /// One Transcribe client for the provider's life (built on first use): the SDK pools its
    /// connections per client.
    #[cfg(feature = "live")]
    client: tokio::sync::OnceCell<aws_sdk_transcribestreaming::Client>,
    /// `h2` once a stream is open: Transcribe streaming is an HTTP/2 event stream.
    last_transport: std::sync::Mutex<Option<String>>,
}

/// What the reader task forwards: a transcript, or the error that ended the event stream.
type SttItem = Result<SttTranscript, SpeechError>;

struct AwsSttInner {
    running: bool,
    pcm_tx: Option<mpsc::UnboundedSender<Bytes>>,
    transcript_rx: Option<mpsc::UnboundedReceiver<SttItem>>,
    reader_task: Option<tokio::task::JoinHandle<()>>,
    /// Results already taken off the stream (by a finalize or a stream switch), polled first.
    pending: std::collections::VecDeque<SttItem>,
}

impl AwsStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        crate::matrix::check_stt_transport(config.transport)?;
        let language_code = config
            .model
            .clone()
            .unwrap_or_else(|| default_stt_language_code().to_string());
        validate_stt_language_code(&language_code).map_err(SpeechError::Config)?;
        Ok(Self {
            language_code,
            state: Arc::new(Mutex::new(AwsSttInner {
                running: false,
                pcm_tx: None,
                transcript_rx: None,
                reader_task: None,
                pending: std::collections::VecDeque::new(),
            })),
            #[cfg(feature = "live")]
            client: tokio::sync::OnceCell::new(),
            last_transport: std::sync::Mutex::new(None),
        })
    }
}

#[async_trait]
impl SttProvider for AwsStt {
    fn vendor_name(&self) -> &'static str {
        "aws"
    }

    fn last_transport(&self) -> Option<String> {
        self.last_transport.lock().ok().and_then(|g| g.clone())
    }

    async fn start(&mut self) -> SpeechResult<()> {
        self.stop().await?;
        #[cfg(feature = "live")]
        {
            self.open_stream(None).await?;
            self.state.lock().await.running = true;
            return Ok(());
        }

        #[cfg(not(feature = "live"))]
        {
            Err(SpeechError::Vendor {
                vendor: "aws".into(),
                message: "live AWS STT requires `--features live` on vendor-aws".into(),
            })
        }
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let mut inner = self.state.lock().await;
        inner.running = false;
        inner.pcm_tx = None;
        if let Some(task) = inner.reader_task.take() {
            task.abort();
        }
        inner.transcript_rx = None;
        inner.pending.clear();
        Ok(())
    }

    /// Audio after a finalize (or after the service closed the stream) opens a new stream,
    /// because Transcribe takes one stream per session and a stream that was ended cannot be
    /// reused. The chunk is queued before the open so the service has audio to read.
    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let needs_stream = {
            let inner = self.state.lock().await;
            if !inner.running {
                return Ok(());
            }
            !inner
                .pcm_tx
                .as_ref()
                .is_some_and(|tx| tx.send(pcm.clone()).is_ok())
        };
        if !needs_stream {
            return Ok(());
        }
        #[cfg(feature = "live")]
        {
            self.open_stream(Some(pcm)).await
        }
        #[cfg(not(feature = "live"))]
        {
            Ok(())
        }
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let mut inner = self.state.lock().await;
        if !inner.running {
            return Ok(None);
        }
        let item = match inner.pending.pop_front() {
            Some(item) => item,
            None => match inner.transcript_rx.as_mut().map(|rx| rx.try_recv()) {
                Some(Ok(item)) => item,
                _ => return Ok(None),
            },
        };
        item.map(Some)
    }

    /// Ends the utterance by ending the stream. Transcribe documents it: "To end your stream,
    /// send an empty audio event to Amazon Transcribe. Create the audio event exactly like any
    /// other, except with an empty payload." (streaming-setting-up, HTTP/2 steps), and "To
    /// indicate the audio stream is complete, send an end frame (an empty data frame)". Closing
    /// the input stream makes the SDK send that end frame. The service then returns the
    /// remaining results, the last one with `IsPartial: false`, and closes its side; those
    /// results wait for `poll_transcript`. The wait is bounded by [`Self::finalize_wait_ms`] and
    /// a stream error or a timeout is an `Err`. The stream is spent after this: the next
    /// `push_audio` opens a new one ("Amazon Transcribe only supports one stream per HTTP/2
    /// session").
    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let wait = std::time::Duration::from_millis(self.finalize_wait_ms());
        let mut inner = self.state.lock().await;
        if !inner.running {
            return Ok(());
        }
        let Some(tx) = inner.pcm_tx.take() else {
            return Ok(());
        };
        let AwsSttInner {
            transcript_rx,
            pending,
            ..
        } = &mut *inner;
        let Some(rx) = transcript_rx.as_mut() else {
            return Ok(());
        };
        end_utterance(tx, rx, pending, wait).await
    }
}

/// Sends the empty audio event, drops the input (the SDK sends the end frame), and moves every
/// result into `pending` until the service closes its side. `Err` when a stream error arrived
/// or when the service did not close in `wait`.
async fn end_utterance(
    pcm_tx: mpsc::UnboundedSender<Bytes>,
    rx: &mut mpsc::UnboundedReceiver<SttItem>,
    pending: &mut std::collections::VecDeque<SttItem>,
    wait: std::time::Duration,
) -> SpeechResult<()> {
    let _ = pcm_tx.send(Bytes::new());
    drop(pcm_tx);
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) => {
                return Err(SpeechError::Vendor {
                    vendor: "aws".into(),
                    message: format!(
                        "transcribe did not finish the stream within {} ms of the end frame",
                        wait.as_millis()
                    ),
                });
            }
            // The service closed the stream: every result was delivered.
            Ok(None) => return Ok(()),
            Ok(Some(Ok(transcript))) => pending.push_back(Ok(transcript)),
            // The error is reported by this call, once; results before it stay queued.
            Ok(Some(Err(err))) => return Err(err),
        }
    }
}

#[cfg(feature = "live")]
impl AwsStt {
    /// Opens a Transcribe stream and installs its channels. `first` is queued before the open.
    async fn open_stream(&mut self, first: Option<Bytes>) -> SpeechResult<()> {
        use aws_sdk_transcribestreaming::primitives::event_stream::EventStreamSender;
        use aws_sdk_transcribestreaming::types::MediaEncoding;

        let language_code = self.language_code.clone();
        let (pcm_tx, pcm_rx) = mpsc::unbounded_channel::<Bytes>();
        let (transcript_tx, transcript_rx) = mpsc::unbounded_channel::<SttItem>();
        if let Some(first) = first {
            let _ = pcm_tx.send(first);
        }
        let audio_stream = EventStreamSender::from(PcmAudioStream { rx: pcm_rx });

        let client = self
            .client
            .get_or_init(|| async {
                let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .timeout_config(crate::matrix::transcribe_timeouts())
                    .load()
                    .await;
                aws_sdk_transcribestreaming::Client::new(&config)
            })
            .await;
        let language = language_code_to_aws(&language_code).map_err(SpeechError::Config)?;

        let open = client
            .start_stream_transcription()
            .language_code(language)
            .media_sample_rate_hertz(16_000)
            .media_encoding(MediaEncoding::Pcm)
            .audio_stream(audio_stream)
            .send();
        // Both the first open (`start`) and a mid-session open (`push_audio`) are bounded.
        let response = bound_open(crate::matrix::CONNECT_TIMEOUT, open)
            .await?
            .map_err(|err| SpeechError::Vendor {
                vendor: "aws".into(),
                message: aws_sdk_transcribestreaming::error::DisplayErrorContext(&err).to_string(),
            })?;

        if let Ok(mut guard) = self.last_transport.lock() {
            *guard = Some("h2".into());
        }
        let mut transcript_stream = response.transcript_result_stream;

        let reader_task = tokio::spawn(async move {
            // A stream that ends with an error (reset, service exception, bad frame) is
            // reported to the next poll; `Ok(None)` is the service closing it normally.
            while forward_event(&transcript_tx, transcript_stream.recv().await) {}
        });

        let mut inner = self.state.lock().await;
        if let Some(old) = inner.reader_task.take() {
            old.abort();
        }
        // Results of the previous stream that were not polled yet stay in `pending`.
        let old_rx = inner.transcript_rx.take();
        if let Some(mut old_rx) = old_rx {
            while let Ok(item) = old_rx.try_recv() {
                inner.pending.push_back(item);
            }
        }
        inner.pcm_tx = Some(pcm_tx);
        inner.transcript_rx = Some(transcript_rx);
        inner.reader_task = Some(reader_task);
        inner.running = true;
        Ok(())
    }
}

/// Runs a stream open under `bound`. A service that never answers is a named error, not a hang
/// (`infra.unbounded-external-wait`).
#[cfg_attr(not(feature = "live"), allow(dead_code))]
async fn bound_open<T>(
    bound: std::time::Duration,
    open: impl std::future::Future<Output = T>,
) -> SpeechResult<T> {
    tokio::time::timeout(bound, open)
        .await
        .map_err(|_| SpeechError::Vendor {
            vendor: "aws".into(),
            message: format!(
                "transcribe stream open timed out after {} ms",
                bound.as_millis()
            ),
        })
}

/// Forwards one `recv()` result of the transcript event stream. True while the stream goes on.
/// `IsPartial: true` results go out as [`SttTranscript::Partial`], `false` as `Final`.
#[cfg(feature = "live")]
fn forward_event<E: std::fmt::Display>(
    tx: &mpsc::UnboundedSender<SttItem>,
    next: Result<Option<aws_sdk_transcribestreaming::types::TranscriptResultStream>, E>,
) -> bool {
    match next {
        Ok(Some(event)) => {
            if let Some(transcript) = transcript_from_event(&event) {
                let _ = tx.send(Ok(transcript));
            }
            true
        }
        Ok(None) => false,
        Err(err) => {
            let _ = tx.send(Err(SpeechError::Vendor {
                vendor: "aws".into(),
                message: format!("transcribe stream error: {err}"),
            }));
            false
        }
    }
}

#[cfg(feature = "live")]
struct PcmAudioStream {
    rx: mpsc::UnboundedReceiver<Bytes>,
}

#[cfg(feature = "live")]
impl Stream for PcmAudioStream {
    type Item = Result<
        aws_sdk_transcribestreaming::types::AudioStream,
        aws_sdk_transcribestreaming::types::error::AudioStreamError,
    >;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(chunk)) => {
                use aws_sdk_transcribestreaming::types::{AudioEvent, AudioStream};
                use aws_smithy_types::Blob;
                let event = AudioStream::AudioEvent(
                    AudioEvent::builder().audio_chunk(Blob::new(chunk)).build(),
                );
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(feature = "live")]
fn language_code_to_aws(
    code: &str,
) -> Result<aws_sdk_transcribestreaming::types::LanguageCode, String> {
    use aws_sdk_transcribestreaming::types::LanguageCode;
    match code {
        "en-US" => Ok(LanguageCode::EnUs),
        "es-US" => Ok(LanguageCode::EsUs),
        "fr-FR" => Ok(LanguageCode::FrFr),
        "de-DE" => Ok(LanguageCode::DeDe),
        "pt-BR" => Ok(LanguageCode::PtBr),
        "ja-JP" => Ok(LanguageCode::JaJp),
        "ko-KR" => Ok(LanguageCode::KoKr),
        "zh-CN" => Ok(LanguageCode::ZhCn),
        "it-IT" => Ok(LanguageCode::ItIt),
        "hi-IN" => Ok(LanguageCode::HiIn),
        other => Err(format!("unsupported AWS language mapping for `{other}`")),
    }
}

#[cfg(feature = "live")]
fn transcript_from_event(
    event: &aws_sdk_transcribestreaming::types::TranscriptResultStream,
) -> Option<SttTranscript> {
    let transcript_event = event.as_transcript_event().ok()?;
    let transcript = transcript_event.transcript()?;
    let first = transcript.results().first()?;
    let text = first.alternatives().first()?.transcript()?.trim();
    if text.is_empty() {
        return None;
    }
    if first.is_partial() {
        Some(SttTranscript::Partial(text.to_string()))
    } else {
        Some(SttTranscript::Final(text.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::SttVendor;

    #[test]
    fn rejects_undocumented_language() {
        assert!(matches!(
            AwsStt::new(&SttConfig {
                provider: SttVendor::Aws,
                model: Some("xx-XX".into()),
                model_path: None,
                language: None,
                api_key: None,
                endpoint: None,
                transport: None,
            }),
            Err(SpeechError::Config(_))
        ));
    }

    #[cfg(feature = "live")]
    fn event(
        is_partial: bool,
        text: &str,
    ) -> aws_sdk_transcribestreaming::types::TranscriptResultStream {
        use aws_sdk_transcribestreaming::types::{
            Alternative, Result as TranscribeResult, Transcript, TranscriptEvent,
            TranscriptResultStream,
        };
        TranscriptResultStream::TranscriptEvent(
            TranscriptEvent::builder()
                .transcript(
                    Transcript::builder()
                        .results(
                            TranscribeResult::builder()
                                .is_partial(is_partial)
                                .alternatives(Alternative::builder().transcript(text).build())
                                .build(),
                        )
                        .build(),
                )
                .build(),
        )
    }

    /// `IsPartial: true` results are surfaced as partials (so `emits_partials()` stays true).
    #[cfg(feature = "live")]
    #[test]
    fn partial_and_final_results_are_forwarded_as_such() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        assert!(forward_event::<String>(
            &tx,
            Ok(Some(event(true, "the quick")))
        ));
        assert!(forward_event::<String>(
            &tx,
            Ok(Some(event(false, "the quick fox")))
        ));
        assert!(matches!(rx.try_recv(), Ok(Ok(SttTranscript::Partial(t))) if t == "the quick"));
        assert!(matches!(rx.try_recv(), Ok(Ok(SttTranscript::Final(t))) if t == "the quick fox"));
    }

    /// A stream error after start reaches `poll_transcript` as an `Err` (not silence), and the
    /// reader stops. A normal close ends the reader without an error.
    #[cfg(feature = "live")]
    #[test]
    fn a_stream_error_is_forwarded_and_a_normal_close_is_not_an_error() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        assert!(!forward_event(&tx, Err("connection reset".to_string())));
        match rx.try_recv() {
            Ok(Err(SpeechError::Vendor { vendor, message })) => {
                assert_eq!(vendor, "aws");
                assert!(message.contains("connection reset"), "{message}");
            }
            other => panic!("expected a vendor error, got {other:?}"),
        }
        assert!(!forward_event::<String>(&tx, Ok(None)));
        assert!(rx.try_recv().is_err());
    }

    #[cfg(feature = "live")]
    #[tokio::test]
    async fn poll_transcript_returns_the_forwarded_error() {
        let mut stt = AwsStt::new(&SttConfig {
            provider: SttVendor::Aws,
            model: None,
            model_path: None,
            language: None,
            api_key: None,
            endpoint: None,
            transport: None,
        })
        .unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut inner = stt.state.lock().await;
            inner.running = true;
            inner.transcript_rx = Some(rx);
        }
        let _ = tx.send(Ok(SttTranscript::Partial("a".into())));
        let _ = tx.send(Err(SpeechError::Vendor {
            vendor: "aws".into(),
            message: "boom".into(),
        }));
        assert!(matches!(
            stt.poll_transcript().await,
            Ok(Some(SttTranscript::Partial(_)))
        ));
        assert!(matches!(
            stt.poll_transcript().await,
            Err(SpeechError::Vendor { .. })
        ));
        assert!(matches!(stt.poll_transcript().await, Ok(None)));
        assert!(stt.emits_partials());
    }

    /// Fake service: reads the input like the SDK would and answers when the input ends.
    fn fake_service(
        mut pcm_rx: mpsc::UnboundedReceiver<Bytes>,
        tx: mpsc::UnboundedSender<SttItem>,
        reply: Vec<SttItem>,
    ) -> (
        tokio::task::JoinHandle<()>,
        Arc<std::sync::Mutex<Vec<usize>>>,
    ) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_task = seen.clone();
        let task = tokio::spawn(async move {
            while let Some(chunk) = pcm_rx.recv().await {
                seen_task.lock().unwrap().push(chunk.len());
            }
            // The input closed: the end frame reached the service. Answer, then close.
            for item in reply {
                let _ = tx.send(item);
            }
        });
        (task, seen)
    }

    fn stt_for_tests() -> AwsStt {
        AwsStt::new(&SttConfig {
            provider: SttVendor::Aws,
            model: None,
            model_path: None,
            language: None,
            api_key: None,
            endpoint: None,
            transport: None,
        })
        .unwrap()
    }

    /// The input ends with an empty audio event, then closes; the results the service sends
    /// before it closes are returned by `poll_transcript`, the last one a final.
    #[tokio::test]
    async fn finalize_ends_the_input_and_the_final_is_returned() {
        let mut stt = stt_for_tests();
        let (pcm_tx, pcm_rx) = mpsc::unbounded_channel();
        let (tx, rx) = mpsc::unbounded_channel();
        let (service, seen) = fake_service(
            pcm_rx,
            tx,
            vec![
                Ok(SttTranscript::Partial("the quick".into())),
                Ok(SttTranscript::Final("the quick brown fox".into())),
            ],
        );
        {
            let mut inner = stt.state.lock().await;
            inner.running = true;
            inner.pcm_tx = Some(pcm_tx);
            inner.transcript_rx = Some(rx);
        }
        stt.push_audio(Bytes::from(vec![1u8; 640])).await.unwrap();
        stt.finalize_utterance().await.unwrap();
        service.await.unwrap();
        // 640 audio bytes, then the empty event.
        assert_eq!(*seen.lock().unwrap(), vec![640, 0]);
        assert!(stt.state.lock().await.pcm_tx.is_none(), "input is closed");
        assert!(matches!(
            stt.poll_transcript().await,
            Ok(Some(SttTranscript::Partial(t))) if t == "the quick"
        ));
        assert!(matches!(
            stt.poll_transcript().await,
            Ok(Some(SttTranscript::Final(t))) if t == "the quick brown fox"
        ));
        assert!(matches!(stt.poll_transcript().await, Ok(None)));
    }

    /// After a finalize the stream is spent: the next push finds no input and has to open a
    /// new stream (the SDK open itself is the live test's part).
    #[tokio::test]
    async fn push_after_finalize_needs_a_new_stream() {
        let mut stt = stt_for_tests();
        let (pcm_tx, pcm_rx) = mpsc::unbounded_channel();
        let (tx, rx) = mpsc::unbounded_channel();
        let (service, _seen) = fake_service(pcm_rx, tx, vec![]);
        {
            let mut inner = stt.state.lock().await;
            inner.running = true;
            inner.pcm_tx = Some(pcm_tx);
            inner.transcript_rx = Some(rx);
        }
        stt.finalize_utterance().await.unwrap();
        service.await.unwrap();
        let inner = stt.state.lock().await;
        assert!(
            inner.pcm_tx.is_none(),
            "no input channel after the finalize"
        );
        assert!(inner.running, "the provider is still running");
        drop(inner);
        // A second finalize with no stream is a no-op, not an error.
        stt.finalize_utterance().await.unwrap();
    }

    /// A stream error while waiting for the final is an `Err` from the finalize, and results
    /// before it are still delivered.
    #[tokio::test]
    async fn finalize_returns_a_stream_error() {
        let (pcm_tx, _pcm_rx) = mpsc::unbounded_channel();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _ = tx.send(Ok(SttTranscript::Partial("a".into())));
        let _ = tx.send(Err(SpeechError::Vendor {
            vendor: "aws".into(),
            message: "boom".into(),
        }));
        let mut pending = std::collections::VecDeque::new();
        let err = end_utterance(
            pcm_tx,
            &mut rx,
            &mut pending,
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("boom"), "{err}");
        assert_eq!(pending.len(), 1, "the partial before the error is kept");
    }

    /// The first open (`start`) goes through the same bound as the lazy re-open.
    #[tokio::test]
    async fn stream_open_that_never_answers_is_a_named_timeout_error() {
        let started = std::time::Instant::now();
        let err = bound_open(
            std::time::Duration::from_millis(100),
            std::future::pending::<()>(),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("transcribe stream open timed out after 100 ms"),
            "{err}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(
            bound_open(std::time::Duration::from_secs(5), async { 7 })
                .await
                .unwrap(),
            7
        );
    }

    /// A service that never closes is bounded by the finalize wait, as an `Err`.
    #[tokio::test]
    async fn finalize_is_bounded_when_the_service_never_closes() {
        let (pcm_tx, _pcm_rx) = mpsc::unbounded_channel();
        let (_tx, mut rx) = mpsc::unbounded_channel::<SttItem>();
        let mut pending = std::collections::VecDeque::new();
        let started = std::time::Instant::now();
        let err = end_utterance(
            pcm_tx,
            &mut rx,
            &mut pending,
            std::time::Duration::from_millis(200),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("within 200 ms"), "{err}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
