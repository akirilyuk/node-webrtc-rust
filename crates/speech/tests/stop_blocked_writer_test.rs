//! `VoiceAgent::stop()` must return even when the outbound PCM writer is stuck.
//!
//! The NAPI binding's writer is `block_in_place(|| block_on(track.write_sample(..)))`. When
//! nothing consumes the outbound track that call parks the TTS drain worker inside a blocking
//! section, where `JoinHandle::abort` cannot cancel it. `stop()` used to `await` the aborted
//! handle without a bound, so the host's `await agent.stop()` never resolved. It now marks the
//! agent shutdown-unhealthy (recycle signal) after a bounded wait and returns.
//!
//! The writer is released through a channel at the end (no sleeps decide the outcome).

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttVendor, TtsConfig, TtsVendor, VoiceAgentConfig,
};
use node_webrtc_rust_speech::{PcmWriter, SpeechError, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_returns_when_outbound_writer_is_blocked() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
    let config = VoiceAgentConfig {
        stt: None,
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
        }),
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();

    // `entered` fires when the writer is parked; `release` unparks it after the assertion.
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let entered_tx = Mutex::new(entered_tx);
    let release_rx = Arc::new(Mutex::new(release_rx));
    let writer: PcmWriter = {
        let release_rx = Arc::clone(&release_rx);
        Arc::new(move |_pcm, _ms| {
            let _ = entered_tx.lock().unwrap().send(());
            let release_rx = Arc::clone(&release_rx);
            tokio::task::block_in_place(move || {
                let _ = release_rx.lock().unwrap().recv();
            });
            Ok(())
        })
    };
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent
        .send_text_to_tts_with_options("hello", SendTextToTtsOptions { non_blocking: true })
        .await
        .unwrap();
    tokio::task::spawn_blocking(move || {
        entered_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("TTS drain never reached the outbound writer");
    })
    .await
    .unwrap();

    let result = timeout(Duration::from_secs(10), agent.stop())
        .await
        .expect("stop() hung while the outbound writer was blocked");
    assert!(
        matches!(result, Err(SpeechError::TtsShutdownUnhealthy)),
        "a worker that cannot be cancelled must surface the recycle signal, got {result:?}"
    );
    let _ = release_tx.send(());
}
