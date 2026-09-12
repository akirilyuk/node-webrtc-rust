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

struct CountingLanguageId {
    calls: Arc<AtomicUsize>,
    sleep_ms: u64,
    languages: Vec<String>,
}

struct FailingLanguageId;

#[async_trait::async_trait]
impl LanguageIdProvider for FailingLanguageId {
    async fn identify(
        &self,
        _pcm: Bytes,
        _sample_rate: u32,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<LanguageIdResult>> {
        Err(node_webrtc_rust_speech::SpeechError::Internal(
            "mock LID failure".into(),
        ))
    }
}

struct TimedOverlapLanguageId {
    guard: Arc<OverlapGuard>,
    sleep_ms: u64,
    started_at: Arc<Mutex<Option<Instant>>>,
    ended_at: Arc<Mutex<Option<Instant>>>,
}

#[async_trait::async_trait]
impl LanguageIdProvider for TimedOverlapLanguageId {
    async fn identify(
        &self,
        _pcm: Bytes,
        _sample_rate: u32,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<LanguageIdResult>> {
        *self.started_at.lock().unwrap() = Some(Instant::now());
        self.guard.enter_lid();
        tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        self.guard.leave_lid();
        *self.ended_at.lock().unwrap() = Some(Instant::now());
        Ok(Some(LanguageIdResult {
            language: "en".into(),
        }))
    }
}

#[async_trait::async_trait]
impl LanguageIdProvider for CountingLanguageId {
    async fn identify(
        &self,
        _pcm: Bytes,
        _sample_rate: u32,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<LanguageIdResult>> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        let language = self
            .languages
            .get(index)
            .or_else(|| self.languages.last())
            .cloned()
            .unwrap_or_else(|| "en".into());
        Ok(Some(LanguageIdResult { language }))
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
    vad.stt_gate_hold_ms = 80;

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
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            tts_exclusion: Some(true),
        }),
        vad,
        ..Default::default()
    };

    VoiceAgent::new(config, Arc::new(registry)).unwrap()
}

