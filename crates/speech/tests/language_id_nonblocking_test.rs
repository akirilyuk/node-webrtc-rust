//! Language ID must not block inbound PCM, STT, or TTS.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    LanguageIdConfig, SendTextToTtsOptions, SttConfig, SttVendor, TtsConfig, TtsVendor,
    VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{
    LanguageIdProvider, LanguageIdResult, SttProvider, SttTranscript, TtsAudioChunk, TtsProvider,
    VendorFactory,
};
use node_webrtc_rust_speech::{VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

const LID_SLEEP_MS: u64 = 1500;

fn loud_stereo_frame() -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
    }
    pcm
}

fn silent_stereo_frame() -> Vec<u8> {
    vec![0_u8; 3840]
}

struct SlowLanguageId {
    sleep_ms: u64,
}

#[async_trait::async_trait]
impl LanguageIdProvider for SlowLanguageId {
    async fn identify(
        &self,
        _pcm: Bytes,
        _sample_rate: u32,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<LanguageIdResult>> {
        tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        Ok(Some(LanguageIdResult {
            language: "en".into(),
        }))
    }
}

struct CountingStt {
    bytes: Arc<Mutex<usize>>,
}

#[async_trait::async_trait]
impl SttProvider for CountingStt {
    fn vendor_name(&self) -> &'static str {
        "counting"
    }

    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        *self.bytes.lock().unwrap() += pcm.len();
        Ok(())
    }

    async fn poll_transcript(
        &mut self,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<SttTranscript>> {
        Ok(None)
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
}

struct LidTestFactory {
    stt_bytes: Arc<Mutex<usize>>,
    lid_sleep_ms: u64,
}

impl VendorFactory for LidTestFactory {
    fn create_stt(
        &self,
        config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        if config.provider == SttVendor::Mock {
            Ok(Box::new(CountingStt {
                bytes: Arc::clone(&self.stt_bytes),
            }))
        } else {
            MockFactory.create_stt(config)
        }
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn TtsProvider>> {
        MockFactory.create_tts(config)
    }

    fn create_language_id(
        &self,
        _config: &LanguageIdConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<Box<dyn LanguageIdProvider>>> {
        Ok(Some(Box::new(SlowLanguageId {
            sleep_ms: self.lid_sleep_ms,
        })))
    }
}

fn agent_with_slow_lid(stt_bytes: Arc<Mutex<usize>>) -> Arc<VoiceAgent> {
    let factory = Arc::new(LidTestFactory {
        stt_bytes: Arc::clone(&stt_bytes),
        lid_sleep_ms: LID_SLEEP_MS,
    });
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::clone(&factory) as Arc<dyn VendorFactory>);
    registry.register_stt(SttVendor::LocalSherpa, factory);
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = true;

    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
        }),
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
        }),
        language_id: Some(LanguageIdConfig {
            enabled: Some(true),
            model_path: Some("/fake/lid-model".into()),
            allowlist: None,
            min_speech_ms: Some(200),
        }),
        vad,
        ..Default::default()
    };

    VoiceAgent::new(config, Arc::new(registry)).unwrap()
}

#[tokio::test]
async fn language_id_does_not_block_inbound_pcm_or_stt() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let agent = agent_with_slow_lid(Arc::clone(&stt_bytes));
    let mut rx = agent.subscribe_events();

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();

    // VAD speech start (min_speech_duration_ms=40 → 3 frames).
    for _ in 0..3 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    let bytes_before_lid = *stt_bytes.lock().unwrap();
    assert!(bytes_before_lid > 0, "STT should receive speech before LID threshold");

    // Frames 4–13: frame ~10 crosses min_speech_ms=200 and spawns slow LID.
    let batch_start = Instant::now();
    let mut per_frame_max_ms = 0_u128;
    for _ in 0..10 {
        let frame_start = Instant::now();
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
        per_frame_max_ms = per_frame_max_ms.max(frame_start.elapsed().as_millis());
    }
    let batch_elapsed = batch_start.elapsed();

    assert!(
        per_frame_max_ms < 200,
        "each process_inbound_pcm must return well under LID sleep (max {per_frame_max_ms} ms)"
    );
    assert!(
        batch_elapsed < Duration::from_millis(400),
        "10 frames after LID threshold must not await identify (batch {batch_elapsed:?})"
    );

    // STT must keep receiving audio while LID sleeps in the background.
    let bytes_mid_lid = *stt_bytes.lock().unwrap();
    assert!(
        bytes_mid_lid > bytes_before_lid,
        "STT push_audio must continue during LID sleep window"
    );

    // Eventually emit user_language when background identify completes.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_user_language = false;
    while Instant::now() < deadline {
        while let Ok(event) = rx.try_recv() {
            if event.kind == SpeechEventKind::UserLanguage {
                assert_eq!(event.language.as_deref(), Some("en"));
                saw_user_language = true;
                break;
            }
        }
        if saw_user_language {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        saw_user_language,
        "expected user_language after background identify"
    );
}

