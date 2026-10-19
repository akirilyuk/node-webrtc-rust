//! Live Google TTS REST + StreamingSynthesize helpers.
//!
//! REST: `POST https://texttospeech.googleapis.com/v1/text:synthesize`. The response is JSON with
//! base64 `audioContent`, so the audio arrives only when the whole synthesis is done (batch by
//! nature). Reference: "For LINEAR16 audio, we include the WAV header." and AudioConfig
//! `sampleRateHertz`: "if this is different from the voice's natural sample rate, then the
//! synthesizer will honor this request by converting to the desired sample rate". The WAV header
//! is parsed and the rate it declares is the one converted to 48 kHz.
//!
//! Streaming: `StreamingSynthesize` (`matrix::TTS_STREAMING_DOC`), Chirp 3 HD voices only, OAuth
//! only. Reference (StreamingSynthesizeResponse): "This is headerless LINEAR16 audio with a
//! sample rate of 24000." The stream's own `streaming_audio_config` is left unset, so the audio
//! is 24 kHz mono and goes through `MonoToStereo48k`.

use std::sync::Mutex;
use std::time::Duration;

use google_cloud_texttospeech_v1::client::TextToSpeech;
use google_cloud_texttospeech_v1::model::streaming_synthesis_input::InputSource;
use google_cloud_texttospeech_v1::model::streaming_synthesize_request::StreamingRequest;
use google_cloud_texttospeech_v1::model::{
    StreamingSynthesisInput, StreamingSynthesizeConfig, StreamingSynthesizeRequest,
    VoiceSelectionParams,
};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_stereo_48k, MonoToStereo48k, WEBRTC_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::auth::{grpc_credentials, rest_auth, RestAuth};
use crate::matrix::CONNECT_TIMEOUT;

/// Sample rate of `StreamingSynthesize` audio (headerless LINEAR16).
pub const STREAMING_SAMPLE_RATE: u32 = 24_000;
/// REST audio is handed to the sink in pieces of this many milliseconds.
pub const REST_CHUNK_MS: usize = 200;

/// Bounds of one `StreamingSynthesize` call.
#[derive(Debug, Clone, Copy)]
pub struct StreamLimits {
    /// Request sent to the first audio.
    pub first_audio: Duration,
    /// Longest gap between two audio messages.
    pub idle: Duration,
    /// Whole call.
    pub total: Duration,
}

pub const STREAM_LIMITS: StreamLimits = StreamLimits {
    first_audio: Duration::from_secs(15),
    idle: Duration::from_secs(10),
    total: Duration::from_secs(60),
};

fn tts_err(message: impl Into<String>) -> SpeechError {
    SpeechError::Tts(message.into())
}

/// Builds the gRPC client a provider keeps for its whole life: OAuth from the service account
/// file only, connect bounded by [`CONNECT_TIMEOUT`].
pub async fn build_grpc_client(
    credentials_file: Option<&str>,
    voice: &str,
) -> SpeechResult<TextToSpeech> {
    let credentials = grpc_credentials(
        credentials_file,
        &format!("tts {voice}"),
        "REST voices such as Neural2",
    )
    .await?;
    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        TextToSpeech::builder()
            .with_credentials(credentials)
            .build(),
    )
    .await
    {
        Ok(result) => result.map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        }),
        Err(_) => Err(tts_err(format!(
            "google tts: gRPC client did not connect within {} ms",
            CONNECT_TIMEOUT.as_millis()
        ))),
    }
}

/// Mono s16le PCM and its sample rate out of a LINEAR16 WAV body. A body without a `RIFF` header
/// is raw PCM at `fallback_rate`.
pub fn wav_pcm(body: &[u8], fallback_rate: u32) -> SpeechResult<(u32, Vec<u8>)> {
    if !body.starts_with(b"RIFF") {
        return Ok((fallback_rate, body.to_vec()));
    }
    if body.len() < 12 || &body[8..12] != b"WAVE" {
        return Err(tts_err("google tts: audioContent is not a WAV file"));
    }
    let mut rate = None;
    let mut pos = 12;
    while pos + 8 <= body.len() {
        let id = &body[pos..pos + 4];
        let len = u32::from_le_bytes(body[pos + 4..pos + 8].try_into().expect("4 bytes")) as usize;
        let start = pos + 8;
        if id == b"fmt " {
            if len < 16 || start + 16 > body.len() {
                return Err(tts_err("google tts: truncated WAV fmt chunk"));
            }
            let format = u16::from_le_bytes([body[start], body[start + 1]]);
            let channels = u16::from_le_bytes([body[start + 2], body[start + 3]]);
            let bits = u16::from_le_bytes([body[start + 14], body[start + 15]]);
            if format != 1 || channels != 1 || bits != 16 {
                return Err(tts_err(format!(
                    "google tts: unsupported WAV (format {format}, {channels} channels, {bits} bits); expected 16-bit mono PCM"
                )));
            }
            rate = Some(u32::from_le_bytes(
                body[start + 4..start + 8].try_into().expect("4 bytes"),
            ));
        } else if id == b"data" {
            let rate =
                rate.ok_or_else(|| tts_err("google tts: WAV data chunk before the fmt chunk"))?;
            // The declared size may be a placeholder; the payload ends where the body ends.
            let end = start.saturating_add(len).min(body.len());
            return Ok((rate, body[start..end].to_vec()));
        }
        pos = start + len + (len & 1);
    }
    Err(tts_err("google tts: WAV body has no data chunk"))
}

