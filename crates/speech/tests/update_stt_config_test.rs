//! VoiceAgent `update_stt_config` / `update_tts_config` boundary behavior.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, TtsProvider, VendorFactory};
use node_webrtc_rust_speech::{VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::time::{sleep, Duration};

struct TaggingSttFactory {
    starts: Arc<AtomicUsize>,
}

struct TaggingStt {
    tag: String,
    starts: Arc<AtomicUsize>,
}

#[async_trait]
impl SttProvider for TaggingStt {
    fn vendor_name(&self) -> &'static str {
        "tagging"
    }

    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn poll_transcript(
        &mut self,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<SttTranscript>> {
        Ok(None)
    }
}

impl VendorFactory for TaggingSttFactory {
    fn create_stt(&self, config: &SttConfig) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        let tag = config
            .model
            .clone()
            .unwrap_or_else(|| "default".into());
        Ok(Box::new(TaggingStt {
            tag,
            starts: Arc::clone(&self.starts),
        }))
    }

    fn create_tts(
        &self,
        _config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn TtsProvider>> {
        MockFactory.create_tts(_config)
    }
}

fn agent_config(stt_model: &str) -> VoiceAgentConfig {
    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.gate_stt = false;
    VoiceAgentConfig {
        stt: Some(SttConfig {
            provider: SttVendor::Mock,
            model: Some(stt_model.into()),
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
        vad,
        ..Default::default()
    }
}

#[tokio::test]
async fn update_stt_config_swaps_at_idle_immediately() {
    let starts = Arc::new(AtomicUsize::new(0));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(TaggingSttFactory {
            starts: Arc::clone(&starts),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let agent = VoiceAgent::new(agent_config("a"), Arc::new(registry)).unwrap();
    let pcm_writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let pcm_reader: node_webrtc_rust_speech::PcmReader = Arc::new(|| Ok(None));
    agent.attach(pcm_reader, pcm_writer).await.unwrap();
    agent.start(None).await.unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 1);

    let mut next = agent_config("b").stt.unwrap();
    next.model = Some("b".into());
    agent.update_stt_config(next).await.unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 2);

    let mut rx = agent.subscribe_events();
    agent
        .update_stt_config({
            let mut c = agent_config("c").stt.unwrap();
            c.model = Some("c".into());
            c.language = Some("de".into());
            c
        })
        .await
        .unwrap();
    sleep(Duration::from_millis(20)).await;
    let mut saw = false;
    while let Ok(ev) = rx.try_recv() {
        if ev.kind == SpeechEventKind::SttConfigUpdated {
            assert_eq!(ev.language.as_deref(), Some("de"));
            saw = true;
        }
    }
    assert!(saw);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn barge_in_still_works_after_stt_config_swap() {
    let starts = Arc::new(AtomicUsize::new(0));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(TaggingSttFactory {
            starts: Arc::clone(&starts),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut config = agent_config("a");
    config.vad.barge_in.enabled = true;
    config.vad.barge_in.use_vad = true;
    config.vad.barge_in.flush_tts = true;
    config.vad.barge_in.require_stt_partial = false;

    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let pcm_writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let pcm_reader: node_webrtc_rust_speech::PcmReader = Arc::new(|| Ok(None));
    agent.attach(pcm_reader, pcm_writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent
        .update_stt_config({
            let mut c = agent_config("b").stt.unwrap();
            c.model = Some("b".into());
            c
        })
        .await
        .unwrap();
    agent.send_text_to_tts("hello").await.unwrap();
    agent.stop().await.unwrap();
}
