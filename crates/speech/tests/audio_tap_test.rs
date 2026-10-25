//! Test-only audio tap: the STT actor writes one WAV per utterance (what it pushed to the
//! provider) and the agent writes the listener's inbound audio per VAD speech end.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{
    FinalizeMode, SttOutput, SttProvider, SttTranscript, VendorFactory,
};
use node_webrtc_rust_speech::stt_actor::{spawn_stt_actor, SttActorOptions, SttCommand};
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// A fresh, empty directory that lives for the whole process.
fn unique_dir(label: &str) -> &'static Path {
    let dir = std::env::temp_dir().join(format!(
        "audio-tap-test-{label}-{}-{}",
        std::process::id(),
        DIR_SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Box::leak(Box::new(dir)).as_path()
}

fn list(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    files.sort();
    files
}

fn name_of(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().to_string()
}

/// Waits (the files are written on a thread) until `pred` holds for the directory listing.
async fn wait_files(dir: &Path, pred: impl Fn(&[PathBuf]) -> bool) -> Vec<PathBuf> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let files = list(dir);
        // A file that exists but is still being written is not complete yet.
        let complete = files.iter().all(|f| {
            std::fs::read(f).is_ok_and(|b| {
                b.len() >= 44
                    && u32::from_le_bytes([b[40], b[41], b[42], b[43]]) as usize == b.len() - 44
            })
        });
        if complete && pred(&files) {
            return files;
        }
        if Instant::now() > deadline {
            return files;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn data_len(path: &Path) -> usize {
    let b = std::fs::read(path).unwrap();
    assert_eq!(&b[0..4], b"RIFF");
    assert_eq!(&b[36..40], b"data");
    let n = u32::from_le_bytes([b[40], b[41], b[42], b[43]]) as usize;
    assert_eq!(b.len() - 44, n, "{}", path.display());
    n
}

/// Provider that accepts everything and answers each finalize request with a bare marker.
#[derive(Default)]
struct Sink {
    pushed: Arc<Mutex<VecDeque<Bytes>>>,
    finalize_pending: bool,
}

#[async_trait]
impl SttProvider for Sink {
    fn vendor_name(&self) -> &'static str {
        "sink"
    }
    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }
    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }
    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        self.pushed.lock().unwrap().push_back(pcm);
        Ok(())
    }
    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        Ok(None)
    }
    async fn poll_output(&mut self) -> SpeechResult<Option<SttOutput>> {
        if std::mem::take(&mut self.finalize_pending) {
            return Ok(Some(SttOutput::FinalizeDone));
        }
        Ok(None)
    }
    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        Ok(())
    }
    async fn request_finalize(&mut self) -> SpeechResult<()> {
        self.finalize_pending = true;
        Ok(())
    }
    fn finalize_mode(&self) -> FinalizeMode {
        FinalizeMode::Deferred
    }
}

fn actor_options(dir: Option<&'static Path>, agent_id: u64) -> SttActorOptions {
    SttActorOptions {
        open_wait_max_ms: 120_000,
        test_final_delay_ms: 0,
        audio_tap_dir: dir,
        audio_tap_agent_id: agent_id,
        metric_attrs: Default::default(),
        stt_latency_sink: node_webrtc_rust_speech::stt_actor::default_stt_latency_sink(),
    }
}

async fn run_actor_commands(dir: Option<&'static Path>, agent_id: u64) {
    let (handle, _rx) = spawn_stt_actor(
        Some(Box::new(Sink::default())),
        Arc::new(VendorRegistry::new()),
        actor_options(dir, agent_id),
    );
    for _ in 0..3 {
        assert!(handle.send(SttCommand::Audio(Bytes::from(vec![1u8; 640]))));
    }
    assert!(handle.send(SttCommand::EndUtterance { key: 7 }));
    // Barrier-like: a Stop is handled after both commands above.
    let (tx, rx) = tokio::sync::oneshot::channel();
    assert!(handle.send(SttCommand::Stop { done: tx }));
    let _ = rx.await;
}

