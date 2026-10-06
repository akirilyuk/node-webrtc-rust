use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{TtsConfig, VoiceSessionContext};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::otel::{self, SherpaTtsMetricAttrs};
use node_webrtc_rust_speech::pcm::duration_ms_from_mono_s16le;
use node_webrtc_rust_speech::pcm::WEBRTC_PCM_SAMPLE_RATE;
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};
use sherpa_onnx::GenerationConfig;
use tokio::sync::Mutex;

use crate::audio::{align_stereo_pcm_to_20ms, slice_for_sink, StreamingStereo48kResampler};
use crate::phrase_cache::{
    build_cache_key, build_metric_attrs, lookup, normalize_phrase_text, phrase_cache_enabled, store,
};
use crate::pool::{SherpaModelPool, TtsEnginePool};
use crate::sentences::split_sentences;
use crate::tts_model_paths::resolve_tts_model_dir_path;

static TTS_GENERATE_COUNT: AtomicUsize = AtomicUsize::new(0);

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

pub struct SherpaTts {
    config: TtsConfig,
    pool: Arc<crate::pool::SherpaModelPool>,
    engine_pool: Arc<Mutex<Option<Arc<TtsEnginePool>>>>,
    speaker_id: i32,
    speed: f32,
    project_id: Arc<StdMutex<String>>,
    resolved_model_dir: Arc<Mutex<Option<String>>>,
}

impl SherpaTts {
    pub fn new(config: &TtsConfig) -> Self {
        Self {
            config: config.clone(),
            pool: SherpaModelPool::global(),
            engine_pool: Arc::new(Mutex::new(None)),
            speaker_id: parse_speaker_id(config),
            speed: parse_speed(config),
            project_id: Arc::new(StdMutex::new(String::new())),
            resolved_model_dir: Arc::new(Mutex::new(None)),
        }
    }

    async fn ensure_engine_pool(&self) -> SpeechResult<Arc<TtsEnginePool>> {
        let mut guard = self.engine_pool.lock().await;
        if let Some(pool) = guard.as_ref() {
            return Ok(Arc::clone(pool));
        }

        let config = self.config.clone();
        let pool = Arc::clone(&self.pool);
        let engine_pool = tokio::task::spawn_blocking(move || pool.get_or_create_tts(&config))
            .await
            .map_err(|err| SpeechError::Internal(err.to_string()))??;
        *guard = Some(Arc::clone(&engine_pool));
        Ok(engine_pool)
    }

    async fn resolved_model_dir(&self) -> SpeechResult<String> {
        let mut guard = self.resolved_model_dir.lock().await;
        if let Some(path) = guard.as_ref() {
            return Ok(path.clone());
        }
        let config = self.config.clone();
        let model_dir = tokio::task::spawn_blocking(move || resolve_tts_model_dir_path(&config))
            .await
            .map_err(|err| SpeechError::Internal(err.to_string()))??
            .display()
            .to_string();
        *guard = Some(model_dir.clone());
        Ok(model_dir)
    }

