//! B5: the LID PCM buffer must stay bounded while VAD never ends the utterance.
//!
//! With LID enabled the agent buffers mono 16 kHz PCM from `SpeechStart` until the utterance
//! ends. A caller that keeps talking (or steady noise above the VAD threshold) keeps the
//! utterance open, so the buffer must be capped at the LID clip length (`lid_max_clip_ms`).

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    LanguageIdConfig, SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::pipeline::{
    LanguageIdProvider, LanguageIdResult, SttProvider, SttTranscript, TtsProvider, VendorFactory,
};
use node_webrtc_rust_speech::{VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

/// 440 Hz, amplitude 10000, 20 ms of stereo 48 kHz s16le (960 frames x 2 channels = 3840 bytes).
fn tone_stereo_frame() -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for n in 0..960_u32 {
        let t = n as f64 / 48_000.0;
        let sample = (10_000.0 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    pcm
}

struct NoneLanguageId;

#[async_trait::async_trait]
impl LanguageIdProvider for NoneLanguageId {
    async fn identify(
        &self,
        _pcm: Bytes,
        _sample_rate: u32,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<LanguageIdResult>> {
        Ok(None)
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
        Ok(Some(Box::new(NoneLanguageId)))
    }
}

fn agent_with_lid(stt_bytes: Arc<Mutex<usize>>) -> Arc<VoiceAgent> {
    let factory = Arc::new(LidTestFactory { stt_bytes });
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::clone(&factory) as Arc<dyn VendorFactory>,
    );
    registry.register_stt(SttVendor::LocalSherpa, factory);
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 20;
    vad.speech_pad_ms = 20;
    vad.gate_stt = false;

    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
            endpoint: None,
        }),
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
        }),
        language_id: Some(LanguageIdConfig {
            enabled: Some(true),
            model_path: Some("/fake/lid-model".into()),
            allowlist: None,
            min_speech_ms: Some(200),
            continuous: None,
            // Default clip length (5000 ms) on purpose.
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            timing: None,
            tts_exclusion: Some(true),
        }),
        vad,
        ..Default::default()
    };

    VoiceAgent::new(config, Arc::new(registry)).unwrap()
}

#[tokio::test]
async fn lid_buffer_stays_bounded_while_vad_stays_open() {
    let stt_bytes = Arc::new(Mutex::new(0_usize));
    let agent = agent_with_lid(Arc::clone(&stt_bytes));

    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let frame = Bytes::from(tone_stereo_frame());
    // 3000 frames x 20 ms = 60 s of continuous voiced audio: VAD never reports SpeechEnd.
    for _ in 0..3000 {
        agent.process_inbound_pcm(frame.clone(), 20).await.unwrap();
    }

    let stt_total = *stt_bytes.lock().unwrap();
    assert!(
        stt_total > 0,
        "STT received no audio, so VAD never opened the utterance (test setup invalid)"
    );

    // 16 kHz mono s16le = 32 bytes/ms; cap = 2 x the default 5000 ms LID clip.
    let cap = 2 * 5000 * 32;
    let len = agent.lid_buffer_len().await;
    assert!(
        len <= cap,
        "LID buffer grew to {len} bytes after 60 s of voiced audio; cap {cap} bytes"
    );

    agent.stop().await.unwrap();
}
