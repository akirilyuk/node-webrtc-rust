//! Sherpa speaker embedding for the speaker gate.

use std::sync::{Arc, Mutex};
use std::thread;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SpeakerGateConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::mono_s16le_bytes_to_f32;
use node_webrtc_rust_speech::pipeline::SpeakerEmbedder;
use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};
use tokio::sync::oneshot;

use crate::native_calls::NativeCallGuard;

const SAMPLE_RATE: i32 = 16_000;

/// Speaker embedding extractor (one model, one thread, calls serialised).
pub struct SherpaSpeakerEmbedder {
    extractor: Arc<Mutex<SpeakerEmbeddingExtractor>>,
}

impl SherpaSpeakerEmbedder {
    /// Loads the model now (blocking), so a bad `modelPath` fails at agent construction.
    pub fn new(config: &SpeakerGateConfig) -> SpeechResult<Self> {
        let model = config
            .model_path
            .clone()
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| {
                SpeechError::Config(
                    "speakerGate.modelPath is required when speakerGate.enabled".into(),
                )
            })?;
        if !std::path::Path::new(&model).is_file() {
            return Err(SpeechError::Config(format!(
                "speakerGate.modelPath does not exist: {model}"
            )));
        }
        let extractor = SpeakerEmbeddingExtractor::create(&SpeakerEmbeddingExtractorConfig {
            model: Some(model),
            num_threads: 1,
            debug: false,
            provider: Some("cpu".into()),
        })
        .ok_or_else(|| SpeechError::Vendor {
            vendor: "local-sherpa".into(),
            message: "SpeakerEmbeddingExtractor::create failed".into(),
        })?;
        Ok(Self {
            extractor: Arc::new(Mutex::new(extractor)),
        })
    }

    fn embed_blocking(
        extractor: &Mutex<SpeakerEmbeddingExtractor>,
        pcm: &[u8],
    ) -> SpeechResult<Option<Vec<f32>>> {
        if pcm.is_empty() {
            return Ok(None);
        }
        let _native = NativeCallGuard::enter();
        let samples = mono_s16le_bytes_to_f32(pcm);
        let guard = extractor
            .lock()
            .map_err(|_| SpeechError::Internal("sherpa speaker lock poisoned".into()))?;
        let Some(stream) = guard.create_stream() else {
            return Ok(None);
        };
        stream.accept_waveform(SAMPLE_RATE, &samples);
        stream.input_finished();
        if !guard.is_ready(&stream) {
            return Ok(None);
        }
        Ok(guard.compute(&stream))
    }
}

#[async_trait]
impl SpeakerEmbedder for SherpaSpeakerEmbedder {
    async fn embed(&self, pcm: Bytes, sample_rate: u32) -> SpeechResult<Option<Vec<f32>>> {
        if sample_rate != SAMPLE_RATE as u32 {
            return Err(SpeechError::Config(format!(
                "Sherpa speaker embedding expects {SAMPLE_RATE} Hz mono PCM, got {sample_rate} Hz"
            )));
        }
        let extractor = Arc::clone(&self.extractor);
        // Dedicated OS thread, like LID: not tokio's blocking pool shared with STT and TTS.
        let (tx, rx) = oneshot::channel();
        thread::Builder::new()
            .name("sherpa-speaker".into())
            .spawn(move || {
                let _ = tx.send(Self::embed_blocking(&extractor, pcm.as_ref()));
            })
            .map_err(|err| SpeechError::Internal(err.to_string()))?;
        rx.await
            .map_err(|_| SpeechError::Internal("sherpa speaker thread dropped".into()))?
    }
}