    fn session_project_id(&self) -> String {
        let from_session = self
            .project_id
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let trimmed = from_session.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        // Runner pods set PROJECT_ID; covers TTS before/without bind_session_context.
        std::env::var("PROJECT_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_default()
    }

    fn voice_label(&self) -> String {
        self.config
            .voice
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("0")
            .to_string()
    }

    fn language_label(&self) -> String {
        // Piper/Sherpa local TTS has no language field on TtsConfig; default en so
        // OTel `tts.language` is never dropped (empty attrs are omitted in Prometheus).
        std::env::var("SHERPA_TTS_LANGUAGE")
            .ok()
            .or_else(|| std::env::var("SHERPA_LANGUAGE").ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "en".to_string())
    }

    /// Synthesize `normalized` one sentence at a time.
    ///
    /// Every sentence takes the TTS permit and an engine on its own and releases both
    /// before the next one. The permit queue is FIFO, so a short request that arrives
    /// while a long one is mid-text waits for one sentence, not the whole text.
    ///
    /// One resampler spans all sentences so the 48 kHz phase stays continuous. The
    /// returned clip is the byte-for-byte concatenation of what was produced (padded to
    /// 20 ms), so it matches the progressive stream and needs no second resample.
    async fn synthesize_miss(
        &self,
        normalized: &str,
        attrs: &SherpaTtsMetricAttrs,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<(TtsAudioChunk, bool)> {
        let engine_pool = self.ensure_engine_pool().await?;
        let speaker_id = self.speaker_id;
        let speed = self.speed;
        let tts_semaphore = self.pool.tts_semaphore();

        let mut sentences = split_sentences(normalized);
        if sentences.is_empty() {
            // No letters or digits: keep the old behaviour and hand the text to the engine.
            sentences.push(normalized.to_string());
        }
        voice_debug(format!(
            "tts synthesis start text_len={} sentences={} stream={}",
            normalized.len(),
            sentences.len(),
            sink.is_some()
        ));

        let resampler: SharedResampler = Arc::new(StdMutex::new(None));
        let mut all_pcm: Vec<u8> = Vec::new();
        let mut completed = true;
        let mut synth_wall_ms = 0.0_f64;
        let mut counted = false;

        for sentence in sentences {
            if sink.as_ref().is_some_and(TtsProgressiveSink::is_cancelled) {
                completed = false;
                break;
            }

            let queue_wait_start = std::time::Instant::now();
            let permit = tts_semaphore
                .acquire()
                .await
                .map_err(|_| SpeechError::Internal("sherpa TTS semaphore closed".into()))?;
            let queue_wait_ms = queue_wait_start.elapsed().as_secs_f64() * 1000.0;
            otel::record_sherpa_pool_wait_ms(queue_wait_ms, Some(attrs));
            otel::record_sherpa_tts_queue_wait_ms(queue_wait_ms, attrs);

            if !counted {
                // One per synthesize_miss call, not per sentence.
                TTS_GENERATE_COUNT.fetch_add(1, Ordering::SeqCst);
                counted = true;
            }

            let sentence_start = std::time::Instant::now();
            let engine_pool = Arc::clone(&engine_pool);
            let sink_for_blocking = sink.clone();
            let resampler_for_blocking = Arc::clone(&resampler);
            let outcome = tokio::task::spawn_blocking(move || {
                synthesize_sentence(
                    &engine_pool,
                    &sentence,
                    speaker_id,
                    speed,
                    sink_for_blocking,
                    resampler_for_blocking,
                )
            })
            .await
            .map_err(|err| SpeechError::Internal(err.to_string()))??;
            drop(permit);
            synth_wall_ms += sentence_start.elapsed().as_secs_f64() * 1000.0;

            for pcm in &outcome.pcm {
                all_pcm.extend_from_slice(pcm);
            }
            if outcome.stopped || sink.as_ref().is_some_and(TtsProgressiveSink::is_cancelled) {
                completed = false;
                break;
            }
        }

        if !completed {
            otel::record_sherpa_tts_synth_wall_ms(synth_wall_ms, attrs);
            voice_debug(format!(
                "tts synthesis done wall_ms={synth_wall_ms:.0} audio_duration_ms=0 completed=false"
            ));
            return Ok((
                TtsAudioChunk {
                    pcm: Bytes::new(),
                    duration_ms: 0,
                },
                false,
            ));
        }

        let tail = resampler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_mut()
            .map(StreamingStereo48kResampler::finish)
            .unwrap_or_default();
        if !tail.is_empty() {
            all_pcm.extend_from_slice(&tail);
            if let Some(sink) = sink.as_ref() {
                for slice in slice_for_sink(&tail) {
                    let duration_ms =
                        duration_ms_from_mono_s16le(slice.len() / 2, WEBRTC_PCM_SAMPLE_RATE).max(1);
                    if !sink.send(TtsAudioChunk {
                        pcm: slice,
                        duration_ms,
                    }) {
                        break;
                    }
                }
            }
        }
        // The tail send can lose a race with barge-in; that is still a cancelled synthesis.
        if sink.as_ref().is_some_and(TtsProgressiveSink::is_cancelled) {
            otel::record_sherpa_tts_synth_wall_ms(synth_wall_ms, attrs);
            return Ok((
                TtsAudioChunk {
                    pcm: Bytes::new(),
                    duration_ms: 0,
                },
                false,
            ));
        }

        let (pcm, duration_ms) = align_stereo_pcm_to_20ms(Bytes::from(all_pcm));
        otel::record_sherpa_tts_synth_wall_ms(synth_wall_ms, attrs);
        voice_debug(format!(
            "tts synthesis done wall_ms={synth_wall_ms:.0} audio_duration_ms={duration_ms} completed=true"
        ));
        Ok((TtsAudioChunk { pcm, duration_ms }, true))
    }
}

/// One resampler shared by every sentence of a synthesis. Created on the first
/// sentence, once the engine's sample rate is known.
type SharedResampler = Arc<StdMutex<Option<StreamingStereo48kResampler>>>;

struct SentenceOutcome {
    /// Stereo 48 kHz PCM produced for this sentence (already sent to the sink, if any).
    pcm: Vec<Bytes>,
    /// Generation was stopped (cancel or sink gone); the audio is partial.
    stopped: bool,
}

/// Synthesize one sentence on a blocking thread. Takes an engine from the pool, holds
/// its lock for this sentence only.
fn synthesize_sentence(
    engine_pool: &TtsEnginePool,
    sentence: &str,
    speaker_id: i32,
    speed: f32,
    sink: Option<TtsProgressiveSink>,
    resampler: SharedResampler,
) -> SpeechResult<SentenceOutcome> {
    if sink.as_ref().is_some_and(TtsProgressiveSink::is_cancelled) {
        return Ok(SentenceOutcome {
            pcm: Vec::new(),
            stopped: true,
        });
    }

    let shared = engine_pool.acquire();
    let _active = shared.track_session();
    let gen_config = GenerationConfig {
        sid: speaker_id,
        speed,
        ..Default::default()
    };
    let tts = shared
        .tts
        .lock()
        .map_err(|_| SpeechError::Internal("sherpa TTS engine lock poisoned".into()))?;
    let src_rate = tts.sample_rate().max(1) as u32;

    // Set by the progress callback right before it returns `false`; a stopped
    // synthesis holds partial audio and must never reach the phrase cache.
    let stopped = Arc::new(AtomicBool::new(false));
    let produced: Arc<StdMutex<Vec<Bytes>>> = Arc::new(StdMutex::new(Vec::new()));

    let push_pcm = {
        let resampler = Arc::clone(&resampler);
        move |samples: &[f32]| -> Bytes {
            resampler
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get_or_insert_with(|| StreamingStereo48kResampler::new(src_rate))
                .push_f32(samples)
        }
    };

    if let Some(sink) = sink {
        let stopped_cb = Arc::clone(&stopped);
        let produced_cb = Arc::clone(&produced);
        let sink_cb = sink.clone();
        generate(
            &tts,
            sentence,
            &gen_config,
            Some(move |samples: &[f32], _progress: f32| {
                if sink_cb.is_cancelled() {
                    voice_debug("tts synthesis cancelled via progressive callback");
                    stopped_cb.store(true, Ordering::SeqCst);
                    return false;
                }
                // VITS (Piper) invokes the callback once per sentence with that
                // sentence's samples only, not a cumulative buffer. Feed each chunk
                // in full into the continuous resampler.
                if samples.is_empty() {
                    return true;
                }
                let pcm = push_pcm(samples);
                if pcm.is_empty() {
                    return true;
                }
                // One Piper sentence can be many seconds of audio; cap each sink chunk
                // so a single gRPC message stays small.
                for slice in slice_for_sink(&pcm) {
                    let duration_ms =
                        duration_ms_from_mono_s16le(slice.len() / 2, WEBRTC_PCM_SAMPLE_RATE).max(1);
                    if !sink_cb.send(TtsAudioChunk {
                        pcm: slice,
                        duration_ms,
                    }) {
                        stopped_cb.store(true, Ordering::SeqCst);
                        return false;
                    }
                }
                produced_cb
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(pcm);
                true
            }),
        )?;
    } else {
        let audio = generate(&tts, sentence, &gen_config, None::<fn(&[f32], f32) -> bool>)?;
        let pcm = push_pcm(audio.samples());
        if !pcm.is_empty() {
            produced
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(pcm);
        }
    }

    let pcm = std::mem::take(
        &mut *produced
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
    Ok(SentenceOutcome {
        pcm,
        stopped: stopped.load(Ordering::SeqCst),
    })
}

fn generate<F>(
    tts: &sherpa_onnx::OfflineTts,
    sentence: &str,
    gen_config: &GenerationConfig,
    callback: Option<F>,
) -> SpeechResult<sherpa_onnx::GeneratedAudio>
where
    F: FnMut(&[f32], f32) -> bool + 'static,
{
    tts.generate_with_config(sentence, gen_config, callback)
        .ok_or_else(|| SpeechError::Vendor {
            vendor: "local-sherpa".into(),
            message: "OfflineTts generation returned no audio".into(),
        })
}

pub(crate) fn parse_speaker_id(config: &TtsConfig) -> i32 {
    config
        .voice
        .as_deref()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(0)
}

pub(crate) fn parse_speed(config: &TtsConfig) -> f32 {
    config
        .model
        .as_deref()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .or_else(|| {
            std::env::var("SHERPA_TTS_SPEED")
                .ok()
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(1.0)
        .clamp(0.2, 2.0)
}

pub fn tts_generate_count() -> usize {
    TTS_GENERATE_COUNT.load(Ordering::SeqCst)
}

pub fn reset_tts_generate_count() {
    TTS_GENERATE_COUNT.store(0, Ordering::SeqCst);
}

#[async_trait]
impl TtsProvider for SherpaTts {
    fn vendor_name(&self) -> &'static str {
        "local-sherpa"
    }

    fn bind_session_context(&self, ctx: &VoiceSessionContext) {
        let project_id = ctx
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("")
            .to_string();
        if let Ok(mut guard) = self.project_id.lock() {
            *guard = project_id;
        }
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        // Buffered / one-shot callers — no progressive sink.
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let normalized = normalize_phrase_text(text);
        if normalized.is_empty() {
            return Ok(Vec::new());
        }

        let model_dir = self.resolved_model_dir().await?;
        let project_id = self.session_project_id();
        let language = self.language_label();
        let voice = self.voice_label();
        let model_id = self
            .config
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("");
        let cache_key = build_cache_key(&project_id, &model_dir, &language, &voice, &normalized);
        let attrs = build_metric_attrs(&project_id, model_id, &model_dir, &language, &voice);

        if phrase_cache_enabled() {
            if let Some(chunk) = lookup(&cache_key, &attrs) {
                voice_debug(format!(
                    "tts phrase cache hit text_len={} project_id={project_id}",
                    normalized.len()
                ));
                if let Some(sink) = sink {
                    // The cached clip is the whole utterance; deliver it in bounded slices.
                    for slice in slice_for_sink(&chunk.pcm) {
                        let duration_ms =
                            duration_ms_from_mono_s16le(slice.len() / 2, WEBRTC_PCM_SAMPLE_RATE)
                                .max(1);
                        if !sink.send(TtsAudioChunk {
                            pcm: slice,
                            duration_ms,
                        }) {
                            break;
                        }
                    }
                }
                return Ok(vec![chunk]);
            }
        }

        otel::record_sherpa_tts_phrase_cache_miss(&attrs);
        let (chunk, completed) = self.synthesize_miss(&normalized, &attrs, sink).await?;
        if !completed {
            // Cancelled mid-way: partial audio must not be cached or replayed.
            return Ok(Vec::new());
        }
        if phrase_cache_enabled() {
            store(cache_key, chunk.clone(), &attrs);
        }
        Ok(vec![chunk])
    }
}
