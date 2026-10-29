//! The speaker gate inside a real `VoiceAgent` (Sherpa STT, TitaNet embedder, default gate
//! settings), fed the speaker-fixture S3 test conversations (main caller -20 dBFS, quieter
//! bystanders and babble) in 20 ms frames. No WebRTC.
//!
//! Gated on `SHERPA_SPEAKER_MODEL`, `SHERPA_STT_MODEL_PATH` and `SPEAKER_FIXTURES_DIR` (a
//! `speaker_fixtures` output dir: `manifest.json` plus the scene wavs, directly or under
//! `scenes/`); skips with a printed reason when unset. Run with `--release --nocapture`.
//!
//! Metrics follow the bench (Part 2): per level, over the manifest's turns (main segments
//! without `enroll`, and bystander segments), a turn is matched to the agent utterance that
//! starts inside it, and its verdict is the `speaker` of that utterance's final.
//! (b) = main turns attributed `caller`, (c) = bystander turns attributed `other`,
//! (d) = bystander turns attributed `caller`. A turn with no matching final counts as a miss.
//!
//! Asserts at the bystander level -15 dB (bystanders 15 dB below the caller): (b) and (c) are at
//! least the bench value (run 5: 100 % for both VADs) minus 5 points. All levels are printed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SpeakerGateConfig, SttConfig, SttVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;
use tokio::time::{sleep, Duration};

/// Bench value (run 5, main -20 dBFS, bystander -15 dB, raw, AGC off) for (b) and (c), both VADs.
const BENCH_B_AT_M15: f64 = 100.0;
const BENCH_C_AT_M15: f64 = 100.0;
const MARGIN_POINTS: f64 = 5.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Main,
    Bystander,
}

struct Turn {
    role: Role,
    start_ms: u64,
    end_ms: u64,
}

struct Scene {
    id: String,
    level_db: i64,
    wav48: PathBuf,
    turns: Vec<Turn>,
}

/// 48 kHz stereo s16le bytes of a fixture wav.
fn read_wav_48k_stereo(path: &Path) -> Vec<u8> {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(&data[0..4], b"RIFF");
    let mut pos = 12;
    let (mut channels, mut rate) = (2_u16, 48_000_u32);
    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let len = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if id == b"fmt " {
            channels = u16::from_le_bytes(data[body + 2..body + 4].try_into().unwrap());
            rate = u32::from_le_bytes(data[body + 4..body + 8].try_into().unwrap());
        } else if id == b"data" {
            assert_eq!((channels, rate), (2, 48_000), "{}", path.display());
            return data[body..(body + len).min(data.len())].to_vec();
        }
        pos = body + len + (len & 1);
    }
    panic!("no data chunk in {}", path.display());
}

