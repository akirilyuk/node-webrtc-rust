//! Language ID must not block inbound PCM, STT, or TTS.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    LanguageIdConfig, SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{
    LanguageIdProvider, LanguageIdResult, SttProvider, SttTranscript, TtsProvider, VendorFactory,
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
    let agent = agent_with_slow_lid(Arc::clone(&stt_bytes));
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
