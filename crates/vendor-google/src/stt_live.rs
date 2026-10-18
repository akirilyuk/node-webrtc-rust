//! Live Google STT helpers.
//!
//! V1 recognize: `POST https://speech.googleapis.com/v1/speech:recognize`, one request per
//! utterance, a final only (REST credentials: `crate::auth`).
//!
//! V2 streaming: `Speech.StreamingRecognize` with `interim_results` (`matrix::STT_V2_STREAMING_DOC`),
//! OAuth only. One stream per utterance: the stream opens with the first audio of the utterance,
//! and `finalize_stream` half-closes the request side so the server flushes its final results and
//! ends the response stream, which the finalize waits for (bounded by `finalize_wait_ms()`).
//! Streaming guide: "streaming speech recognition is available through gRPC only" and "There is a
//! 25 KB limit on audio sent in the requests of a stream", so audio goes out in pieces of at most
//! [`MAX_AUDIO_MESSAGE`] bytes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use google_cloud_speech_v2::client::Speech;
use google_cloud_speech_v2::model::{
    ExplicitDecodingConfig, RecognitionConfig, StreamingRecognitionConfig,
    StreamingRecognitionFeatures, StreamingRecognizeRequest,
};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::STT_PCM_SAMPLE_RATE;
use node_webrtc_rust_speech::pipeline::SttTranscript;
use tokio::sync::{mpsc, watch};

use crate::auth::{grpc_credentials, rest_auth, RestAuth};
use crate::matrix::{GoogleSttV2Locator, CONNECT_TIMEOUT};

/// Longest audio piece per request (documented limit: 25 KB).
pub const MAX_AUDIO_MESSAGE: usize = 24_000;
/// Bound for sending one request message into the stream.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);

fn stt_err(message: impl Into<String>) -> SpeechError {
    SpeechError::Stt(message.into())
}

/// Builds the gRPC client a provider keeps for its whole life: OAuth from the service account
/// file only, the endpoint of the recognizer's location, connect bounded by [`CONNECT_TIMEOUT`].
pub async fn build_grpc_client(
    credentials_file: Option<&str>,
    locator: &GoogleSttV2Locator,
    model: &str,
) -> SpeechResult<Speech> {
    let credentials = grpc_credentials(
        credentials_file,
        &format!("stt {model}"),
        "latest_long (REST)",
    )
    .await?;
    let mut builder = Speech::builder().with_credentials(credentials);
    if let Some(endpoint) = locator.endpoint() {
        builder = builder.with_endpoint(endpoint);
    }
    match tokio::time::timeout(CONNECT_TIMEOUT, builder.build()).await {
        Ok(result) => result.map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        }),
        Err(_) => Err(stt_err(format!(
            "google stt: gRPC client did not connect within {} ms",
            CONNECT_TIMEOUT.as_millis()
        ))),
    }
}

/// `url_override` is a test hook: POST there with a fixed bearer token instead of the Google
/// endpoint with real credentials.
pub async fn v1_recognize(
    client: &reqwest::Client,
    transport: &Mutex<Option<String>>,
    url_override: Option<&str>,
    config_api_key: Option<&str>,
    pcm: Bytes,
    language: &str,
    model: &str,
) -> SpeechResult<String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde_json::json;

    let body = json!({
        "config": {
            "encoding": "LINEAR16",
            "sampleRateHertz": STT_PCM_SAMPLE_RATE,
            "languageCode": language,
            "model": model,
            "enableAutomaticPunctuation": true
        },
        "audio": {
            "content": STANDARD.encode(pcm.as_ref())
        }
    });

    const URL: &str = "https://speech.googleapis.com/v1/speech:recognize";
    let request = if let Some(url) = url_override {
        client.post(url).bearer_auth("test-token")
    } else {
        match rest_auth(config_api_key).await? {
            RestAuth::ApiKey(key) => client.post(URL).query(&[("key", key)]),
            RestAuth::Bearer(token) => client.post(URL).bearer_auth(token),
        }
    };

    // `without_url`: the request URL carries the API key.
    let response = request
        .json(&body)
        .send()
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.without_url().to_string(),
        })?;

    crate::http::note(transport, crate::http::transport_label(response.version()));
    let status = response.status();
    let payload: serde_json::Value = response.json().await.map_err(|err| SpeechError::Vendor {
        vendor: "google".into(),
        message: err.without_url().to_string(),
    })?;

    if !status.is_success() {
        return Err(SpeechError::Vendor {
            vendor: "google".into(),
            message: payload.to_string(),
        });
    }

    Ok(payload
        .pointer("/results/0/alternatives/0/transcript")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim()
        .to_string())
}