#[tokio::test]
async fn stt_actor_tap_writes_one_wav_per_utterance() {
    let dir = unique_dir("actor");
    run_actor_commands(Some(dir), 42).await;
    let files = wait_files(dir, |f| !f.is_empty()).await;
    assert_eq!(files.len(), 1, "{files:?}");
    let name = name_of(&files[0]);
    assert!(name.starts_with("stt-in-"), "{name}");
    assert!(name.ends_with("-a42-utt7.wav"), "{name}");
    assert_eq!(data_len(&files[0]), 1920);
}

#[tokio::test]
async fn stt_actor_without_tap_writes_nothing() {
    let dir = unique_dir("actor-off");
    run_actor_commands(None, 0).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(list(dir).is_empty());
}

/// 48 kHz stereo 20 ms frame of a constant level.
fn stereo_frame(level: i16) -> Bytes {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&level.to_le_bytes());
        pcm.extend_from_slice(&level.to_le_bytes());
    }
    Bytes::from(pcm)
}

/// Accepts audio and never produces a transcript, so the utterance ends through the VAD and the
/// gate hold only (the mock vendor answers with a final on the first frame).
struct SilentStt;

#[async_trait]
impl SttProvider for SilentStt {
    fn vendor_name(&self) -> &'static str {
        "silent"
    }
    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }
    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }
    async fn push_audio(&mut self, _pcm: Bytes) -> SpeechResult<()> {
        Ok(())
    }
    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        Ok(None)
    }
}

struct MockStt;

impl VendorFactory for MockStt {
    fn create_stt(&self, _config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(SilentStt))
    }
    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

#[tokio::test]
async fn agent_inbound_tap_flushes_a_segment_at_speech_end() {
    let dir = unique_dir("agent");
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockStt));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockStt));

    let mut vad = VadConfig {
        enabled: true,
        provider: "energy".into(),
        threshold: 0.05,
        min_speech_duration_ms: 200,
        min_silence_duration_ms: 300,
        speech_pad_ms: 200,
        gate_stt: true,
        stt_gate_hold_ms: 100,
        ..Default::default()
    };
    vad.barge_in.enabled = false;
    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
            endpoint: None,
            transport: None,
        }),
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
            sentence_case: Default::default(),
            transport: None,
        }),
        vad,
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.set_audio_tap_dir_for_test(dir).await;
    agent.start(None).await.unwrap();

    // The energy VAD starts speech after 10 loud frames (200 ms) and ends it at the 15th
    // silent frame (300 ms): 25 frames up to and including the one that produced SpeechEnd.
    let loud = stereo_frame(6000);
    let quiet = stereo_frame(0);
    for _ in 0..10 {
        agent.process_inbound_pcm(loud.clone(), 20).await.unwrap();
    }
    for _ in 0..80 {
        agent.process_inbound_pcm(quiet.clone(), 20).await.unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    let files = wait_files(dir, |f| {
        f.iter().any(|p| name_of(p).starts_with("inbound-"))
            && f.iter().any(|p| name_of(p).starts_with("stt-in-"))
    })
    .await;
    let inbound: Vec<&PathBuf> = files
        .iter()
        .filter(|p| name_of(p).starts_with("inbound-"))
        .collect();
    assert!(!inbound.is_empty(), "{files:?}");
    assert!(name_of(inbound[0]).ends_with("-seg0000.wav"), "{files:?}");
    assert_eq!(data_len(inbound[0]), 25 * 640, "{files:?}");
    let stt_in: Vec<&PathBuf> = files
        .iter()
        .filter(|p| name_of(p).starts_with("stt-in-"))
        .collect();
    assert!(!stt_in.is_empty(), "{files:?}");
    assert!(name_of(stt_in[0]).contains("-utt"), "{files:?}");

    let _ = agent.stop().await;
}
