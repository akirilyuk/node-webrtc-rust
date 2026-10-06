//! Session churn: repeated agent create/start/stop (or drop) must release workers, the PCM
//! writer closure and the agent itself. Uses the mock vendor (no models).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsAudioChunk, TtsProvider, VendorFactory};
use node_webrtc_rust_speech::{PcmReader, PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

fn churn_agent() -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
    let config = VoiceAgentConfig {
        vad: VadConfig {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    VoiceAgent::new(config, Arc::new(registry)).unwrap()
}

/// 20 ms stereo s16le 48 kHz, 440 Hz sine, amplitude 10000, L = R.
fn tone_frame() -> Bytes {
    let mut pcm = Vec::with_capacity(3840);
    for i in 0..960 {
        let t = i as f64 / 48_000.0;
        let sample = (10_000.0 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    Bytes::from(pcm)
}

async fn wait_until(mut f: impl FnMut() -> bool, max: Duration) -> bool {
    let deadline = Instant::now() + max;
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn drive_cycle(agent: &Arc<VoiceAgent>, probe: &Arc<()>, frame: &Bytes) {
    let probe_for_writer = Arc::clone(probe);
    let writer: PcmWriter = Arc::new(move |_pcm, _ms| {
        let _keep = &probe_for_writer;
        Ok(())
    });
    let reader: PcmReader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start(None).await.unwrap();
    for _ in 0..20 {
        agent.process_inbound_pcm(frame.clone(), 20).await.unwrap();
    }
    agent
        .send_text_to_tts_with_options(
            "hello churn",
            SendTextToTtsOptions {
                non_blocking: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn churn_start_stop_releases_workers_and_writer() {
    let probe = Arc::new(());
    let frame = tone_frame();
    for cycle in 0..300 {
        let agent = churn_agent();
        drive_cycle(&agent, &probe, &frame).await;
        agent.stop().await.unwrap();
        assert!(
            wait_until(
                || agent.tts_worker_tasks_alive() == 0 && agent.tts_vendor_calls_inflight() == 0,
                Duration::from_secs(2)
            )
            .await,
            "cycle {cycle}: tts workers alive={} vendor inflight={} after stop",
            agent.tts_worker_tasks_alive(),
            agent.tts_vendor_calls_inflight()
        );
        drop(agent);
    }
    assert!(
        wait_until(|| Arc::strong_count(&probe) == 1, Duration::from_secs(2)).await,
        "writer closures leaked after 300 cycles: strong_count={}",
        Arc::strong_count(&probe)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn churn_drop_without_stop_releases_agent() {
    let probe = Arc::new(());
    let frame = tone_frame();
    for cycle in 0..100 {
        let agent = churn_agent();
        drive_cycle(&agent, &probe, &frame).await;
        let weak: Weak<VoiceAgent> = Arc::downgrade(&agent);
        drop(agent);
        assert!(
            wait_until(|| weak.upgrade().is_none(), Duration::from_secs(2)).await,
            "cycle {cycle}: agent still alive after drop without stop()"
        );
    }
    assert!(
        wait_until(|| Arc::strong_count(&probe) == 1, Duration::from_secs(2)).await,
        "writer closures leaked after 100 drop-without-stop cycles: strong_count={}",
        Arc::strong_count(&probe)
    );
}

/// 10 s of stereo s16le 48 kHz silence.
const LONG_CHUNK_BYTES: usize = 1_920_000;

struct LongTts {
    pcm: Bytes,
}

#[async_trait]
impl TtsProvider for LongTts {
    fn vendor_name(&self) -> &'static str {
        "churn-long"
    }

    async fn synthesize(&self, _text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        Ok(vec![TtsAudioChunk {
            pcm: self.pcm.clone(),
            duration_ms: 10_000,
        }])
    }
}

struct LongTtsFactory {
    pcm: Bytes,
}

impl VendorFactory for LongTtsFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(LongTts {
            pcm: self.pcm.clone(),
        }))
    }
}

/// Dropping the last owner while a long utterance is mid-playback must stop the drain at once
/// and release the writer (class `rt.task-holds-strong-self`). The mock TTS in the tests above
/// only plays ~550 ms, which hid this on fast machines.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn churn_drop_without_stop_mid_playback_releases_writer() {
    let probe = Arc::new(());
    let pcm = Bytes::from(vec![0u8; LONG_CHUNK_BYTES]);
    for cycle in 0..20 {
        let mut registry = VendorRegistry::new();
        registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
        registry.register_tts(
            TtsVendor::Mock,
            Arc::new(LongTtsFactory { pcm: pcm.clone() }),
        );
        let agent = VoiceAgent::new(VoiceAgentConfig::default(), Arc::new(registry)).unwrap();

        let frames = Arc::new(AtomicUsize::new(0));
        let probe_for_writer = Arc::clone(&probe);
        let frames_for_writer = Arc::clone(&frames);
        let writer: PcmWriter = Arc::new(move |_pcm, _ms| {
            let _keep = &probe_for_writer;
            frames_for_writer.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let reader: PcmReader = Arc::new(|| Ok(None));
        agent.attach(reader, writer).await.unwrap();
        agent.start(None).await.unwrap();
        agent
            .send_text_to_tts_with_options(
                "long",
                SendTextToTtsOptions {
                    non_blocking: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(
            wait_until(
                || frames.load(Ordering::SeqCst) >= 3,
                Duration::from_secs(2)
            )
            .await,
            "cycle {cycle}: playback never started"
        );
        drop(agent);
        assert!(
            wait_until(|| Arc::strong_count(&probe) == 1, Duration::from_millis(500)).await,
            "cycle {cycle}: writer still held 500 ms after drop without stop() mid-playback: strong_count={}",
            Arc::strong_count(&probe)
        );
    }
}
