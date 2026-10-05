//! Session churn: repeated agent create/start/stop (or drop) must release workers, the PCM
//! writer closure and the agent itself. Uses the mock vendor (no models).

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttVendor, TtsVendor, VadConfig, VoiceAgentConfig,
};
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
