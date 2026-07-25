//! TTS injection tests through VoiceAgent with mock vendor.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttConfig, SttVendor, TtsConfig, TtsVendor, VoiceAgentConfig,
};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsAudioChunk, TtsProvider, VendorFactory};
use node_webrtc_rust_speech::{
    PcmWriter, SpeechEvent, SpeechEventKind, VendorRegistry, VoiceAgent,
};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::time::{timeout, Duration as TokioDuration};

#[tokio::test]
async fn tts_injection_writes_pcm_to_outbound() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let config = VoiceAgentConfig {
        stt: None,
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();

    let written: Arc<Mutex<Vec<(Bytes, u32)>>> = Arc::new(Mutex::new(Vec::new()));
    let written_clone = Arc::clone(&written);
    let writer: PcmWriter = Arc::new(move |pcm, ms| {
        written_clone.lock().unwrap().push((pcm, ms));
        Ok(())
    });
    let reader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent.send_text_to_tts("hello agent").await.unwrap();
    agent.wait_tts_playback_idle().await.unwrap();
    agent.stop().await.unwrap();

    let chunks = written.lock().unwrap();
    assert!(!chunks.is_empty());
    assert!(chunks[0].0.len() > 0);
}

#[tokio::test]
async fn tts_injection_emits_agent_speaking_events() {
    let mut registry = VendorRegistry::new();
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let config = VoiceAgentConfig {
        stt: None,
        tts: Some(node_webrtc_rust_speech::TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
        }),
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let mut rx = agent.subscribe_events();

    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let reader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent.send_text_to_tts("hi").await.unwrap();
    agent.wait_tts_playback_idle().await.unwrap();

    let event = timeout(TokioDuration::from_secs(1), rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    assert_eq!(event.kind, SpeechEventKind::AgentSpeakingStart);
}

struct SlowMockTts {
    delay: Duration,
    inner: Box<dyn TtsProvider>,
}

#[async_trait]
impl TtsProvider for SlowMockTts {
    fn vendor_name(&self) -> &'static str {
        "mock-slow"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        tokio::time::sleep(self.delay).await;
        self.inner.synthesize(text).await
    }
}

struct SlowMockFactory {
    delay: Duration,
}

impl VendorFactory for SlowMockFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(SlowMockTts {
            delay: self.delay,
            inner: MockFactory.create_tts(config)?,
        }))
    }
}

