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
use node_webrtc_rust_speech::pipeline::TtsAudioChunk;
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

/// Ten 20 ms stereo frames with distinct levels, as a single chunk.
struct TenFrameTts;

#[async_trait]
impl node_webrtc_rust_speech::TtsProvider for TenFrameTts {
    fn vendor_name(&self) -> &'static str {
        "ten-frame"
    }
    async fn synthesize(&self, _text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        let mut pcm = Vec::new();
        for i in 0..10 {
            pcm.extend_from_slice(&stereo_frame(100 * (i + 1)));
        }
        Ok(vec![TtsAudioChunk {
            pcm: Bytes::from(pcm),
            duration_ms: 200,
        }])
    }
}

struct TenFrameFactory;

impl VendorFactory for TenFrameFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }
    fn create_tts(
        &self,
        _config: &TtsConfig,
    ) -> SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        Ok(Box::new(TenFrameTts))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tts_out_tap_writes_the_frames_sent_to_the_outbound_writer() {
    let dir = unique_dir("tts-out");
    let mut registry = VendorRegistry::new();
    registry.register_tts(TtsVendor::Mock, Arc::new(TenFrameFactory));
    let config = VoiceAgentConfig {
        stt: None,
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
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let written: Arc<Mutex<Vec<Bytes>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&written);
    let writer: PcmWriter = Arc::new(move |pcm, _ms| {
        sink.lock().unwrap().push(pcm);
        Ok(())
    });
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.set_audio_tap_dir_for_test(dir).await;
    agent.start(None).await.unwrap();

    agent.send_text_to_tts("hello tap").await.unwrap();
    agent.wait_tts_playback_idle().await.unwrap();

    let files = wait_files(dir, |f| {
        f.iter().any(|p| name_of(p).starts_with("tts-out-"))
    })
    .await;
    let tts_out: Vec<&PathBuf> = files
        .iter()
        .filter(|p| name_of(p).starts_with("tts-out-"))
        .collect();
    assert_eq!(tts_out.len(), 1, "{files:?}");
    assert!(name_of(tts_out[0]).ends_with("-utt0000.wav"), "{files:?}");

    // The first 10 writer calls are the TTS frames; anything after is post-utterance silence.
    let expected: Vec<u8> = written
        .lock()
        .unwrap()
        .iter()
        .take(10)
        .flat_map(|b| b.to_vec())
        .collect();
    assert_eq!(expected.len(), 10 * 3840);
    let bytes = std::fs::read(tts_out[0]).unwrap();
    assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 2);
    assert_eq!(
        u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
        48_000
    );
    assert_eq!(&bytes[44..], expected.as_slice());

    let _ = agent.stop().await;
}

// --- LID clip alignment with the STT stream (plan 2026-11-15-lid-spanish-clip-greek) ---

/// Returns `en`; with `out_of_set_first`, the first call is a restricted-mode decision `ja`
/// (raw `ja`, beyond the gap) so the agent runs the end-of-utterance second pass.
struct CountingLid(Arc<AtomicU64>, bool);

#[async_trait]
impl node_webrtc_rust_speech::pipeline::LanguageIdProvider for CountingLid {
    async fn identify(
        &self,
        _pcm: Bytes,
        _sample_rate: u32,
    ) -> SpeechResult<Option<node_webrtc_rust_speech::pipeline::LanguageIdResult>> {
        let call = self.0.fetch_add(1, Ordering::SeqCst);
        if self.1 {
            let code = if call == 0 { "ja" } else { "es" };
            return Ok(Some(node_webrtc_rust_speech::pipeline::LanguageIdResult {
                language: code.into(),
                raw_language: Some(code.into()),
                gap: Some(5.0),
                allowed_logits: None,
            }));
        }
        Ok(Some(
            node_webrtc_rust_speech::pipeline::LanguageIdResult::new("en"),
        ))
    }
}

struct LidStt(Arc<AtomicU64>, bool);

impl VendorFactory for LidStt {
    fn create_stt(&self, _config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(SilentStt))
    }
    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
    fn create_language_id(
        &self,
        _config: &node_webrtc_rust_speech::config::LanguageIdConfig,
    ) -> SpeechResult<Option<Box<dyn node_webrtc_rust_speech::pipeline::LanguageIdProvider>>> {
        Ok(Some(Box::new(CountingLid(Arc::clone(&self.0), self.1))))
    }
}

