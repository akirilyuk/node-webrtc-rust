//! Live Google STT HTTP / V2 streaming helpers.
//!
//! V1 recognize: `POST https://speech.googleapis.com/v1/speech:recognize`
//! V2 streaming: `Speech.StreamingRecognize` with `interim_results` — `matrix::STT_V2_STREAMING_DOC`.

use std::sync::Mutex;

use bytes::Bytes;
use google_cloud_speech_v2::client::Speech;
use google_cloud_speech_v2::model::{
    ExplicitDecodingConfig, RecognitionConfig, StreamingRecognitionConfig,
    StreamingRecognitionFeatures, StreamingRecognizeRequest,
};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::STT_PCM_SAMPLE_RATE;
use node_webrtc_rust_speech::pipeline::SttTranscript;
use tokio::sync::mpsc;

use crate::auth::{google_access_token, google_api_key};
use crate::matrix::GoogleSttV2Locator;

/// Builds the gRPC client a provider keeps for its whole life.
pub async fn build_grpc_client() -> SpeechResult<Speech> {
    Speech::builder()
        .build()
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })
}

/// `url_override` is a test hook: POST there with a fixed bearer token instead of the Google
/// endpoint with real credentials.
pub async fn v1_recognize(
    client: &reqwest::Client,
    transport: &Mutex<Option<String>>,
    url_override: Option<&str>,
    pcm: Bytes,
    language: &str,
) -> SpeechResult<String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde_json::json;

    let body = json!({
        "config": {
            "encoding": "LINEAR16",
            "sampleRateHertz": STT_PCM_SAMPLE_RATE,
            "languageCode": language,
            "enableAutomaticPunctuation": true
        },
        "audio": {
            "content": STANDARD.encode(pcm.as_ref())
        }
    });

    let request = if let Some(url) = url_override {
        client.post(url).bearer_auth("test-token")
    } else if let Some(api_key) = google_api_key() {
        client.post(format!(
            "https://speech.googleapis.com/v1/speech:recognize?key={api_key}"
        ))
    } else {
        let token = google_access_token()
            .await?
            .ok_or_else(|| SpeechError::Vendor {
                vendor: "google".into(),
                message: "failed to obtain Google access token".into(),
            })?;
        client
            .post("https://speech.googleapis.com/v1/speech:recognize")
            .bearer_auth(token)
    };

    let response = request
        .json(&body)
        .send()
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;

    crate::http::note(transport, crate::http::transport_label(response.version()));
    let status = response.status();
    let payload: serde_json::Value = response.json().await.map_err(|err| SpeechError::Vendor {
        vendor: "google".into(),
        message: err.to_string(),
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

#[derive(Clone)]
pub struct V2StreamHandle {
    audio_tx: mpsc::UnboundedSender<Bytes>,
}

impl V2StreamHandle {
    pub async fn push_audio(&self, pcm: Bytes) -> SpeechResult<()> {
        let _ = self.audio_tx.send(pcm);
        Ok(())
    }

    pub async fn finalize_stream(&self) -> SpeechResult<()> {
        Ok(())
    }

    pub async fn stop(&self) {}
}

pub async fn start_v2_streaming(
    client: &Speech,
    transport: &Mutex<Option<String>>,
    locator: &GoogleSttV2Locator,
    model: &str,
    language: &str,
) -> SpeechResult<(mpsc::UnboundedReceiver<SttTranscript>, V2StreamHandle)> {
    crate::http::note(transport, "grpc".into());

    let recognizer = format!(
        "projects/{}/locations/{}/recognizers/{}",
        locator.project, locator.location, locator.recognizer
    );

    let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Bytes>();
    let (transcript_tx, transcript_rx) = mpsc::unbounded_channel::<SttTranscript>();

    let (grpc_tx, mut grpc_rx) = client.streaming_recognize().build();

    let decoding = ExplicitDecodingConfig::new()
        .set_encoding("LINEAR16")
        .set_sample_rate_hertz(STT_PCM_SAMPLE_RATE as i32)
        .set_audio_channel_count(1);

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

    grpc_tx
        .send(config_msg)
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;

    tokio::spawn(async move {
        while let Some(chunk) = audio_rx.recv().await {
            let msg = StreamingRecognizeRequest::new()
                .set_recognizer(recognizer.clone())
                .set_audio(chunk);
            if grpc_tx.send(msg).await.is_err() {
                break;
            }
        }
        drop(grpc_tx);
    });

    tokio::spawn(async move {
        while let Some(response) = grpc_rx.next().await {
            let Ok(response) = response else { break };
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
    });

    Ok((
        transcript_rx,
        V2StreamHandle {
            audio_tx: audio_tx.clone(),
        },
    ))
}
