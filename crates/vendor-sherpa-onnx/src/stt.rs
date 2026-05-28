use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::mono_s16le_bytes_to_f32;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig, OnlineStream};
use tokio::sync::Mutex;

use crate::model_paths::resolve_model_paths;

const SAMPLE_RATE: i32 = 16_000;

static SHERPA_PUSH_COUNT: AtomicU64 = AtomicU64::new(0);

fn voice_debug_enabled() -> bool {
    matches!(
        std::env::var("VOICE_DEBUG").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn voice_debug(message: impl AsRef<str>) {
    if voice_debug_enabled() {
        eprintln!("[voice-debug] {}", message.as_ref());
    }
}

struct SherpaRuntime {
    recognizer: OnlineRecognizer,
    stream: OnlineStream,
    last_emitted_text: String,
    pending: VecDeque<SttTranscript>,
}

struct SherpaSttState {
    running: bool,
    runtime: Option<SherpaRuntime>,
}

pub struct SherpaStt {
    config: SttConfig,
    state: Arc<Mutex<SherpaSttState>>,
}

impl SherpaStt {
    pub fn new(config: &SttConfig) -> Self {
        Self {
            config: config.clone(),
            state: Arc::new(Mutex::new(SherpaSttState {
                running: false,
                runtime: None,
            })),
        }
    }

    fn build_runtime(
        config: &SttConfig,
    ) -> SpeechResult<SherpaRuntime> {
        let paths = resolve_model_paths(config)?;

        let mut recognizer_config = OnlineRecognizerConfig::default();
        recognizer_config.model_config.transducer.encoder =
            Some(path_to_string(&paths.encoder)?);
        recognizer_config.model_config.transducer.decoder =
            Some(path_to_string(&paths.decoder)?);
        recognizer_config.model_config.transducer.joiner =
            Some(path_to_string(&paths.joiner)?);
        recognizer_config.model_config.tokens = Some(path_to_string(&paths.tokens)?);
        recognizer_config.enable_endpoint = true;
        recognizer_config.rule1_min_trailing_silence = 2.4;
        recognizer_config.rule2_min_trailing_silence = 1.0;
        recognizer_config.rule3_min_utterance_length = 20.0;
        recognizer_config.decoding_method = Some("greedy_search".into());

        let recognizer = OnlineRecognizer::create(&recognizer_config).ok_or_else(|| {
            SpeechError::Vendor {
                vendor: "local-sherpa".into(),
                message: "failed to create OnlineRecognizer".into(),
            }
        })?;
        let stream = recognizer.create_stream();

        Ok(SherpaRuntime {
            recognizer,
            stream,
            last_emitted_text: String::new(),
            pending: VecDeque::new(),
        })
    }
}

fn path_to_string(path: &std::path::Path) -> SpeechResult<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| {
            SpeechError::Config(format!(
                "model path is not valid UTF-8: {}",
                path.display()
            ))
        })
}

#[async_trait]
impl SttProvider for SherpaStt {
    fn vendor_name(&self) -> &'static str {
        "local-sherpa"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        let config = self.config.clone();
        let state = Arc::clone(&self.state);

        tokio::task::spawn_blocking(move || -> SpeechResult<()> {
            let runtime = SherpaStt::build_runtime(&config)?;
            voice_debug("sherpa OnlineRecognizer ready");
            let mut guard = state.blocking_lock();
            guard.running = true;
            guard.runtime = Some(runtime);
            Ok(())
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))??;

        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let state = Arc::clone(&self.state);

        tokio::task::spawn_blocking(move || {
            let mut guard = state.blocking_lock();
            guard.running = false;
            if let Some(runtime) = guard.runtime.as_mut() {
                runtime.recognizer.reset(&runtime.stream);
                runtime.last_emitted_text.clear();
                runtime.pending.clear();
            }
            guard.runtime = None;
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))?;

        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let samples = mono_s16le_bytes_to_f32(pcm.as_ref());
        if samples.is_empty() {
            return Ok(());
        }

        let state = Arc::clone(&self.state);

        tokio::task::spawn_blocking(move || -> SpeechResult<()> {
            let mut guard = state.blocking_lock();
            if !guard.running {
                return Ok(());
            }
            let Some(runtime) = guard.runtime.as_mut() else {
                return Ok(());
            };

            runtime
                .stream
                .accept_waveform(SAMPLE_RATE, &samples);
            let mut decode_steps = 0u32;
            while runtime.recognizer.is_ready(&runtime.stream) {
                runtime.recognizer.decode(&runtime.stream);
                decode_steps = decode_steps.saturating_add(1);
                if decode_steps >= 64 {
                    voice_debug("sherpa decode loop capped at 64 steps (possible is_ready stuck)");
                    break;
                }
            }

            let push = SHERPA_PUSH_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if push == 1 || push % 50 == 0 {
                voice_debug(format!("sherpa push_audio samples={}", samples.len()));
            }

            Ok(())
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))??;

        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let state = Arc::clone(&self.state);

        tokio::task::spawn_blocking(move || -> SpeechResult<Option<SttTranscript>> {
            let mut guard = state.blocking_lock();
            if !guard.running {
                return Ok(None);
            }
            let Some(runtime) = guard.runtime.as_mut() else {
                return Ok(None);
            };

            if let Some(pending) = runtime.pending.pop_front() {
                return Ok(Some(pending));
            }

            let result = runtime.recognizer.get_result(&runtime.stream);
            let text = result
                .as_ref()
                .map(|value| value.text.trim())
                .unwrap_or("")
                .to_string();

            if text.is_empty() {
                return Ok(None);
            }

            let endpoint = runtime.recognizer.is_endpoint(&runtime.stream);
            voice_debug(format!("sherpa hypothesis={text:?} endpoint={endpoint}"));

            if endpoint {
                runtime.recognizer.reset(&runtime.stream);
                runtime.last_emitted_text.clear();
                return Ok(Some(SttTranscript::Final(text)));
            }

            if text == runtime.last_emitted_text {
                return Ok(None);
            }

            runtime.last_emitted_text = text.clone();
            Ok(Some(SttTranscript::Partial(text)))
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))?
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let state = Arc::clone(&self.state);

        tokio::task::spawn_blocking(move || -> SpeechResult<()> {
            let mut guard = state.blocking_lock();
            if !guard.running {
                return Ok(());
            }
            let Some(runtime) = guard.runtime.as_mut() else {
                return Ok(());
            };

            runtime.stream.input_finished();
            let mut decode_steps = 0u32;
            while runtime.recognizer.is_ready(&runtime.stream) {
                runtime.recognizer.decode(&runtime.stream);
                decode_steps = decode_steps.saturating_add(1);
                if decode_steps >= 64 {
                    break;
                }
            }

            let result = runtime.recognizer.get_result(&runtime.stream);
            let text = result
                .as_ref()
                .map(|value| value.text.trim())
                .unwrap_or("")
                .to_string();

            voice_debug(format!("sherpa finalize_utterance text={text:?}"));

            if !text.is_empty() {
                runtime.pending.push_back(SttTranscript::Final(text));
            }
            runtime.recognizer.reset(&runtime.stream);
            runtime.last_emitted_text.clear();
            Ok(())
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))??;

        Ok(())
    }
}