/// 48 kHz stereo bytes as sink-sized chunks.
pub fn stereo_chunks(stereo: &[u8]) -> Vec<TtsAudioChunk> {
    let piece = REST_CHUNK_MS * 192;
    stereo
        .chunks(piece)
        .map(|part| TtsAudioChunk {
            duration_ms: duration_ms_from_stereo_48k(part.len()),
            pcm: bytes::Bytes::copy_from_slice(part),
        })
        .collect()
}

/// `url_override` is a test hook: POST there with a fixed bearer token instead of the Google
/// endpoint with real credentials. Returns the audio as 48 kHz stereo s16le.
pub async fn synthesize_linear16(
    client: &reqwest::Client,
    transport: &Mutex<Option<String>>,
    url_override: Option<&str>,
    config_api_key: Option<&str>,
    text: &str,
    voice: &str,
    language: &str,
) -> SpeechResult<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde_json::json;

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

    const URL: &str = "https://texttospeech.googleapis.com/v1/text:synthesize";
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

    let audio_b64 = payload
        .get("audioContent")
        .and_then(|value| value.as_str())
        .ok_or_else(|| SpeechError::Vendor {
            vendor: "google".into(),
            message: "missing audioContent in TTS response".into(),
        })?;

    let wav = STANDARD
        .decode(audio_b64)
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;
    let (rate, mono) = wav_pcm(&wav, WEBRTC_PCM_SAMPLE_RATE)?;
    let mut up = MonoToStereo48k::new(rate)?;
    let mut stereo = up.push(&mono);
    stereo.extend(up.finish());
    Ok(stereo)
}

/// Reads audio messages from `rx` until it ends, converting 24 kHz mono to 48 kHz stereo chunks
/// that go to `sink` as they arrive and are returned (the same chunks, once). Bounded by
/// `limits`; a failed message, a stall and a stream without audio are named errors.
pub async fn collect_stream(
    mut rx: mpsc::Receiver<SpeechResult<Vec<u8>>>,
    limits: StreamLimits,
    sink: Option<&TtsProgressiveSink>,
) -> SpeechResult<Vec<TtsAudioChunk>> {
    let started = Instant::now();
    let mut up = MonoToStereo48k::new(STREAMING_SAMPLE_RATE)?;
    let mut chunks: Vec<TtsAudioChunk> = Vec::new();
    let mut cancelled = false;
    let emit = |stereo: Vec<u8>, chunks: &mut Vec<TtsAudioChunk>| {
        if stereo.is_empty() {
            return;
        }
        let chunk = TtsAudioChunk {
            duration_ms: duration_ms_from_stereo_48k(stereo.len()),
            pcm: bytes::Bytes::from(stereo),
        };
        if let Some(s) = sink {
            let _ = s.send(chunk.clone());
        }
        chunks.push(chunk);
    };
    loop {
        let (wait, what) = if chunks.is_empty() {
            (limits.first_audio, "no audio within")
        } else {
            (limits.idle, "audio stalled for")
        };
        let left = limits.total.saturating_sub(started.elapsed());
        let (bound, what) = if left < wait {
            (left, "synthesis exceeded")
        } else {
            (wait, what)
        };
        let bound_ms = if what == "synthesis exceeded" {
            limits.total
        } else {
            bound
        };
        let next = match tokio::time::timeout(bound, rx.recv()).await {
            Ok(next) => next,
            Err(_) => {
                return Err(tts_err(format!(
                    "google tts: StreamingSynthesize {what} {} ms",
                    bound_ms.as_millis()
                )))
            }
        };
        match next {
            None => break,
            Some(Err(err)) => return Err(err),
            Some(Ok(mono)) => {
                if mono.is_empty() {
                    continue;
                }
                if sink.is_some_and(TtsProgressiveSink::is_cancelled) {
                    cancelled = true;
                    break;
                }
                emit(up.push(&mono), &mut chunks);
            }
        }
    }
    if !cancelled {
        // The stream ended normally: flush the interpolator's held last sample.
        emit(up.finish(), &mut chunks);
    }
    if chunks.is_empty() {
        return Err(tts_err("google tts: StreamingSynthesize returned no audio"));
    }
    Ok(chunks)
}