fn slow_tts_config() -> VoiceAgentConfig {
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

#[tokio::test]
async fn non_blocking_send_returns_before_slow_synthesis_finishes() {
    let mut registry = VendorRegistry::new();
    registry.register_tts(
        TtsVendor::Mock,
        Arc::new(SlowMockFactory {
            delay: Duration::from_millis(400),
        }),
    );

    let agent = VoiceAgent::new(slow_tts_config(), Arc::new(registry)).unwrap();
    let written: Arc<Mutex<Vec<(Bytes, u32)>>> = Arc::new(Mutex::new(Vec::new()));
    let written_clone = Arc::clone(&written);
    let writer: PcmWriter = Arc::new(move |pcm, ms| {
        written_clone.lock().unwrap().push((pcm, ms));
        Ok(())
    });
    let reader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start(None).await.unwrap();

    let started = Instant::now();
    agent
        .send_text_to_tts_with_options("hello", SendTextToTtsOptions { non_blocking: true })
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(150),
        "non_blocking should return quickly, took {:?}",
        started.elapsed()
    );
    assert!(
        written.lock().unwrap().is_empty(),
        "PCM should not be written before synthesis completes"
    );

    agent.wait_tts_playback_idle().await.unwrap();
    assert!(!written.lock().unwrap().is_empty());
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn non_blocking_two_jobs_still_play_in_order() {
    let mut registry = VendorRegistry::new();
    registry.register_tts(
        TtsVendor::Mock,
        Arc::new(SlowMockFactory {
            delay: Duration::from_millis(80),
        }),
    );

    let agent = VoiceAgent::new(slow_tts_config(), Arc::new(registry)).unwrap();
    let written: Arc<Mutex<Vec<(Bytes, u32)>>> = Arc::new(Mutex::new(Vec::new()));
    let written_clone = Arc::clone(&written);
    let writer: PcmWriter = Arc::new(move |pcm, ms| {
        written_clone.lock().unwrap().push((pcm, ms));
        Ok(())
    });
    let reader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start(None).await.unwrap();

    let opts = SendTextToTtsOptions { non_blocking: true };
    agent
        .send_text_to_tts_with_options("first phrase here", opts)
        .await
        .unwrap();
    agent
        .send_text_to_tts_with_options("second phrase here", opts)
        .await
        .unwrap();

    agent.wait_tts_playback_idle().await.unwrap();
    let chunks = written.lock().unwrap();
    let total_bytes: usize = chunks.iter().map(|(pcm, _)| pcm.len()).sum();
    assert!(
        total_bytes > 100_000,
        "expected PCM from two TTS jobs, got {total_bytes} bytes in {} writes",
        chunks.len()
    );
    agent.stop().await.unwrap();
}

fn mock_tts_only_config() -> VoiceAgentConfig {
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

async fn drain_speech_events(
    rx: &mut tokio::sync::broadcast::Receiver<SpeechEvent>,
) -> Vec<SpeechEvent> {
    let mut events = Vec::new();
    loop {
        match timeout(TokioDuration::from_millis(100), rx.recv()).await {
            Ok(Ok(event)) => events.push(event),
            _ => break,
        }
    }
    events
}

#[tokio::test]
async fn tts_writer_failure_reaches_blocking_caller_and_recovers() {
    let mut registry = VendorRegistry::new();
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let agent = VoiceAgent::new(mock_tts_only_config(), Arc::new(registry)).unwrap();
    let mut rx = agent.subscribe_events();

    let fail_first = Arc::new(AtomicBool::new(true));
    let write_count = Arc::new(AtomicUsize::new(0));
    let fail_first_w = Arc::clone(&fail_first);
    let write_count_w = Arc::clone(&write_count);
    let writer: PcmWriter = Arc::new(move |_pcm, _ms| {
        if fail_first_w.swap(false, Ordering::SeqCst) {
            return Err(SpeechError::Internal("simulated pcm writer failure".into()));
        }
        write_count_w.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let reader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start(None).await.unwrap();

    let err = agent
        .send_text_to_tts("ready")
        .await
        .expect_err("writer failure must reach blocking caller");
    let err_text = err.to_string();
    assert!(
        err_text.contains("simulated pcm writer failure"),
        "expected original writer error, got {err_text}"
    );

    // Drain abort must leave the agent idle for a later job.
    timeout(TokioDuration::from_secs(2), agent.wait_tts_playback_idle())
        .await
        .expect("idle wait timed out")
        .expect("agent should be idle after playback abort");
    assert!(
        !agent.tts_buffer().is_speaking().await,
        "TTS buffer must be flushed after writer failure"
    );

    let events = drain_speech_events(&mut rx).await;
    let starts = events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::AgentSpeakingStart)
        .count();
    let ends = events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::AgentSpeakingEnd)
        .count();
    let errors = events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::Error)
        .count();
    assert_eq!(
        starts, 1,
        "expected one agent_speaking_start, events={events:?}"
    );
    assert_eq!(
        ends, 1,
        "expected balanced agent_speaking_end, events={events:?}"
    );
    assert_eq!(
        errors, 1,
        "expected one SpeechEvent::error, events={events:?}"
    );
    let error_text = events
        .iter()
        .find(|e| e.kind == SpeechEventKind::Error)
        .and_then(|e| e.error.as_deref())
        .unwrap_or("");
    assert!(
        error_text.contains("simulated pcm writer failure"),
        "error event should preserve original text, got {error_text:?}"
    );

    // Subsequent TTS job must synthesize and play after the failed writer/drain.
    agent.send_text_to_tts("hello again").await.unwrap();
    agent.wait_tts_playback_idle().await.unwrap();
    assert!(
        write_count.load(Ordering::SeqCst) > 0,
        "second TTS job should write PCM after recovery"
    );

    let recovery_events = drain_speech_events(&mut rx).await;
    let recovery_starts = recovery_events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::AgentSpeakingStart)
        .count();
    let recovery_ends = recovery_events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::AgentSpeakingEnd)
        .count();
    assert_eq!(
        recovery_starts, recovery_ends,
        "recovery playback events must stay balanced, events={recovery_events:?}"
    );
    assert!(
        recovery_starts >= 1,
        "expected speaking start/end for recovered job, events={recovery_events:?}"
    );

    agent.stop().await.unwrap();
}

#[tokio::test]
async fn successful_tts_playback_still_emits_balanced_speaking_events() {
    let mut registry = VendorRegistry::new();
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let agent = VoiceAgent::new(mock_tts_only_config(), Arc::new(registry)).unwrap();
    let mut rx = agent.subscribe_events();

    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let reader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent.send_text_to_tts("hi").await.unwrap();
    agent.wait_tts_playback_idle().await.unwrap();

    let events = drain_speech_events(&mut rx).await;
    let starts = events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::AgentSpeakingStart)
        .count();
    let ends = events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::AgentSpeakingEnd)
        .count();
    let errors = events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::Error)
        .count();
    assert_eq!(starts, 1, "events={events:?}");
    assert_eq!(ends, 1, "events={events:?}");
    assert_eq!(
        errors, 0,
        "success path must not emit error, events={events:?}"
    );
    agent.stop().await.unwrap();
}
