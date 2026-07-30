//! Progressive TTS (`VOICE_TTS_STREAM_CHUNKS`) vs buffered path.
//!
//! Asserts streaming first-audio latency is better than (or equal to) the
//! legacy fully-buffered path, including under parallel multi-agent load.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttConfig, TtsConfig, TtsVendor, VoiceAgentConfig,
};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsProgressiveSink, TtsProvider, VendorFactory,
};
use node_webrtc_rust_speech::{
    tts_stream_chunks_enabled, PcmWriter, SttProvider, VendorRegistry, VoiceAgent,
};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::time::timeout;

/// Serialize env mutations — tokio runs integration tests in parallel by default.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var(key).ok();
        unsafe { std::env::set_var(key, value) };
        Self {
            key,
            previous,
            _lock: lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// Mock TTS that emits `parts` chunks with `part_delay` between them when progressive.
struct ChunkedMockTts {
    parts: usize,
    part_delay: Duration,
    part_duration_ms: u32,
}

fn sine_chunk(duration_ms: u32, frequency_hz: f32) -> TtsAudioChunk {
    let sample_rate = 48_000_u32;
    let samples = (sample_rate * duration_ms / 1000) as usize;
    let mut pcm = Vec::with_capacity(samples * 4);
    for i in 0..samples {
        let t = i as f32 / sample_rate as f32;
        let sample = (t * frequency_hz * 2.0 * std::f32::consts::PI).sin() * 0.2;
        let i16_sample = (sample * i16::MAX as f32) as i16;
        pcm.extend_from_slice(&i16_sample.to_le_bytes());
        pcm.extend_from_slice(&i16_sample.to_le_bytes());
    }
    TtsAudioChunk {
        pcm: Bytes::from(pcm),
        duration_ms,
    }
}

#[async_trait]
impl TtsProvider for ChunkedMockTts {
    fn vendor_name(&self) -> &'static str {
        "mock-chunked"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        _text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let mut all = Vec::with_capacity(self.parts);
        for i in 0..self.parts {
            if i > 0 {
                tokio::time::sleep(self.part_delay).await;
            }
            if sink.as_ref().is_some_and(|s| s.is_cancelled()) {
                break;
            }
            let chunk = sine_chunk(self.part_duration_ms, 440.0 + i as f32 * 20.0);
            if let Some(ref sink) = sink {
                let _ = sink.send(chunk.clone());
            }
            all.push(chunk);
        }
        Ok(all)
    }
}

struct ChunkedMockFactory {
    parts: usize,
    part_delay: Duration,
    part_duration_ms: u32,
}

impl VendorFactory for ChunkedMockFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(ChunkedMockTts {
            parts: self.parts,
            part_delay: self.part_delay,
            part_duration_ms: self.part_duration_ms,
        }))
    }
}

fn agent_config() -> VoiceAgentConfig {
    VoiceAgentConfig {
        stt: None,
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
        }),
        ..Default::default()
    }
}

fn chunked_factory() -> Arc<ChunkedMockFactory> {
    Arc::new(ChunkedMockFactory {
        parts: 4,
        part_delay: Duration::from_millis(80),
        part_duration_ms: 40,
    })
}

