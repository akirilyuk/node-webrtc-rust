//! Sherpa speaker embedding for the speaker gate.

use std::sync::{mpsc, Mutex};
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

type EmbedReply = oneshot::Sender<SpeechResult<Option<Vec<f32>>>>;

struct EmbedRequest {
    pcm: Bytes,
    reply: EmbedReply,
}

/// Starts the one worker thread of an embedder. It owns `compute` and serves requests in order.
/// A request whose caller already gave up (bounded wait timed out, reply dropped) is skipped
/// before computing, so a slow host never piles up threads or stale work. The thread ends when
/// the embedder (the sender) is dropped.
fn spawn_worker<F>(mut compute: F) -> SpeechResult<mpsc::Sender<EmbedRequest>>
where
    F: FnMut(&[u8]) -> SpeechResult<Option<Vec<f32>>> + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<EmbedRequest>();
    // Dedicated OS thread, like LID: not tokio's blocking pool shared with STT and TTS.
    thread::Builder::new()
        .name("sherpa-speaker".into())
        .spawn(move || {
            for req in rx {
                if req.reply.is_closed() {
                    continue;
                }
                let _ = req.reply.send(compute(req.pcm.as_ref()));
            }
        })
        .map_err(|err| SpeechError::Internal(err.to_string()))?;
    Ok(tx)
}

/// Speaker embedding extractor (one model, one persistent worker thread, calls serialised).
pub struct SherpaSpeakerEmbedder {
    worker: Mutex<mpsc::Sender<EmbedRequest>>,
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
        let worker = spawn_worker(move |pcm| Self::embed_blocking(&extractor, pcm))?;
        Ok(Self {
            worker: Mutex::new(worker),
        })
    }

    fn embed_blocking(
        guard: &SpeakerEmbeddingExtractor,
        pcm: &[u8],
    ) -> SpeechResult<Option<Vec<f32>>> {
        if pcm.is_empty() {
            return Ok(None);
        }
        let _native = NativeCallGuard::enter();
        let samples = mono_s16le_bytes_to_f32(pcm);
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
        let (reply, rx) = oneshot::channel();
        self.worker
            .lock()
            .map_err(|_| SpeechError::Internal("sherpa speaker lock poisoned".into()))?
            .send(EmbedRequest { pcm, reply })
            .map_err(|_| SpeechError::Internal("sherpa speaker worker stopped".into()))?;
        rx.await
            .map_err(|_| SpeechError::Internal("sherpa speaker worker dropped".into()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn worker_skips_requests_whose_caller_gave_up() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let c = Arc::clone(&calls);
        let tx = spawn_worker(move |_pcm| {
            c.fetch_add(1, Ordering::SeqCst);
            let _ = started_tx.send(());
            // The first call blocks until released, standing in for a slow 1-CPU host.
            let _ = gate_rx.recv_timeout(Duration::from_secs(5));
            Ok(Some(vec![1.0]))
        })
        .unwrap();
        let send = |pcm: &'static [u8]| {
            let (reply, rx) = oneshot::channel();
            tx.send(EmbedRequest {
                pcm: Bytes::from_static(pcm),
                reply,
            })
            .unwrap();
            rx
        };
        let first = send(b"a");
        // The worker is inside the first compute before the others are queued.
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // Three callers time out (drop their reply) while the worker is busy.
        for _ in 0..3 {
            drop(send(b"b"));
        }
        let live = send(b"c");
        drop(first);
        gate_tx.send(()).unwrap();
        gate_tx.send(()).unwrap();
        assert_eq!(live.await.unwrap().unwrap(), Some(vec![1.0]));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "abandoned requests are not computed"
        );
    }
}