pub async fn streaming_synthesize(
    client: &TextToSpeech,
    transport: &Mutex<Option<String>>,
    voice: &str,
    language: &str,
    text: &str,
    sink: Option<TtsProgressiveSink>,
) -> SpeechResult<Vec<TtsAudioChunk>> {
    crate::http::note(transport, "grpc".into());
    let (grpc_tx, mut grpc_rx) = client.streaming_synthesize().build();

    // Streaming guide: send exactly one config request, then the text input.
    let config =
        StreamingSynthesizeRequest::new().set_streaming_request(StreamingRequest::StreamingConfig(
            StreamingSynthesizeConfig::new()
                .set_voice(
                    VoiceSelectionParams::new()
                        .set_name(voice)
                        .set_language_code(language),
                )
                .into(),
        ));

    let input = StreamingSynthesizeRequest::new().set_streaming_request(StreamingRequest::Input(
        StreamingSynthesisInput::new()
            .set_input_source(Some(InputSource::Text(text.to_string())))
            .into(),
    ));

    for request in [config, input] {
        match tokio::time::timeout(CONNECT_TIMEOUT, grpc_tx.send(request)).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                return Err(SpeechError::Vendor {
                    vendor: "google".into(),
                    message: err.to_string(),
                })
            }
            Err(_) => {
                return Err(tts_err(format!(
                    "google tts: StreamingSynthesize request not accepted within {} ms",
                    CONNECT_TIMEOUT.as_millis()
                )))
            }
        }
    }
    drop(grpc_tx);

    let (tx, rx) = mpsc::channel::<SpeechResult<Vec<u8>>>(64);
    let pump = tokio::spawn(async move {
        while let Some(msg) = grpc_rx.next().await {
            let item = match msg {
                Ok(response) => Ok(response.audio_content.to_vec()),
                Err(err) => Err(SpeechError::Vendor {
                    vendor: "google".into(),
                    message: err.to_string(),
                }),
            };
            if tx.send(item).await.is_err() {
                return;
            }
        }
    });
    let result = collect_stream(rx, STREAM_LIMITS, sink.as_ref()).await;
    pump.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: StreamLimits = StreamLimits {
        first_audio: Duration::from_millis(150),
        idle: Duration::from_millis(150),
        total: Duration::from_millis(600),
    };

    /// 24 kHz mono s16le, `ms` long.
    fn mono_24k(ms: usize) -> Vec<u8> {
        (0..24 * ms)
            .flat_map(|i| (((i % 50) as i16 - 25) * 100).to_le_bytes())
            .collect()
    }

    fn wav(rate: u32, channels: u16, pcm: &[u8]) -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        out.extend_from_slice(pcm);
        out
    }

    #[test]
    fn wav_header_is_stripped_and_its_rate_used() {
        let pcm = mono_24k(100);
        let (rate, got) = wav_pcm(&wav(24_000, 1, &pcm), 48_000).unwrap();
        assert_eq!(rate, 24_000);
        assert_eq!(got, pcm);
    }

    #[test]
    fn headerless_body_uses_the_requested_rate() {
        let (rate, got) = wav_pcm(&[1, 2, 3, 4], 48_000).unwrap();
        assert_eq!((rate, got), (48_000, vec![1, 2, 3, 4]));
    }

    #[test]
    fn stereo_or_truncated_wav_is_a_named_error() {
        assert!(wav_pcm(&wav(24_000, 2, &[0; 8]), 48_000).is_err());
        assert!(wav_pcm(b"RIFF....WAVE", 48_000).is_err());
    }

    #[tokio::test]
    async fn one_second_of_24k_messages_is_one_second_of_48k_stereo() {
        let (tx, rx) = mpsc::channel(16);
        for _ in 0..10 {
            tx.send(Ok(mono_24k(100))).await.unwrap();
        }
        drop(tx);
        let chunks = collect_stream(rx, SHORT, None).await.unwrap();
        assert!(chunks.len() >= 10, "{} chunks", chunks.len());
        let bytes: usize = chunks.iter().map(|c| c.pcm.len()).sum();
        assert!((191_000..=193_000).contains(&bytes), "{bytes} bytes");
    }

    #[tokio::test]
    async fn a_stream_that_never_answers_fails_named_at_the_first_audio_bound() {
        let (_tx, rx) = mpsc::channel(1);
        let started = std::time::Instant::now();
        let err = collect_stream(rx, SHORT, None).await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(err.to_string().contains("no audio within 150 ms"), "{err}");
    }

    #[tokio::test]
    async fn a_stream_that_stalls_after_audio_fails_named_at_the_idle_bound() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(Ok(mono_24k(100))).await.unwrap();
        let err = collect_stream(rx, SHORT, None).await.unwrap_err();
        assert!(
            err.to_string().contains("audio stalled for 150 ms"),
            "{err}"
        );
        drop(tx);
    }

    #[tokio::test]
    async fn a_stream_that_trickles_past_the_total_bound_fails_named() {
        let (tx, rx) = mpsc::channel(4);
        let feeder = tokio::spawn(async move {
            loop {
                if tx.send(Ok(mono_24k(20))).await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        let err = collect_stream(rx, SHORT, None).await.unwrap_err();
        feeder.abort();
        assert!(
            err.to_string().contains("synthesis exceeded 600 ms"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_failed_message_and_an_empty_stream_are_errors() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(Err(tts_err("boom"))).await.unwrap();
        assert!(collect_stream(rx, SHORT, None).await.is_err());
        let (tx, rx) = mpsc::channel::<SpeechResult<Vec<u8>>>(1);
        drop(tx);
        let err = collect_stream(rx, SHORT, None).await.unwrap_err();
        assert!(err.to_string().contains("no audio"), "{err}");
    }
}