/// Milliseconds from `send_text_to_tts` to the first outbound PCM write.
async fn measure_first_audio_ms(factory: Arc<ChunkedMockFactory>, text: &str) -> u128 {
    let mut registry = VendorRegistry::new();
    registry.register_tts(TtsVendor::Mock, factory);
    let agent = VoiceAgent::new(agent_config(), Arc::new(registry)).unwrap();

    let first_ms: Arc<Mutex<Option<u128>>> = Arc::new(Mutex::new(None));
    let first_ms_w = Arc::clone(&first_ms);
    let send_start: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let send_start_w = Arc::clone(&send_start);

    let writer: PcmWriter = Arc::new(move |_pcm, _ms| {
        if let Some(t0) = *send_start_w.lock().unwrap() {
            let mut slot = first_ms_w.lock().unwrap();
            if slot.is_none() {
                // micros → round up to ms so sub-millisecond first audio is not 0.
                *slot = Some(t0.elapsed().as_micros().div_ceil(1000));
            }
        }
        Ok(())
    });

    agent
        .attach(Arc::new(|| Ok(None)), writer)
        .await
        .unwrap();
    agent.start(None).await.unwrap();

    *send_start.lock().unwrap() = Some(Instant::now());
    agent
        .send_text_to_tts_with_options(text, SendTextToTtsOptions { non_blocking: true })
        .await
        .unwrap();

    timeout(Duration::from_secs(5), async {
        loop {
            if first_ms.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("timed out waiting for first PCM");

    agent.wait_tts_playback_idle().await.unwrap();
    agent.stop().await.unwrap();
    let ms = first_ms.lock().unwrap().expect("first audio timestamp");
    ms
}

#[tokio::test]
async fn stream_chunks_env_and_latency_paths() {
    // Single test body so EnvGuard holds the lock across sequential path compares.
    {
        let _on = EnvGuard::set("VOICE_TTS_STREAM_CHUNKS", "1");
        assert!(tts_stream_chunks_enabled());
    }
    {
        let _off = EnvGuard::set("VOICE_TTS_STREAM_CHUNKS", "0");
        assert!(!tts_stream_chunks_enabled());
    }
    {
        let _false = EnvGuard::set("VOICE_TTS_STREAM_CHUNKS", "false");
        assert!(!tts_stream_chunks_enabled());
    }

    let factory = chunked_factory();

    let buffered_ms = {
        let _buf = EnvGuard::set("VOICE_TTS_STREAM_CHUNKS", "0");
        assert!(!tts_stream_chunks_enabled());
        measure_first_audio_ms(factory.clone(), "hello streaming world").await
    };

    let streaming_ms = {
        let _stream = EnvGuard::set("VOICE_TTS_STREAM_CHUNKS", "1");
        assert!(tts_stream_chunks_enabled());
        measure_first_audio_ms(factory.clone(), "hello streaming world").await
    };

    // Streaming emits part 0 immediately; buffered waits for all 4 parts (~240ms+).
    assert!(
        streaming_ms + 40 <= buffered_ms,
        "streaming first-audio ({streaming_ms}ms) should beat buffered ({buffered_ms}ms)"
    );

    let serial_ms = {
        let _stream = EnvGuard::set("VOICE_TTS_STREAM_CHUNKS", "1");
        measure_first_audio_ms(factory.clone(), "parallel one").await
    };

    let (a_ms, b_ms) = {
        let _stream = EnvGuard::set("VOICE_TTS_STREAM_CHUNKS", "1");
        tokio::join!(
            measure_first_audio_ms(factory.clone(), "parallel a"),
            measure_first_audio_ms(factory.clone(), "parallel b"),
        )
    };

    let parallel_worst = a_ms.max(b_ms);
    assert!(
        parallel_worst <= serial_ms.saturating_mul(2).saturating_add(80),
        "parallel first-audio worst={parallel_worst}ms serial={serial_ms}ms"
    );

    {
        let _buf = EnvGuard::set("VOICE_TTS_STREAM_CHUNKS", "0");
        let mut registry = VendorRegistry::new();
        registry.register_tts(TtsVendor::Mock, factory.clone());
        let agent = VoiceAgent::new(agent_config(), Arc::new(registry)).unwrap();
        let frames = Arc::new(AtomicUsize::new(0));
        let frames_w = Arc::clone(&frames);
        let writer: PcmWriter = Arc::new(move |_pcm, _ms| {
            frames_w.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        agent
            .attach(Arc::new(|| Ok(None)), writer)
            .await
            .unwrap();
        agent.start(None).await.unwrap();
        agent.send_text_to_tts("full").await.unwrap();
        agent.wait_tts_playback_idle().await.unwrap();
        agent.stop().await.unwrap();
        assert!(
            frames.load(Ordering::SeqCst) >= 8,
            "expected full buffered playback, got {} frames",
            frames.load(Ordering::SeqCst)
        );
    }

    {
        let _stream = EnvGuard::set("VOICE_TTS_STREAM_CHUNKS", "1");
        let mut registry = VendorRegistry::new();
        registry.register_tts(TtsVendor::Mock, factory);
        let agent = VoiceAgent::new(agent_config(), Arc::new(registry)).unwrap();
        let frames = Arc::new(AtomicUsize::new(0));
        let frames_w = Arc::clone(&frames);
        let writer: PcmWriter = Arc::new(move |_pcm, _ms| {
            frames_w.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        agent
            .attach(Arc::new(|| Ok(None)), writer)
            .await
            .unwrap();
        agent.start(None).await.unwrap();
        agent.send_text_to_tts("full stream").await.unwrap();
        agent.wait_tts_playback_idle().await.unwrap();
        agent.stop().await.unwrap();
        assert!(
            frames.load(Ordering::SeqCst) >= 8,
            "expected full streaming playback, got {} frames",
            frames.load(Ordering::SeqCst)
        );
    }
}