#[tokio::test]
async fn user_speaking_end_not_delayed_by_slow_language_id() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let factory = Arc::new(LidTestFactory {
        stt_bytes: Arc::clone(&stt_bytes),
        lid_sleep_ms: LID_SLEEP_MS,
    });
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::LocalSherpa, factory);
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = false;

    let config = VoiceAgentConfig {
        stt: None,
        tts: None,
        language_id: Some(LanguageIdConfig {
            enabled: Some(true),
            model_path: Some("/fake/lid-model".into()),
            allowlist: None,
            min_speech_ms: Some(200),
        }),
        vad,
        ..Default::default()
    };

    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let mut rx = agent.subscribe_events();

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    for _ in 0..12 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    let end_start = Instant::now();
    let mut saw_speaking_end = false;
    for _ in 0..5 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
        while let Ok(event) = rx.try_recv() {
            if event.kind == SpeechEventKind::UserSpeakingEnd {
                saw_speaking_end = true;
                break;
            }
        }
        if saw_speaking_end {
            break;
        }
    }

    assert!(saw_speaking_end, "expected user_speaking_end after silence");
    assert!(
        end_start.elapsed() < Duration::from_millis(500),
        "user_speaking_end must not await slow LID (took {end_start:?})"
    );
}

/// Staging regression (`usage-credits-smoke`): echo TTS clip was delayed ~45s while Whisper LID
/// ran on the blocking pool. Full stack STT+TTS+LID+VAD — first outbound PCM must not await
/// slow `identify` while LID is in flight.
#[tokio::test]
async fn language_id_does_not_block_tts_playback() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let agent = agent_with_slow_lid(Arc::clone(&stt_bytes));
    let mut rx = agent.subscribe_events();

    let first_ms: Arc<Mutex<Option<u128>>> = Arc::new(Mutex::new(None));
    let first_ms_w = Arc::clone(&first_ms);
    let send_start: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let send_start_w = Arc::clone(&send_start);
    let written_bytes: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let written_bytes_w = Arc::clone(&written_bytes);

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(move |pcm, _ms| {
        *written_bytes_w.lock().unwrap() += pcm.len();
        if let Some(t0) = *send_start_w.lock().unwrap() {
            let mut slot = first_ms_w.lock().unwrap();
            if slot.is_none() {
                *slot = Some(t0.elapsed().as_millis());
            }
        }
        Ok(())
    });

    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();

    // VAD speech start + cross min_speech_ms=200 so slow LID (LID_SLEEP_MS) is in flight.
    for _ in 0..12 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    *send_start.lock().unwrap() = Some(Instant::now());
    agent
        .send_text_to_tts_with_options(
            "one two three",
            SendTextToTtsOptions { non_blocking: true },
        )
        .await
        .unwrap();

    let poll_deadline = Instant::now() + Duration::from_millis(800);
    while Instant::now() < poll_deadline {
        if first_ms.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let ms = first_ms
        .lock()
        .unwrap()
        .expect("expected first outbound PCM within 800ms");
    assert!(
        ms < 500,
        "first TTS PCM must not await LID sleep ({LID_SLEEP_MS}ms); got {ms}ms"
    );

    let nbytes = *written_bytes.lock().unwrap();
    assert!(nbytes > 0, "writer must receive non-empty PCM");

    // LID still completes in background.
    let lang_deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_user_language = false;
    while Instant::now() < lang_deadline {
        while let Ok(event) = rx.try_recv() {
            if event.kind == SpeechEventKind::UserLanguage {
                assert_eq!(event.language.as_deref(), Some("en"));
                saw_user_language = true;
                break;
            }
        }
        if saw_user_language {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        saw_user_language,
        "expected user_language after background identify"
    );

    agent.wait_tts_playback_idle().await.unwrap();
    agent.stop().await.unwrap();
}

struct OverlapGuard {
    lid_in_flight: AtomicUsize,
    tts_in_flight: AtomicUsize,
    violated: AtomicBool,
}

impl OverlapGuard {
    fn enter_lid(&self) {
        self.lid_in_flight.fetch_add(1, Ordering::SeqCst);
        if self.tts_in_flight.load(Ordering::SeqCst) > 0 {
            self.violated.store(true, Ordering::SeqCst);
        }
    }

    fn leave_lid(&self) {
        self.lid_in_flight.fetch_sub(1, Ordering::SeqCst);
    }

    fn enter_tts(&self) {
        self.tts_in_flight.fetch_add(1, Ordering::SeqCst);
        if self.lid_in_flight.load(Ordering::SeqCst) > 0 {
            self.violated.store(true, Ordering::SeqCst);
        }
    }

    fn leave_tts(&self) {
        self.tts_in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

struct OverlapTrackingLanguageId {
    guard: Arc<OverlapGuard>,
    sleep_ms: u64,
}

#[async_trait::async_trait]
impl LanguageIdProvider for OverlapTrackingLanguageId {
    async fn identify(
        &self,
        _pcm: Bytes,
        _sample_rate: u32,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<LanguageIdResult>> {
        self.guard.enter_lid();
        tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        self.guard.leave_lid();
        Ok(Some(LanguageIdResult {
            language: "en".into(),
        }))
    }
}

struct OverlapTrackingTts {
    guard: Arc<OverlapGuard>,
}

#[async_trait::async_trait]
impl TtsProvider for OverlapTrackingTts {
    fn vendor_name(&self) -> &'static str {
        "overlap-mock"
    }

    async fn synthesize(&self, text: &str) -> node_webrtc_rust_speech::SpeechResult<Vec<TtsAudioChunk>> {
        self.guard.enter_tts();
        let duration_ms = (text.len() as u32 * 50).clamp(100, 5000);
        tokio::time::sleep(Duration::from_millis(80)).await;
        let samples = 48_000 * duration_ms / 1000;
        let pcm = Bytes::from(vec![0_u8; samples as usize * 4]);
        self.guard.leave_tts();
        Ok(vec![TtsAudioChunk {
            pcm,
            duration_ms,
        }])
    }
}

struct OverlapLidTestFactory {
    stt_bytes: Arc<Mutex<usize>>,
    guard: Arc<OverlapGuard>,
    lid_sleep_ms: u64,
}

impl VendorFactory for OverlapLidTestFactory {
    fn create_stt(
        &self,
        config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        if config.provider == SttVendor::Mock {
            Ok(Box::new(CountingStt {
                bytes: Arc::clone(&self.stt_bytes),
            }))
        } else {
            MockFactory.create_stt(config)
        }
    }

    fn create_tts(
        &self,
        _config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(OverlapTrackingTts {
            guard: Arc::clone(&self.guard),
        }))
    }

    fn create_language_id(
        &self,
        _config: &LanguageIdConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<Box<dyn LanguageIdProvider>>> {
        Ok(Some(Box::new(OverlapTrackingLanguageId {
            guard: Arc::clone(&self.guard),
            sleep_ms: self.lid_sleep_ms,
        })))
    }
}

fn agent_with_overlap_tracking_lid(
    stt_bytes: Arc<Mutex<usize>>,
    guard: Arc<OverlapGuard>,
) -> Arc<VoiceAgent> {
    let factory = Arc::new(OverlapLidTestFactory {
        stt_bytes: Arc::clone(&stt_bytes),
        guard,
        lid_sleep_ms: LID_SLEEP_MS,
    });
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::clone(&factory) as Arc<dyn VendorFactory>);
    registry.register_stt(
        SttVendor::LocalSherpa,
        Arc::clone(&factory) as Arc<dyn VendorFactory>,
    );
    registry.register_tts(TtsVendor::Mock, factory as Arc<dyn VendorFactory>);

    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = true;

    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
        }),
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
        }),
        language_id: Some(LanguageIdConfig {
            enabled: Some(true),
            model_path: Some("/fake/lid-model".into()),
            allowlist: None,
            min_speech_ms: Some(200),
        }),
        vad,
        ..Default::default()
    };

    VoiceAgent::new(config, Arc::new(registry)).unwrap()
}