async fn lid_tap_agent(
    dir: &'static Path,
    timing: Option<node_webrtc_rust_speech::config::LanguageIdTiming>,
    min_speech_ms: u32,
    lid_calls: Arc<AtomicU64>,
) -> Arc<VoiceAgent> {
    lid_tap_agent_with(dir, timing, min_speech_ms, lid_calls, None, None).await
}

/// `allowlist` set: the mock LID decides `ja` (out of set) first, then `es`.
async fn lid_tap_agent_with(
    dir: &'static Path,
    timing: Option<node_webrtc_rust_speech::config::LanguageIdTiming>,
    min_speech_ms: u32,
    lid_calls: Arc<AtomicU64>,
    allowlist: Option<Vec<String>>,
    lid_max_clip_ms: Option<u32>,
) -> Arc<VoiceAgent> {
    let factory = Arc::new(LidStt(lid_calls, allowlist.is_some()));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::clone(&factory) as Arc<dyn VendorFactory>,
    );
    registry.register_stt(
        SttVendor::LocalSherpa,
        Arc::clone(&factory) as Arc<dyn VendorFactory>,
    );
    registry.register_tts(TtsVendor::Mock, factory);
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
        language_id: Some(node_webrtc_rust_speech::config::LanguageIdConfig {
            enabled: Some(true),
            model_path: Some("/fake/lid-model".into()),
            allowlist,
            allowlist_max_gap: None,
            allowlist_min_margin: None,
            report_out_of_set: None,
            min_speech_ms: Some(min_speech_ms),
            continuous: None,
            lid_max_clip_ms,
            lid_gate_max_wait_ms: None,
            timing,
            tts_exclusion: Some(true),
        }),
        vad,
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.set_audio_tap_dir_for_test(dir).await;
    agent.start(None).await.unwrap();
    agent
}