fn find_wav(dir: &Path, wav48: &str) -> PathBuf {
    let name = Path::new(wav48).file_name().unwrap();
    for candidate in [
        dir.join(wav48),
        dir.join(name),
        dir.join("scenes").join(name),
    ] {
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("scene wav {wav48} not found under {}", dir.display());
}

/// S3 test-set conversations at the default caller level (about -20 dBFS).
fn load_scenes(dir: &Path) -> Vec<Scene> {
    let text = std::fs::read_to_string(dir.join("manifest.json")).expect("manifest.json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("manifest json");
    let mut scenes = Vec::new();
    for s in json["scenes"].as_array().expect("scenes") {
        let id = s["id"].as_str().unwrap().to_string();
        let effective = s["effective_main_db"].as_f64().unwrap_or(0.0);
        if !(id.starts_with("s3_test")
            && s["kind"] == "conversation"
            && (-21.0..=-19.0).contains(&effective))
        {
            continue;
        }
        let mut turns = Vec::new();
        for seg in s["segments"].as_array().unwrap() {
            let role = match seg["role"].as_str().unwrap() {
                "main" if seg["enroll"].as_bool() != Some(true) => Role::Main,
                "bystander" => Role::Bystander,
                _ => continue,
            };
            turns.push(Turn {
                role,
                start_ms: seg["start_ms"].as_u64().unwrap(),
                end_ms: seg["end_ms"].as_u64().unwrap(),
            });
        }
        scenes.push(Scene {
            level_db: s["level_db"].as_i64().unwrap(),
            wav48: find_wav(dir, s["wav48"].as_str().unwrap()),
            id,
            turns,
        });
    }
    scenes.sort_by_key(|s| std::cmp::Reverse(s.level_db));
    scenes
}

#[derive(Default, Clone, Copy)]
struct Stats {
    main: usize,
    main_caller: usize,
    by: usize,
    by_other: usize,
    by_caller: usize,
    missed_main: usize,
    missed_by: usize,
    utterances: usize,
}

fn pct(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        100.0 * n as f64 / d as f64
    }
}

struct Utt {
    start_ms: u64,
    speaker: Option<String>,
    has_final: bool,
}

async fn run_scene(scene: &Scene, provider: &str, stt_model: &str, speaker_model: &str) -> Stats {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::LocalSherpa, Arc::new(SherpaFactory));
    let vad = VadConfig {
        provider: provider.to_string(),
        ..VadConfig::default()
    };
    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
            provider: SttVendor::LocalSherpa,
            model: None,
            model_path: Some(stt_model.to_string()),
            language: Some("en".into()),
            api_key: None,
            endpoint: None,
            transport: None,
        }),
        tts: None,
        vad,
        speaker_gate: SpeakerGateConfig {
            enabled: true,
            model_path: Some(speaker_model.to_string()),
            ..SpeakerGateConfig::default()
        },
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).expect("agent");
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    // Media clock (ms of audio fed) at the moment each event arrives.
    let fed = Arc::new(AtomicU64::new(0));
    let log: Arc<Mutex<Vec<(u64, SpeechEvent)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut rx = agent.subscribe_events();
    let (fed_c, log_c) = (Arc::clone(&fed), Arc::clone(&log));
    let collector = tokio::spawn(async move {
        while let Ok(e) = rx.recv().await {
            log_c
                .lock()
                .unwrap()
                .push((fed_c.load(Ordering::SeqCst), e));
        }
    });

    let pcm = read_wav_48k_stereo(&scene.wav48);
    let silent = vec![0_u8; 3840];
    let (speech_frames, _) = pcm.as_chunks::<3840>();
    let frames = speech_frames
        .iter()
        .map(|f| &f[..])
        .chain(std::iter::repeat_n(&silent[..], 250));
    for frame in frames {
        agent
            .process_inbound_pcm(Bytes::copy_from_slice(frame), 20)
            .await
            .unwrap();
        fed.fetch_add(20, Ordering::SeqCst);
        sleep(Duration::from_millis(1)).await;
    }
    assert!(agent.wait_stt_idle(Duration::from_secs(120)).await);
    sleep(Duration::from_millis(500)).await;
    agent.stop().await.unwrap();
    collector.abort();

    let events = log.lock().unwrap().clone();
    // One entry per utterance id: start time from user_speaking_start, speaker from the final.
    let mut utts: Vec<(String, Utt)> = Vec::new();
    for (t, e) in &events {
        let Some(id) = e.utterance_id.clone() else {
            continue;
        };
        if !utts.iter().any(|(i, _)| *i == id) && e.kind == SpeechEventKind::UserSpeakingStart {
            utts.push((
                id.clone(),
                Utt {
                    start_ms: *t,
                    speaker: None,
                    has_final: false,
                },
            ));
        }
        if e.kind == SpeechEventKind::UserSpeechFinal && e.replay != Some(true) {
            if let Some((_, u)) = utts.iter_mut().find(|(i, _)| *i == id) {
                u.has_final = true;
                u.speaker = e.speaker.clone();
            }
        }
    }

    let mut st = Stats {
        utterances: utts.len(),
        ..Stats::default()
    };
    for turn in &scene.turns {
        // First utterance starting inside the turn (onset slack for the VAD pre-roll).
        let hit = utts
            .iter()
            .map(|(_, u)| u)
            .filter(|u| u.start_ms + 500 >= turn.start_ms && u.start_ms <= turn.end_ms)
            .min_by_key(|u| u.start_ms.abs_diff(turn.start_ms));
        let speaker = hit
            .filter(|u| u.has_final)
            .and_then(|u| u.speaker.as_deref());
        match turn.role {
            Role::Main => {
                st.main += 1;
                st.main_caller += usize::from(speaker == Some("caller"));
                st.missed_main += usize::from(hit.is_none());
            }
            Role::Bystander => {
                st.by += 1;
                st.by_other += usize::from(speaker == Some("other"));
                st.by_caller += usize::from(speaker == Some("caller"));
                st.missed_by += usize::from(hit.is_none());
            }
        }
    }
    st
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn speaker_gate_in_a_real_agent_on_the_s3_conversations() {
    let (Ok(speaker_model), Ok(stt_model), Ok(dir)) = (
        std::env::var("SHERPA_SPEAKER_MODEL"),
        std::env::var("SHERPA_STT_MODEL_PATH"),
        std::env::var("SPEAKER_FIXTURES_DIR"),
    ) else {
        eprintln!("skip: set SHERPA_SPEAKER_MODEL, SHERPA_STT_MODEL_PATH, SPEAKER_FIXTURES_DIR");
        return;
    };
    let scenes = load_scenes(Path::new(&dir));
    assert!(!scenes.is_empty(), "no S3 test conversations in {dir}");

    let mut failures = Vec::new();
    for provider in ["energy", "adaptive"] {
        println!("\nVAD {provider}: level | utt | main turns (missed) | (b) main->caller % | bystander turns (missed) | (c) other % | (d) caller %");
        for scene in &scenes {
            let s = run_scene(scene, provider, &stt_model, &speaker_model).await;
            let (b, c, d) = (
                pct(s.main_caller, s.main),
                pct(s.by_other, s.by),
                pct(s.by_caller, s.by),
            );
            println!(
                "| {} dB ({}) | {} | {} ({}) | {:.1} | {} ({}) | {:.1} | {:.1} |",
                scene.level_db,
                scene.id,
                s.utterances,
                s.main,
                s.missed_main,
                b,
                s.by,
                s.missed_by,
                c,
                d
            );
            if scene.level_db == -15 {
                if b < BENCH_B_AT_M15 - MARGIN_POINTS {
                    failures.push(format!(
                        "{provider} -15 dB: (b) {b:.1} < {}",
                        BENCH_B_AT_M15 - MARGIN_POINTS
                    ));
                }
                if c < BENCH_C_AT_M15 - MARGIN_POINTS {
                    failures.push(format!(
                        "{provider} -15 dB: (c) {c:.1} < {}",
                        BENCH_C_AT_M15 - MARGIN_POINTS
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