struct Inner {
    audio_tx: Mutex<Option<mpsc::UnboundedSender<Bytes>>>,
    done: watch::Receiver<bool>,
    error: Mutex<Option<String>>,
    tasks: Mutex<Vec<tokio::task::AbortHandle>>,
}

/// One open V2 stream (one utterance).
#[derive(Clone)]
pub struct V2StreamHandle {
    inner: Arc<Inner>,
}

impl V2StreamHandle {
    pub async fn push_audio(&self, pcm: Bytes) -> SpeechResult<()> {
        self.take_error()?;
        if let Some(tx) = self.inner.audio_tx.lock().expect("audio_tx").as_ref() {
            let _ = tx.send(pcm);
        }
        Ok(())
    }

    /// True once the request side was half-closed or the response stream ended.
    pub fn is_spent(&self) -> bool {
        self.inner.audio_tx.lock().expect("audio_tx").is_none() || *self.inner.done.borrow()
    }

    /// The stream's failure, once; named, never audio or transcripts.
    pub fn take_error(&self) -> SpeechResult<()> {
        match self.inner.error.lock().expect("error").take() {
            Some(message) => Err(stt_err(message)),
            None => Ok(()),
        }
    }

    /// Half-closes the request side and waits until the server ended the response stream, so
    /// every final result is queued. `wait` = `finalize_wait_ms()`.
    pub async fn finalize_stream(&self, wait: Duration) -> SpeechResult<()> {
        // Dropping the sender ends the writer task, which drops the gRPC sender (half-close).
        self.inner.audio_tx.lock().expect("audio_tx").take();
        let mut done = self.inner.done.clone();
        let finished = tokio::time::timeout(wait, done.wait_for(|finished| *finished))
            .await
            .is_ok();
        if finished {
            self.take_error()
        } else {
            Err(stt_err(format!(
                "google stt: V2 stream gave no final within {} ms of the half-close",
                wait.as_millis()
            )))
        }
    }

    pub async fn stop(&self) {
        self.inner.audio_tx.lock().expect("audio_tx").take();
        for task in self.inner.tasks.lock().expect("tasks").drain(..) {
            task.abort();
        }
    }
}