/// Quiet lead-in, then `loud` voiced frames whose level differs per frame (so any frame
/// offset between two clips shows), then silence until the utterance closes.
async fn drive_ramp_utterance(agent: &VoiceAgent, lead: usize, loud: usize) {
    for _ in 0..lead {
        agent
            .process_inbound_pcm(stereo_frame(0), 20)
            .await
            .unwrap();
    }
    for i in 0..loud {
        let level = 6000 + 37 * i as i16;
        agent
            .process_inbound_pcm(stereo_frame(level), 20)
            .await
            .unwrap();
    }
    for _ in 0..80 {
        agent
            .process_inbound_pcm(stereo_frame(0), 20)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn wav_data(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()[44..].to_vec()
}

async fn assert_lid_clip_starts_with_stt_stream(
    timing: Option<node_webrtc_rust_speech::config::LanguageIdTiming>,
    label: &str,
) {
    let dir = unique_dir(label);
    let calls = Arc::new(AtomicU64::new(0));
    let agent = lid_tap_agent(dir, timing, 300, Arc::clone(&calls)).await;
    drive_ramp_utterance(&agent, 15, 60).await;
    let files = wait_files(dir, |f| {
        f.iter().any(|p| name_of(p).starts_with("lid-in-"))
            && f.iter().any(|p| name_of(p).starts_with("stt-in-"))
    })
    .await;
    let lid = files
        .iter()
        .find(|p| name_of(p).starts_with("lid-in-") && name_of(p).ends_with("-p1.wav"))
        .unwrap_or_else(|| panic!("no lid-in: {files:?}"));
    let key = name_of(lid)
        .split("-utt")
        .nth(1)
        .unwrap()
        .trim_end_matches("-p1.wav")
        .to_string();
    let stt = files
        .iter()
        .find(|p| {
            name_of(p).starts_with("stt-in-") && name_of(p).ends_with(&format!("-utt{key}.wav"))
        })
        .unwrap_or_else(|| panic!("no stt-in for utt{key}: {files:?}"));
    let (lid, stt) = (wav_data(lid), wav_data(stt));
    let n = lid.len().min(stt.len());
    assert!(
        n >= 300 * 32,
        "clips too short: lid {} stt {}",
        lid.len(),
        stt.len()
    );
    // Offset 0: the LID clip is the head of the STT stream, sample for sample.
    if lid[..n] != stt[..n] {
        let first_diff = (0..n).find(|&k| lid[k] != stt[k]).unwrap();
        let nz = |b: &[u8]| b.chunks(2).position(|c| c != [0, 0]).map(|p| p * 2);
        panic!(
            "lid-in is not the head of stt-in ({label}): lid {} B, stt {} B, first diff at byte {first_diff}, first non-zero lid {:?} stt {:?}",
            lid.len(),
            stt.len(),
            nz(&lid),
            nz(&stt)
        );
    }
    assert!(calls.load(Ordering::SeqCst) >= 1);
    let _ = agent.stop().await;
}

#[tokio::test]
async fn lid_clip_starts_on_the_stt_streams_first_sample_early_timing() {
    assert_lid_clip_starts_with_stt_stream(
        Some(node_webrtc_rust_speech::config::LanguageIdTiming::Early),
        "lid-early",
    )
    .await;
}

#[tokio::test]
async fn lid_clip_starts_on_the_stt_streams_first_sample_end_of_utterance() {
    assert_lid_clip_starts_with_stt_stream(None, "lid-eou").await;
}

#[tokio::test]
async fn lid_second_pass_clip_keeps_the_head_and_is_capped_at_max_clip() {
    // Early timing, allowlist {en, es}: the early identify decides `ja` (out of set beyond the
    // gap), so the end-of-utterance second pass runs. Its clip (p2) starts on the same first
    // sample as the early clip (p1) and the STT stream, and is cut at lidMaxClipMs (the tail
    // goes, never the head).
    const MAX_CLIP_MS: u32 = 1000;
    let dir = unique_dir("lid-p2-cap");
    let calls = Arc::new(AtomicU64::new(0));
    let agent = lid_tap_agent_with(
        dir,
        Some(node_webrtc_rust_speech::config::LanguageIdTiming::Early),
        300,
        Arc::clone(&calls),
        Some(vec!["en".into(), "es".into()]),
        Some(MAX_CLIP_MS),
    )
    .await;
    // 15 quiet + 70 voiced frames: well over 1000 ms of buffered audio at utterance end.
    drive_ramp_utterance(&agent, 15, 70).await;
    let files = wait_files(dir, |f| {
        f.iter().any(|p| name_of(p).ends_with("-p2.wav"))
            && f.iter().any(|p| name_of(p).starts_with("stt-in-"))
    })
    .await;
    let find = |suffix: &str| {
        files
            .iter()
            .find(|p| name_of(p).starts_with("lid-in-") && name_of(p).ends_with(suffix))
            .unwrap_or_else(|| panic!("no lid-in {suffix}: {files:?}"))
    };
    let (p1, p2) = (wav_data(find("-p1.wav")), wav_data(find("-p2.wav")));
    let stt = files
        .iter()
        .find(|p| name_of(p).starts_with("stt-in-"))
        .map(|p| wav_data(p))
        .unwrap();
    let max_bytes = (16_000 * MAX_CLIP_MS as usize / 1000) * 2;
    assert_eq!(p2.len(), max_bytes, "p2 capped at lidMaxClipMs");
    assert!(p1.len() < p2.len(), "p1 {} B, p2 {} B", p1.len(), p2.len());
    assert_eq!(
        &p2[..p1.len()],
        &p1[..],
        "p2 keeps the early clip as its head"
    );
    let n = p2.len().min(stt.len());
    assert_eq!(&p2[..n], &stt[..n], "p2 is the head of the STT stream");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "early + one second pass");
    let _ = agent.stop().await;
}

#[tokio::test]
async fn lid_min_speech_counts_voiced_audio_only_with_pre_roll_seed() {
    // minSpeechMs 600 with a 15-frame quiet lead-in: the seed adds pre-roll silence and pad to
    // the buffer, but only voiced frames count. 25 voiced frames (500 ms) → no identify.
    let dir = unique_dir("lid-min-short");
    let calls = Arc::new(AtomicU64::new(0));
    let agent = lid_tap_agent(
        dir,
        Some(node_webrtc_rust_speech::config::LanguageIdTiming::Early),
        600,
        Arc::clone(&calls),
    )
    .await;
    drive_ramp_utterance(&agent, 15, 25).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "500 ms voiced < minSpeechMs 600"
    );
    let _ = agent.stop().await;

    // 40 voiced frames (800 ms) → one identify.
    let dir = unique_dir("lid-min-long");
    let calls = Arc::new(AtomicU64::new(0));
    let agent = lid_tap_agent(
        dir,
        Some(node_webrtc_rust_speech::config::LanguageIdTiming::Early),
        600,
        Arc::clone(&calls),
    )
    .await;
    drive_ramp_utterance(&agent, 15, 40).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "800 ms voiced >= minSpeechMs 600"
    );
    let _ = agent.stop().await;
}
