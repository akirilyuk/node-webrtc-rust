//! Live Google TTS REST + StreamingSynthesize helpers.
//!
//! REST: `POST https://texttospeech.googleapis.com/v1/text:synthesize`
//! Streaming: `StreamingSynthesize` — `matrix::TTS_STREAMING_DOC`

use futures_util::StreamExt;
use google_cloud_texttospeech_v1::client::TextToSpeech;
use google_cloud_texttospeech_v1::model::streaming_synthesis_input::InputSource;
use google_cloud_texttospeech_v1::model::streaming_synthesize_request::StreamingRequest;
use google_cloud_texttospeech_v1::model::{
    StreamingSynthesisInput, StreamingSynthesizeConfig, StreamingSynthesizeRequest,
    VoiceSelectionParams,
};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink};

use crate::auth::{google_access_token, google_api_key};

pub async fn synthesize_linear16(text: &str, voice: &str, language: &str) -> SpeechResult<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde_json::json;

    let client = reqwest::Client::new();
    let body = json!({
        "input": { "text": text },
        "voice": {
            "languageCode": language,
            "name": voice
        },
        "audioConfig": {
            "audioEncoding": "LINEAR16",
            "sampleRateHertz": WEBRTC_PCM_SAMPLE_RATE,
            "speakingRate": 1.0
        }
    });

    let request = if let Some(api_key) = google_api_key() {
        client.post(format!(
            "https://texttospeech.googleapis.com/v1/text:synthesize?key={api_key}"
        ))
    } else {
        let token = google_access_token()
            .await?
            .ok_or_else(|| SpeechError::Vendor {
                vendor: "google".into(),
                message: "failed to obtain Google access token".into(),
            })?;
        client
            .post("https://texttospeech.googleapis.com/v1/text:synthesize")
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

    let audio_b64 = payload
        .get("audioContent")
        .and_then(|value| value.as_str())
        .ok_or_else(|| SpeechError::Vendor {
            vendor: "google".into(),
            message: "missing audioContent in TTS response".into(),
        })?;

    STANDARD
        .decode(audio_b64)
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })
}

pub async fn streaming_synthesize(
    voice: &str,
    language: &str,
    text: &str,
    sink: Option<TtsProgressiveSink>,
) -> SpeechResult<Vec<TtsAudioChunk>> {
    let mut client = TextToSpeech::builder()
        .build()
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;

    let (grpc_tx, mut grpc_rx) = client.streaming_synthesize().build();

    let config = StreamingSynthesizeRequest::new().set_streaming_request(
        StreamingRequest::StreamingConfig(
            StreamingSynthesizeConfig::new()
                .set_voice(
                    VoiceSelectionParams::new()
                        .set_name(voice)
                        .set_language_code(language),
                )
                .into(),
        ),
    );

    let input = StreamingSynthesizeRequest::new().set_streaming_request(
        StreamingRequest::Input(
            StreamingSynthesisInput::new()
                .set_input_source(Some(InputSource::Text(text.to_string())))
                .into(),
        ),
    );

    grpc_tx
        .send(config)
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;
    grpc_tx
        .send(input)
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;
    drop(grpc_tx);

    let mut mono = Vec::new();
    while let Some(msg) = grpc_rx.next().await {
        let response = msg.map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;
        if response.audio_content.is_empty() {
            continue;
        }
        mono.extend_from_slice(&response.audio_content);
        if let Some(s) = sink.as_ref() {
            if s.is_cancelled() {
                break;
            }
            let duration_ms =
                duration_ms_from_mono_s16le(response.audio_content.len(), WEBRTC_PCM_SAMPLE_RATE);
            let pcm = mono_s16le_to_stereo(&response.audio_content);
            let _ = s.send(TtsAudioChunk { pcm, duration_ms });
        }
    }

    let duration_ms = duration_ms_from_mono_s16le(mono.len(), WEBRTC_PCM_SAMPLE_RATE);
    let pcm = mono_s16le_to_stereo(&mono);
    Ok(vec![TtsAudioChunk { pcm, duration_ms }])
}
