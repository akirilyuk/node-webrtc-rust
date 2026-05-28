//! Voice agent orchestration (attach, start/stop, TTS injection).

use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{broadcast, Mutex};

use crate::config::{EventDeliveryMode, VoiceAgentConfig};
use crate::error::{SpeechError, SpeechResult};
use crate::events::{SpeechEvent, SpeechEventBus};
use crate::pipeline::{SttProvider, TtsProvider};
use crate::registry::VendorRegistry;
use crate::tts_buffer::TtsBuffer;

/// Callback invoked when PCM should be written to the outbound track.
pub type PcmWriter = Arc<dyn Fn(Bytes, u32) -> SpeechResult<()> + Send + Sync>;

/// Callback invoked to read inbound PCM from the attached remote track.
pub type PcmReader = Arc<dyn Fn() -> SpeechResult<Option<(Bytes, u32)>> + Send + Sync>;

struct AgentInner {
    config: VoiceAgentConfig,
    attached: bool,
    running: bool,
    stt: Option<Box<dyn SttProvider>>,
    tts: Option<Box<dyn TtsProvider>>,
    pcm_writer: Option<PcmWriter>,
    pcm_reader: Option<PcmReader>,
}

/// One voice agent session bound to a single peer connection.
pub struct VoiceAgent {
    event_bus: SpeechEventBus,
    tts_buffer: TtsBuffer,
    #[allow(dead_code)]
    registry: Arc<VendorRegistry>,
    inner: Mutex<AgentInner>,
}

impl VoiceAgent {
    pub fn new(config: VoiceAgentConfig, registry: Arc<VendorRegistry>) -> SpeechResult<Self> {
        let mut stt = None;
        let mut tts = None;

        if let Some(stt_cfg) = &config.stt {
            stt = Some(registry.create_stt(stt_cfg)?);
        }
        if let Some(tts_cfg) = &config.tts {
            tts = Some(registry.create_tts(tts_cfg)?);
        }

        Ok(Self {
            event_bus: SpeechEventBus::new(),
            tts_buffer: TtsBuffer::new(),
            registry,
            inner: Mutex::new(AgentInner {
                config,
                attached: false,
                running: false,
                stt,
                tts,
                pcm_writer: None,
                pcm_reader: None,
            }),
        })
    }

    pub fn event_bus(&self) -> &SpeechEventBus {
        &self.event_bus
    }

    pub fn tts_buffer(&self) -> &TtsBuffer {
        &self.tts_buffer
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<SpeechEvent> {
        self.event_bus.subscribe()
    }

    pub fn events_mode(&self) -> EventDeliveryMode {
        self.inner
            .try_lock()
            .map(|inner| inner.config.events.mode)
            .unwrap_or(EventDeliveryMode::Both)
    }

    pub async fn attach(
        &self,
        pcm_reader: PcmReader,
        pcm_writer: PcmWriter,
    ) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        inner.pcm_reader = Some(pcm_reader);
        inner.pcm_writer = Some(pcm_writer);
        inner.attached = true;
        Ok(())
    }

    pub async fn start(&self) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        if !inner.attached {
            return Err(SpeechError::NotAttached);
        }
        if inner.running {
            return Err(SpeechError::AlreadyRunning);
        }
        if let Some(stt) = inner.stt.as_mut() {
            stt.start().await?;
        }
        inner.running = true;
        Ok(())
    }

    pub async fn stop(&self) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        if !inner.running {
            return Err(SpeechError::NotRunning);
        }
        if let Some(stt) = inner.stt.as_mut() {
            stt.stop().await?;
        }
        inner.running = false;
        Ok(())
    }

    pub async fn send_text_to_tts(&self, text: &str) -> SpeechResult<()> {
        let chunks = {
            let inner = self.inner.lock().await;
            let tts = inner
                .tts
                .as_ref()
                .ok_or_else(|| SpeechError::Config("TTS not configured".into()))?;
            tts.synthesize(text).await?
        };

        self.emit(SpeechEvent::agent_speaking_start());
        self.tts_buffer.enqueue(chunks).await;
        self.drain_tts_buffer().await?;
        Ok(())
    }

    pub async fn flush_tts(&self) -> SpeechResult<()> {
        self.tts_buffer.flush().await;
        self.emit(SpeechEvent::agent_speaking_end());
        Ok(())
    }

    pub async fn pull_speech_event(&self) -> Option<SpeechEvent> {
        None
    }

    pub(crate) fn emit(&self, event: SpeechEvent) {
        let mode = self.events_mode();
        if matches!(
            mode,
            EventDeliveryMode::Callback | EventDeliveryMode::Stream | EventDeliveryMode::Both
        ) {
            self.event_bus.emit(event);
        }
    }

    async fn drain_tts_buffer(&self) -> SpeechResult<()> {
        let writer = {
            let inner = self.inner.lock().await;
            inner
                .pcm_writer
                .clone()
                .ok_or(SpeechError::NotAttached)?
        };

        while let Some(chunk) = self.tts_buffer.pop_chunk().await {
            writer(chunk.pcm, chunk.duration_ms)?;
        }
        self.emit(SpeechEvent::agent_speaking_end());
        Ok(())
    }
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
    use crate::pipeline::VendorFactory;

    struct NoopFactory;

    impl VendorFactory for NoopFactory {
        fn create_stt(&self, _config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
            Err(SpeechError::Config("noop".into()))
        }

        fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
            Err(SpeechError::Config("noop".into()))
        }
    }

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }

    #[tokio::test]
    async fn attach_requires_pcm_hooks() {
        let mut registry = VendorRegistry::new();
        registry.register_stt(SttVendor::Mock, Arc::new(NoopFactory));
        registry.register_tts(TtsVendor::Mock, Arc::new(NoopFactory));

        let config = VoiceAgentConfig {
            stt: None,
            tts: None,
            ..Default::default()
        };
        let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
        let reader: PcmReader = Arc::new(|| Ok(None));
        let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
        agent.attach(reader, writer).await.unwrap();
    }
}