#[tokio::test]
async fn language_id_does_not_overlap_tts_synthesis() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let guard = Arc::new(OverlapGuard {
        lid_in_flight: AtomicUsize::new(0),
        tts_in_flight: AtomicUsize::new(0),
        violated: AtomicBool::new(false),
    });
    let agent = agent_with_overlap_tracking_lid(Arc::clone(&stt_bytes), Arc::clone(&guard));
    let mut rx = agent.subscribe_events();

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    // VAD speech start (min_speech_duration_ms=40 → 3 frames) without crossing LID threshold.
    for _ in 0..3 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    agent
        .send_text_to_tts_with_options(
            "one two three",
            SendTextToTtsOptions { non_blocking: true },
        )
        .await
        .unwrap();

    // Cross min_speech_ms while TTS synthesis/playback is active — LID must defer.
    for _ in 0..10 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    // user_speaking_end + force LID while TTS may still be active — must defer, not overlap.
    for _ in 0..5 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    agent.wait_tts_playback_idle().await.unwrap();

    assert!(
        !guard.violated.load(Ordering::SeqCst),
        "LID identify must not overlap TTS synthesize"
    );

    let lang_deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_user_language = false;
    while Instant::now() < lang_deadline {
        while let Ok(event) = rx.try_recv() {
            if event.kind == SpeechEventKind::UserLanguage {
                assert_eq!(event.language.as_deref(), Some("en"));
                saw_user_language = true;
                break;
            }
        }
        if saw_user_language {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        saw_user_language,
        "expected user_language after deferred identify"
    );

    agent.stop().await.unwrap();
}