#[tokio::test]
async fn language_id_does_not_block_inbound_pcm_or_stt() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
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
    // gate_stt off: mock STT never emits finals; gate-hold finalize would skip speaking_end.
    // C1 (`stt_listen_timeout_ms`) closes the stream and pairs user_speaking_end with LID.
    vad.gate_stt = false;
    vad.stt_listen_timeout_ms = 400;

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
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            tts_exclusion: Some(true),
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

    // VAD speech start (min_speech_duration_ms=40 → 3 frames).
    for _ in 0..3 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    let bytes_before_lid = *stt_bytes.lock().unwrap();
    assert!(bytes_before_lid > 0, "STT should receive speech before LID threshold");

    // Frames 4–13: default mode buffers only (no mid-utterance identify).
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
        "10 frames after min_speech_ms buffer must not await identify (batch {batch_elapsed:?})"
    );

    // STT must keep receiving audio; identify waits for user_speaking_end.
    let bytes_mid = *stt_bytes.lock().unwrap();
    assert!(
        bytes_mid > bytes_before_lid,
        "STT push_audio must continue while LID is only buffered"
    );

    let silent = silent_stereo_frame();
    for _ in 0..20 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    let deadline = Instant::now() + Duration::from_secs(4);
    let mut saw_user_language = false;
    while Instant::now() < deadline {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
        if wait_for_user_language(&agent, &mut rx).await {
            saw_user_language = true;
            break;
        }
    }
    assert!(
        saw_user_language,
        "expected user_language after background identify"
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
    let silent = silent_stereo_frame();

    // VAD speech start + buffer past min_speech_ms (default mode does not identify yet).
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

    for _ in 0..5 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    agent.wait_tts_playback_idle().await.unwrap();

    assert!(
        wait_for_user_language_with_pcm(&agent, &mut rx, &silent).await,
        "expected user_language after background identify"
    );

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
            Ok(Box::new(FinalOnceStt {
                bytes: Arc::clone(&self.stt_bytes),
                finalized: Arc::new(Mutex::new(false)),
                emitted: AtomicBool::new(false),
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
    vad.stt_gate_hold_ms = 80;
    vad.stt_listen_timeout_ms = 60_000;

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
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            tts_exclusion: Some(true),
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

    // Cross min_speech_ms while TTS is active — default mode buffers only; identify at hang-up.
    for _ in 0..10 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    // user_speaking_end + hang-up defer while TTS may still be active — must defer, not overlap.
    for _ in 0..12 {
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

    assert!(
        wait_for_user_language_with_pcm(&agent, &mut rx, &silent).await,
        "expected user_language after deferred identify"
    );

    agent.stop().await.unwrap();
}

/// Staging echo-smoke order: speech crosses minSpeechMs, speaking_end, then immediate `speak("echo. …")`.
/// LID starts at speech end; with exclusion on, deferred identify must not overlap Piper synthesis.
#[tokio::test]
async fn language_id_leftover_min_speech_must_not_overlap_later_tts() {
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

    // Buffer past min_speech_ms=200 without mid-utterance identify.
    for _ in 0..12 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }

    // Silence → speaking_end + hang-up defer (staging: final handler then immediate speak).
    for _ in 0..5 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    agent
        .send_text_to_tts_with_options(
            "echo. One, two, three, four, five, six, seven, eight, nine, ten",
            SendTextToTtsOptions { non_blocking: true },
        )
        .await
        .unwrap();

    agent.wait_tts_playback_idle().await.unwrap();

    assert!(
        !guard.violated.load(Ordering::SeqCst),
        "LID at speech end with exclusion on must not overlap TTS when speak() follows user_speech_final"
    );

    assert!(
        wait_for_user_language_with_pcm(&agent, &mut rx, &silent).await,
        "expected user_language after deferred identify when TTS was active at SpeechEnd"
    );

    agent.stop().await.unwrap();
}

/// Emits `Final` only on the first `poll_transcript` after `finalize_utterance` (Sherpa-like).
struct FinalOnceStt {
    bytes: Arc<Mutex<usize>>,
    finalized: Arc<Mutex<bool>>,
    emitted: AtomicBool,
}

#[async_trait::async_trait]
impl SttProvider for FinalOnceStt {
    fn vendor_name(&self) -> &'static str {
        "final-once"
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
        if self.emitted.load(Ordering::SeqCst) {
            return Ok(None);
        }
        if !*self.finalized.lock().unwrap() {
            return Ok(None);
        }
        *self.finalized.lock().unwrap() = false;
        self.emitted.store(true, Ordering::SeqCst);
        Ok(Some(SttTranscript::Final("hello".into())))
    }

    async fn finalize_utterance(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        *self.finalized.lock().unwrap() = true;
        Ok(())
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
}

struct FinalOnceLidFactory {
    stt_bytes: Arc<Mutex<usize>>,
    lid_sleep_ms: u64,
}

impl VendorFactory for FinalOnceLidFactory {
    fn create_stt(
        &self,
        config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        if config.provider == SttVendor::Mock {
            Ok(Box::new(FinalOnceStt {
                bytes: Arc::clone(&self.stt_bytes),
                finalized: Arc::new(Mutex::new(false)),
                emitted: AtomicBool::new(false),
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

fn agent_with_final_once_stt(lid_sleep_ms: u64) -> Arc<VoiceAgent> {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let factory = Arc::new(FinalOnceLidFactory {
        stt_bytes: Arc::clone(&stt_bytes),
        lid_sleep_ms,
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
    vad.stt_gate_hold_ms = 80;
    vad.stt_listen_timeout_ms = 60_000;

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
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            tts_exclusion: Some(true),
        }),
        vad,
        ..Default::default()
    };

    VoiceAgent::new(config, Arc::new(registry)).unwrap()
}

struct CountingLidTestFactory {
    stt_bytes: Arc<Mutex<usize>>,
    lid_calls: Arc<AtomicUsize>,
    lid_sleep_ms: u64,
    lid_languages: Vec<String>,
}

impl VendorFactory for CountingLidTestFactory {
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
        Ok(Some(Box::new(CountingLanguageId {
            calls: Arc::clone(&self.lid_calls),
            sleep_ms: self.lid_sleep_ms,
            languages: self.lid_languages.clone(),
        })))
    }
}

fn agent_with_counting_lid(
    stt_bytes: Arc<Mutex<usize>>,
    lid_calls: Arc<AtomicUsize>,
    gate_stt: bool,
    continuous: Option<bool>,
    include_stt: bool,
    lid_languages: Vec<String>,
) -> Arc<VoiceAgent> {
    let factory = Arc::new(CountingLidTestFactory {
        stt_bytes: Arc::clone(&stt_bytes),
        lid_calls: Arc::clone(&lid_calls),
        lid_sleep_ms: 50,
        lid_languages,
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
    vad.gate_stt = gate_stt;
    if gate_stt {
        vad.stt_gate_hold_ms = 80;
    }

    let config = VoiceAgentConfig {
        stt: if include_stt {
            Some(SttConfig {
                provider: SttVendor::Mock,
                model: None,
                model_path: None,
                language: Some("en".into()),
                api_key: None,
            })
        } else {
            None
        },
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
            continuous,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            tts_exclusion: Some(true),
        }),
        vad,
        ..Default::default()
    };

    VoiceAgent::new(config, Arc::new(registry)).unwrap()
}

async fn wait_for_event(
    rx: &mut tokio::sync::broadcast::Receiver<node_webrtc_rust_speech::events::SpeechEvent>,
    kind: SpeechEventKind,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        while let Ok(event) = rx.try_recv() {
            if event.kind == kind {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

async fn wait_for_user_language(
    _agent: &VoiceAgent,
    rx: &mut tokio::sync::broadcast::Receiver<node_webrtc_rust_speech::events::SpeechEvent>,
) -> bool {
    wait_for_event(rx, SpeechEventKind::UserLanguage).await
}

async fn collect_events_until_final(
    rx: &mut tokio::sync::broadcast::Receiver<node_webrtc_rust_speech::events::SpeechEvent>,
    agent: &VoiceAgent,
    silent: &[u8],
) -> Vec<node_webrtc_rust_speech::events::SpeechEvent> {
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        while let Ok(event) = rx.try_recv() {
            events.push(event);
            if events.last().map(|e| e.kind) == Some(SpeechEventKind::UserSpeechFinal) {
                return events;
            }
        }
        agent
            .process_inbound_pcm(Bytes::from(silent.to_vec()), 20)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    events
}

fn agent_with_timed_overlap_lid(
    stt_bytes: Arc<Mutex<usize>>,
    guard: Arc<OverlapGuard>,
    lid_sleep_ms: u64,
) -> (Arc<VoiceAgent>, Arc<Mutex<Option<Instant>>>, Arc<Mutex<Option<Instant>>>) {
    let started_at = Arc::new(Mutex::new(None));
    let ended_at = Arc::new(Mutex::new(None));
    let lid_started = Arc::clone(&started_at);
    let lid_ended = Arc::clone(&ended_at);
    let guard_c = Arc::clone(&guard);

    struct TimedOverlapLidFactory {
        stt_bytes: Arc<Mutex<usize>>,
        guard: Arc<OverlapGuard>,
        lid_sleep_ms: u64,
        started_at: Arc<Mutex<Option<Instant>>>,
        ended_at: Arc<Mutex<Option<Instant>>>,
    }

    impl VendorFactory for TimedOverlapLidFactory {
        fn create_stt(
            &self,
            config: &SttConfig,
        ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
            if config.provider == SttVendor::Mock {
                Ok(Box::new(FinalOnceStt {
                    bytes: Arc::clone(&self.stt_bytes),
                    finalized: Arc::new(Mutex::new(false)),
                    emitted: AtomicBool::new(false),
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
            Ok(Some(Box::new(TimedOverlapLanguageId {
                guard: Arc::clone(&self.guard),
                sleep_ms: self.lid_sleep_ms,
                started_at: Arc::clone(&self.started_at),
                ended_at: Arc::clone(&self.ended_at),
            })))
        }
    }

    let factory = Arc::new(TimedOverlapLidFactory {
        stt_bytes: Arc::clone(&stt_bytes),
        guard: guard_c,
        lid_sleep_ms,
        started_at: lid_started,
        ended_at: lid_ended,
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
    vad.stt_gate_hold_ms = 80;
    vad.stt_listen_timeout_ms = 60_000;

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
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            tts_exclusion: Some(true),
        }),
        vad,
        ..Default::default()
    };

    (
        VoiceAgent::new(config, Arc::new(registry)).unwrap(),
        started_at,
        ended_at,
    )
}

async fn wait_for_user_language_with_pcm(
    agent: &VoiceAgent,
    rx: &mut tokio::sync::broadcast::Receiver<node_webrtc_rust_speech::events::SpeechEvent>,
    silent: &[u8],
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        drive_silent_frames(agent, silent, 5).await;
        if wait_for_user_language(agent, rx).await {
            return true;
        }
    }
    false
}

async fn drive_loud_frames(agent: &VoiceAgent, loud: &[u8], count: usize) {
    for _ in 0..count {
        agent
            .process_inbound_pcm(Bytes::from(loud.to_vec()), 20)
            .await
            .unwrap();
    }
}

async fn drive_silent_frames(agent: &VoiceAgent, silent: &[u8], count: usize) {
    for _ in 0..count {
        agent
            .process_inbound_pcm(Bytes::from(silent.to_vec()), 20)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn language_id_runs_once_per_utterance_by_default() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let lid_calls = Arc::new(AtomicUsize::new(0));
    // gate_stt off so VAD closes utterances without waiting for mock STT finals.
    let agent = agent_with_counting_lid(
        Arc::clone(&stt_bytes),
        Arc::clone(&lid_calls),
        false,
        None,
        false,
        vec!["en".into(), "de".into()],
    );
    let mut rx = agent.subscribe_events();

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    // Utterance A: buffer past min_speech_ms, then keep speaking — no mid-utterance identify.
    drive_loud_frames(&agent, &loud, 12).await;
    assert!(
        wait_for_event(&mut rx, SpeechEventKind::UserSpeakingStart).await,
        "expected user_speaking_start for utterance A"
    );

    drive_loud_frames(&agent, &loud, 80).await;
    assert_eq!(
        lid_calls.load(Ordering::SeqCst),
        0,
        "default mode must not identify during speech before speaking_end"
    );

    drive_silent_frames(&agent, &silent, 12).await;
    assert!(
        wait_for_user_language(&agent, &mut rx).await,
        "expected user_language after speaking_end for utterance A"
    );
    assert_eq!(
        lid_calls.load(Ordering::SeqCst),
        1,
        "default mode must identify exactly once per utterance at hang-up"
    );

    // Silence long enough for VAD SpeechEnd before the next SpeechStart (new utterance).
    drive_silent_frames(&agent, &silent, 12).await;

    assert_eq!(
        lid_calls.load(Ordering::SeqCst),
        1,
        "silence between utterances must not start a second identify"
    );

    // Utterance B: new turn.
    drive_loud_frames(&agent, &loud, 12).await;
    assert!(
        wait_for_event(&mut rx, SpeechEventKind::UserSpeakingStart).await,
        "expected user_speaking_start for utterance B"
    );
    drive_silent_frames(&agent, &silent, 12).await;
    assert!(
        wait_for_user_language(&agent, &mut rx).await,
        "expected user_language for utterance B"
    );
    assert_eq!(
        lid_calls.load(Ordering::SeqCst),
        2,
        "new utterance should start a second identify"
    );

    agent.stop().await.unwrap();
}

#[tokio::test]
async fn continuous_language_id_rechecks_during_long_utterance() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let lid_calls = Arc::new(AtomicUsize::new(0));
    let agent = agent_with_counting_lid(
        Arc::clone(&stt_bytes),
        Arc::clone(&lid_calls),
        true,
        Some(true),
        true,
        vec!["en".into()],
    );
    let mut rx = agent.subscribe_events();

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();

    drive_loud_frames(&agent, &loud, 12).await;
    assert!(
        wait_for_user_language(&agent, &mut rx).await,
        "expected first user_language in continuous mode"
    );

    // Keep speaking through a second min_speech_ms window after first identify completes.
    drive_loud_frames(&agent, &loud, 80).await;

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && lid_calls.load(Ordering::SeqCst) < 2 {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        lid_calls.load(Ordering::SeqCst) > 1,
        "continuous mode should identify more than once during one long utterance"
    );

    agent.stop().await.unwrap();
}

#[tokio::test]
async fn hangup_does_not_start_second_identify_after_inbound_lid() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let lid_calls = Arc::new(AtomicUsize::new(0));
    let agent = agent_with_counting_lid(
        Arc::clone(&stt_bytes),
        Arc::clone(&lid_calls),
        false,
        None,
        false,
        vec!["en".into()],
    );
    let mut rx = agent.subscribe_events();

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    // Buffer past min_speech_ms=200 — default mode does not spawn identify mid-utterance.
    for _ in 0..12 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    assert_eq!(
        lid_calls.load(Ordering::SeqCst),
        0,
        "default mode must not identify at minSpeechMs while user is still speaking"
    );

    // TTS starts — speaking_end identify must defer, not overlap or double-spawn.
    agent
        .send_text_to_tts_with_options(
            "one two three",
            SendTextToTtsOptions { non_blocking: true },
        )
        .await
        .unwrap();

    for _ in 0..5 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    assert_eq!(
        lid_calls.load(Ordering::SeqCst),
        0,
        "hang-up identify must defer while TTS is active"
    );

    agent.wait_tts_playback_idle().await.unwrap();

    let lang_deadline = Instant::now() + Duration::from_secs(3);
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
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        saw_user_language,
        "expected user_language from deferred speaking_end identify"
    );
    assert_eq!(
        lid_calls.load(Ordering::SeqCst),
        1,
        "speaking_end must yield exactly one identify after TTS idle"
    );

    agent.stop().await.unwrap();
}

#[tokio::test]
async fn short_utterance_still_gets_user_language_after_tts_idle() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let lid_calls = Arc::new(AtomicUsize::new(0));
    // gate_stt off so VAD emits user_speaking_end without waiting for mock STT finals.
    let agent = agent_with_counting_lid(
        Arc::clone(&stt_bytes),
        Arc::clone(&lid_calls),
        false,
        None,
        false,
        vec!["en".into()],
    );
    let mut rx = agent.subscribe_events();

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    // VAD speech start only — below min_speech_ms=200 for inbound LID.
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

    for _ in 0..5 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
    }

    agent.wait_tts_playback_idle().await.unwrap();

    let lang_deadline = Instant::now() + Duration::from_secs(3);
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
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        saw_user_language,
        "short utterance should get user_language after deferred hang-up identify"
    );
    assert_eq!(
        lid_calls.load(Ordering::SeqCst),
        1,
        "short utterance should run exactly one identify after TTS idle"
    );

    agent.stop().await.unwrap();
}

#[tokio::test]
async fn default_lid_completes_before_user_speech_final() {
    let agent = agent_with_final_once_stt(250);
    let mut rx = agent.subscribe_events();

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    drive_loud_frames(&agent, &loud, 12).await;
    let events = collect_events_until_final(&mut rx, &agent, &silent).await;

    let lang_idx = events
        .iter()
        .position(|e| e.kind == SpeechEventKind::UserLanguage);
    let end_idx = events
        .iter()
        .position(|e| e.kind == SpeechEventKind::UserSpeakingEnd);
    let final_idx = events
        .iter()
        .position(|e| e.kind == SpeechEventKind::UserSpeechFinal);

    assert!(lang_idx.is_some(), "expected user_language");
    assert!(end_idx.is_some(), "expected user_speaking_end");
    assert!(final_idx.is_some(), "expected user_speech_final");
    assert!(
        lang_idx.unwrap() < end_idx.unwrap(),
        "user_language must precede user_speaking_end"
    );
    assert!(
        end_idx.unwrap() + 1 == final_idx.unwrap(),
        "user_speaking_end must immediately precede user_speech_final"
    );

    agent.stop().await.unwrap();
}

#[tokio::test]
async fn detached_speak_after_final_never_overlaps_lid() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let guard = Arc::new(OverlapGuard {
        lid_in_flight: AtomicUsize::new(0),
        tts_in_flight: AtomicUsize::new(0),
        violated: AtomicBool::new(false),
    });
    let (agent, _lid_start, lid_end) =
        agent_with_timed_overlap_lid(Arc::clone(&stt_bytes), Arc::clone(&guard), 300);
    let mut rx = agent.subscribe_events();

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    drive_loud_frames(&agent, &loud, 12).await;
    drive_silent_frames(&agent, &silent, 40).await;
    assert!(
        wait_for_event(&mut rx, SpeechEventKind::UserSpeechFinal).await,
        "expected user_speech_final before detached speak"
    );

    let agent_spawn = Arc::clone(&agent);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        agent_spawn
            .send_text_to_tts_with_options(
                "echo. one two three",
                SendTextToTtsOptions { non_blocking: true },
            )
            .await
            .unwrap();
    });

    agent.wait_tts_playback_idle().await.unwrap();

    assert!(
        !guard.violated.load(Ordering::SeqCst),
        "detached speak after final must not overlap LID"
    );
    assert!(
        lid_end.lock().unwrap().is_some(),
        "LID should have completed"
    );

    agent.stop().await.unwrap();
}

#[tokio::test]
async fn tts_active_at_speech_end_defers_lid_to_drain() {
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

    agent
        .send_text_to_tts_with_options(
            "this is a longer agent preface to keep playback active across user speech end",
            SendTextToTtsOptions { non_blocking: true },
        )
        .await
        .unwrap();

    drive_loud_frames(&agent, &loud, 8).await;
    drive_silent_frames(&agent, &silent, 40).await;

    agent.wait_tts_playback_idle().await.unwrap();

    assert!(
        !guard.violated.load(Ordering::SeqCst),
        "deferred LID after TTS drain must not overlap synthesis"
    );
    assert!(
        wait_for_user_language_with_pcm(&agent, &mut rx, &silent).await,
        "expected user_language after deferred identify when TTS was active at SpeechEnd"
    );

    agent.stop().await.unwrap();
}

struct FailingLidFactory {
    stt_bytes: Arc<Mutex<usize>>,
}

impl VendorFactory for FailingLidFactory {
    fn create_stt(
        &self,
        config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        if config.provider == SttVendor::Mock {
            Ok(Box::new(FinalOnceStt {
                bytes: Arc::clone(&self.stt_bytes),
                finalized: Arc::new(Mutex::new(false)),
                emitted: AtomicBool::new(false),
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
        Ok(Some(Box::new(FailingLanguageId)))
    }
}

#[tokio::test]
async fn lid_error_does_not_block_final() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let factory = Arc::new(FailingLidFactory {
        stt_bytes: Arc::clone(&stt_bytes),
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
    vad.stt_gate_hold_ms = 80;

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
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            tts_exclusion: Some(true),
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

    drive_loud_frames(&agent, &loud, 12).await;
    let events = collect_events_until_final(&mut rx, &agent, &silent).await;

    assert!(
        events.iter().any(|e| e.kind == SpeechEventKind::UserSpeechFinal),
        "expected user_speech_final despite LID error"
    );
    assert!(
        !events.iter().any(|e| e.kind == SpeechEventKind::UserLanguage),
        "LID error must not emit user_language"
    );

    agent.stop().await.unwrap();
}

#[tokio::test]
async fn tts_synthesis_waits_for_in_flight_lid() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let guard = Arc::new(OverlapGuard {
        lid_in_flight: AtomicUsize::new(0),
        tts_in_flight: AtomicUsize::new(0),
        violated: AtomicBool::new(false),
    });
    let started_at = Arc::new(Mutex::new(None));
    let ended_at = Arc::new(Mutex::new(None));

    struct TimedOverlapLidFactory {
        stt_bytes: Arc<Mutex<usize>>,
        guard: Arc<OverlapGuard>,
        lid_sleep_ms: u64,
        started_at: Arc<Mutex<Option<Instant>>>,
        ended_at: Arc<Mutex<Option<Instant>>>,
    }

    impl VendorFactory for TimedOverlapLidFactory {
        fn create_stt(
            &self,
            config: &SttConfig,
        ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
            if config.provider == SttVendor::Mock {
                Ok(Box::new(FinalOnceStt {
                    bytes: Arc::clone(&self.stt_bytes),
                    finalized: Arc::new(Mutex::new(false)),
                    emitted: AtomicBool::new(false),
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
            Ok(Some(Box::new(TimedOverlapLanguageId {
                guard: Arc::clone(&self.guard),
                sleep_ms: self.lid_sleep_ms,
                started_at: Arc::clone(&self.started_at),
                ended_at: Arc::clone(&self.ended_at),
            })))
        }
    }

    let factory = Arc::new(TimedOverlapLidFactory {
        stt_bytes: Arc::clone(&stt_bytes),
        guard: Arc::clone(&guard),
        lid_sleep_ms: 300,
        started_at: Arc::clone(&started_at),
        ended_at: Arc::clone(&ended_at),
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
    vad.stt_gate_hold_ms = 80;
    vad.stt_listen_timeout_ms = 60_000;

    let agent = VoiceAgent::new(
        VoiceAgentConfig {
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
                continuous: Some(true),
                lid_max_clip_ms: None,
                lid_gate_max_wait_ms: None,
            tts_exclusion: Some(true),
            }),
            vad,
            ..Default::default()
        },
        Arc::new(registry),
    )
    .unwrap();

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    drive_loud_frames(&agent, &loud, 15).await;

    let lid_started_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < lid_started_deadline && started_at.lock().unwrap().is_none() {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        started_at.lock().unwrap().is_some(),
        "continuous LID should start mid-utterance"
    );

    agent
        .send_text_to_tts_with_options(
            "during lid",
            SendTextToTtsOptions { non_blocking: true },
        )
        .await
        .unwrap();

    agent.wait_tts_playback_idle().await.unwrap();

    assert!(
        !guard.violated.load(Ordering::SeqCst),
        "TTS synthesis must wait for in-flight continuous LID"
    );
    let lid_done_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < lid_done_deadline && ended_at.lock().unwrap().is_none() {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        ended_at.lock().unwrap().is_some(),
        "continuous LID identify should complete"
    );

    agent.stop().await.unwrap();
}

#[tokio::test]
async fn lid_gate_bound_never_fires_with_mock() {
    let agent = agent_with_final_once_stt(200);
    let mut rx = agent.subscribe_events();

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    drive_loud_frames(&agent, &loud, 12).await;
    drive_silent_frames(&agent, &silent, 40).await;
    assert!(
        wait_for_event(&mut rx, SpeechEventKind::UserSpeechFinal).await,
        "expected user_speech_final after mock LID"
    );

    assert_eq!(
        agent.lid_gate_bound_hits(),
        0,
        "lid_gate_max_wait_ms bound must not fire with 200ms mock LID"
    );

    agent.stop().await.unwrap();
}

struct RecordingTts {
    synthesis_started: Arc<Mutex<Option<Instant>>>,
}

#[async_trait::async_trait]
impl TtsProvider for RecordingTts {
    fn vendor_name(&self) -> &'static str {
        "recording-mock"
    }

    async fn synthesize(
        &self,
        _text: &str,
    ) -> node_webrtc_rust_speech::SpeechResult<Vec<TtsAudioChunk>> {
        *self.synthesis_started.lock().unwrap() = Some(Instant::now());
        tokio::time::sleep(Duration::from_millis(40)).await;
        Ok(vec![TtsAudioChunk {
            pcm: Bytes::from(vec![0_u8; 4800 * 4]),
            duration_ms: 100,
        }])
    }
}

#[tokio::test]
async fn remote_tts_exclusion_off_does_not_gate_final_or_tts() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let lid_started = Arc::new(Mutex::new(None));
    let lid_ended = Arc::new(Mutex::new(None));
    let tts_started = Arc::new(Mutex::new(None));

    struct RemoteLidFactory {
        stt_bytes: Arc<Mutex<usize>>,
        lid_started: Arc<Mutex<Option<Instant>>>,
        lid_ended: Arc<Mutex<Option<Instant>>>,
        tts_started: Arc<Mutex<Option<Instant>>>,
    }

    impl VendorFactory for RemoteLidFactory {
        fn create_stt(
            &self,
            config: &SttConfig,
        ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
            if config.provider == SttVendor::Mock {
                Ok(Box::new(FinalOnceStt {
                    bytes: Arc::clone(&self.stt_bytes),
                    finalized: Arc::new(Mutex::new(false)),
                    emitted: AtomicBool::new(false),
                }))
            } else {
                MockFactory.create_stt(config)
            }
        }

        fn create_tts(
            &self,
            _config: &TtsConfig,
        ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn TtsProvider>> {
            Ok(Box::new(RecordingTts {
                synthesis_started: Arc::clone(&self.tts_started),
            }))
        }

        fn create_language_id(
            &self,
            _config: &LanguageIdConfig,
        ) -> node_webrtc_rust_speech::SpeechResult<Option<Box<dyn LanguageIdProvider>>> {
            Ok(Some(Box::new(TimedOverlapLanguageId {
                guard: Arc::new(OverlapGuard {
                    lid_in_flight: AtomicUsize::new(0),
                    tts_in_flight: AtomicUsize::new(0),
                    violated: AtomicBool::new(false),
                }),
                sleep_ms: 300,
                started_at: Arc::clone(&self.lid_started),
                ended_at: Arc::clone(&self.lid_ended),
            })))
        }
    }

    let factory = Arc::new(RemoteLidFactory {
        stt_bytes: Arc::clone(&stt_bytes),
        lid_started: Arc::clone(&lid_started),
        lid_ended: Arc::clone(&lid_ended),
        tts_started: Arc::clone(&tts_started),
    });
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::clone(&factory) as Arc<dyn VendorFactory>);
    registry.register_tts(TtsVendor::Mock, Arc::clone(&factory) as Arc<dyn VendorFactory>);
    registry.register_stt(SttVendor::LocalSherpa, factory);

    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 80;
    vad.stt_listen_timeout_ms = 60_000;

    let agent = VoiceAgent::new(
        VoiceAgentConfig {
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
                continuous: None,
                lid_max_clip_ms: None,
                lid_gate_max_wait_ms: None,
                tts_exclusion: Some(false),
            }),
            vad,
            ..Default::default()
        },
        Arc::new(registry),
    )
    .unwrap();

    let mut rx = agent.subscribe_events();
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    let utterance_start = Instant::now();

    drive_loud_frames(&agent, &loud, 12).await;
    drive_silent_frames(&agent, &silent, 40).await;

    assert!(
        wait_for_event(&mut rx, SpeechEventKind::UserSpeechFinal).await,
        "expected user_speech_final without waiting for slow LID"
    );
    let final_at = Instant::now();
    assert!(
        final_at.duration_since(utterance_start) < Duration::from_millis(400),
        "final must not await 300ms mock LID when ttsExclusion is off"
    );

    agent
        .send_text_to_tts_with_options(
            "remote echo",
            SendTextToTtsOptions { non_blocking: true },
        )
        .await
        .unwrap();

    let tts_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < tts_deadline && tts_started.lock().unwrap().is_none() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        tts_started.lock().unwrap().is_some(),
        "TTS synthesis should start while LID may still be in flight"
    );

    let lid_done_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < lid_done_deadline && lid_ended.lock().unwrap().is_none() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        lid_ended.lock().unwrap().is_some(),
        "LID should complete in background"
    );
    assert!(
        tts_started.lock().unwrap().unwrap() < lid_ended.lock().unwrap().unwrap(),
        "TTS synthesis must start before LID completes when exclusion is off"
    );
    assert!(
        lid_started.lock().unwrap().is_some(),
        "LID should have started at speech end"
    );

    assert!(
        wait_for_user_language_with_pcm(&agent, &mut rx, &silent).await,
        "user_language should still arrive after background identify"
    );
    assert_eq!(
        agent.lid_gate_bound_hits(),
        0,
        "gate bound must not fire when final is not gated"
    );

    agent.stop().await.unwrap();
}