/// Opens one stream. Results arrive on `transcript_tx`: interim results as `Partial`, final
/// results as `Final`.
pub async fn start_v2_streaming(
    client: &Speech,
    transport: &Mutex<Option<String>>,
    locator: &GoogleSttV2Locator,
    model: &str,
    language: &str,
    transcript_tx: mpsc::UnboundedSender<SttTranscript>,
) -> SpeechResult<V2StreamHandle> {
    crate::http::note(transport, "grpc".into());

    let recognizer = locator.resource_name();
    let (grpc_tx, mut grpc_rx) = client.streaming_recognize().build();

    let decoding = ExplicitDecodingConfig::new()
        .set_encoding("LINEAR16")
        .set_sample_rate_hertz(STT_PCM_SAMPLE_RATE as i32)
        .set_audio_channel_count(1);

    // Inline config: with the `_` recognizer the model and language travel here.
    let recognition_config = RecognitionConfig::new()
        .set_model(model)
        .set_language_codes([language.to_string()])
        .set_explicit_decoding_config(decoding);

    let streaming_config = StreamingRecognitionConfig::new()
        .set_config(recognition_config)
        .set_streaming_features(StreamingRecognitionFeatures::new().set_interim_results(true));

    let config_msg = StreamingRecognizeRequest::new()
        .set_recognizer(recognizer.clone())
        .set_streaming_config(streaming_config);

    match tokio::time::timeout(CONNECT_TIMEOUT, grpc_tx.send(config_msg)).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            return Err(SpeechError::Vendor {
                vendor: "google".into(),
                message: err.to_string(),
            })
        }
        Err(_) => {
            return Err(stt_err(format!(
                "google stt: V2 stream did not open within {} ms",
                CONNECT_TIMEOUT.as_millis()
            )))
        }
    }

    let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Bytes>();
    let (done_tx, done_rx) = watch::channel(false);
    let inner = Arc::new(Inner {
        audio_tx: Mutex::new(Some(audio_tx)),
        done: done_rx,
        error: Mutex::new(None),
        tasks: Mutex::new(Vec::new()),
    });

    let writer_inner = inner.clone();
    let writer = tokio::spawn(async move {
        while let Some(chunk) = audio_rx.recv().await {
            for piece in chunk.chunks(MAX_AUDIO_MESSAGE) {
                let msg = StreamingRecognizeRequest::new()
                    .set_recognizer(recognizer.clone())
                    .set_audio(Bytes::copy_from_slice(piece));
                match tokio::time::timeout(SEND_TIMEOUT, grpc_tx.send(msg)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => return,
                    Err(_) => {
                        *writer_inner.error.lock().expect("error") = Some(format!(
                            "google stt: sending audio did not finish within {} ms",
                            SEND_TIMEOUT.as_millis()
                        ));
                        return;
                    }
                }
            }
        }
        // The sender was dropped: `grpc_tx` drops here, which half-closes the stream.
    });

    let reader_inner = inner.clone();
    let reader = tokio::spawn(async move {
        while let Some(response) = grpc_rx.next().await {
            let response = match response {
                Ok(response) => response,
                Err(err) => {
                    *reader_inner.error.lock().expect("error") =
                        Some(format!("google stt: V2 stream failed: {err}"));
                    break;
                }
            };
            for result in response.results {
                let transcript = result
                    .alternatives
                    .first()
                    .map(|a| a.transcript.trim().to_string())
                    .unwrap_or_default();
                if transcript.is_empty() {
                    continue;
                }
                let update = if result.is_final {
                    SttTranscript::Final(transcript)
                } else {
                    SttTranscript::Partial(transcript)
                };
                let _ = transcript_tx.send(update);
            }
        }
        let _ = done_tx.send(true);
    });

    inner
        .tasks
        .lock()
        .expect("tasks")
        .extend([writer.abort_handle(), reader.abort_handle()]);
    Ok(V2StreamHandle { inner })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(
        done: bool,
    ) -> (
        V2StreamHandle,
        watch::Sender<bool>,
        mpsc::UnboundedReceiver<Bytes>,
    ) {
        let (audio_tx, audio_rx) = mpsc::unbounded_channel();
        let (done_tx, done_rx) = watch::channel(done);
        (
            V2StreamHandle {
                inner: Arc::new(Inner {
                    audio_tx: Mutex::new(Some(audio_tx)),
                    done: done_rx,
                    error: Mutex::new(None),
                    tasks: Mutex::new(Vec::new()),
                }),
            },
            done_tx,
            audio_rx,
        )
    }

    #[tokio::test]
    async fn finalize_half_closes_then_waits_for_the_end_of_the_response_stream() {
        let (handle, done_tx, mut audio_rx) = handle(false);
        let waiting = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.finalize_stream(Duration::from_secs(5)).await })
        };
        // The request side closes first: the writer sees the end of the audio channel.
        assert!(audio_rx.recv().await.is_none());
        assert!(!waiting.is_finished());
        done_tx.send(true).unwrap();
        waiting.await.unwrap().unwrap();
        assert!(handle.is_spent());
    }

    #[tokio::test]
    async fn finalize_that_never_gets_the_end_of_the_stream_fails_named_within_the_bound() {
        let (handle, _done_tx, _audio_rx) = handle(false);
        let started = std::time::Instant::now();
        let err = handle
            .finalize_stream(Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        let message = err.to_string();
        assert!(message.contains("no final within 100 ms"), "{message}");
    }

    #[tokio::test]
    async fn a_stream_error_is_reported_once_by_finalize() {
        let (handle, _done_tx, _audio_rx) = handle(true);
        *handle.inner.error.lock().unwrap() = Some("google stt: V2 stream failed: x".into());
        assert!(handle
            .finalize_stream(Duration::from_secs(1))
            .await
            .is_err());
        assert!(handle.take_error().is_ok());
    }
}
