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
}

struct AwsSttInner {
    running: bool,
    pcm_tx: Option<mpsc::UnboundedSender<Bytes>>,
    transcript_rx: Option<mpsc::UnboundedReceiver<SttTranscript>>,
    reader_task: Option<tokio::task::JoinHandle<()>>,
}

impl AwsStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
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
            })),
        })
    }
}

#[async_trait]
impl SttProvider for AwsStt {
    fn vendor_name(&self) -> &'static str {
        "aws"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        #[cfg(feature = "live")]
        {
            use aws_sdk_transcribestreaming::types::MediaEncoding;
            use aws_sdk_transcribestreaming::primitives::event_stream::EventStreamSender;

            let language_code = self.language_code.clone();
            let (pcm_tx, pcm_rx) = mpsc::unbounded_channel::<Bytes>();
            let (transcript_tx, transcript_rx) = mpsc::unbounded_channel::<SttTranscript>();

            let audio_stream = EventStreamSender::from(PcmAudioStream { rx: pcm_rx });

            let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                .load()
                .await;
            let client = aws_sdk_transcribestreaming::Client::new(&config);
            let language = language_code_to_aws(&language_code).map_err(SpeechError::Config)?;

            let response = client
                .start_stream_transcription()
                .language_code(language)
                .media_sample_rate_hertz(16_000)
                .media_encoding(MediaEncoding::Pcm)
                .audio_stream(audio_stream)
                .send()
                .await
                .map_err(|err| SpeechError::Vendor {
                    vendor: "aws".into(),
                    message: err.to_string(),
                })?;

            let mut transcript_stream = response.transcript_result_stream;

            let reader_task = tokio::spawn(async move {
                while let Ok(Some(event)) = transcript_stream.recv().await {
                    if let Some(transcript) = transcript_from_event(&event) {
                        let _ = transcript_tx.send(transcript);
                    }
                }
            });

            let mut inner = self.state.lock().await;
            inner.running = true;
            inner.pcm_tx = Some(pcm_tx);
            inner.transcript_rx = Some(transcript_rx);
            inner.reader_task = Some(reader_task);
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
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let inner = self.state.lock().await;
        if !inner.running {
            return Ok(());
        }
        if let Some(tx) = &inner.pcm_tx {
            let _ = tx.send(pcm);
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let mut inner = self.state.lock().await;
        if !inner.running {
            return Ok(None);
        }
        if let Some(rx) = inner.transcript_rx.as_mut() {
            return Ok(rx.try_recv().ok());
        }
        Ok(None)
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
                    AudioEvent::builder()
                        .audio_chunk(Blob::new(chunk))
                        .build(),
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
            }),
            Err(SpeechError::Config(_))
        ));
    }
}
