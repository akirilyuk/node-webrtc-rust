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
//! Asserts, adaptive VAD at the bystander level -15 dB (bystanders 15 dB below the caller): (b)
//! and (c) are at least the bench value (run 5: 100 %) minus 5 points, and (d) is 0. Energy VAD,
//! every level: only (d) == 0 (a bystander is never labelled caller). All levels are printed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SpeakerGateConfig, SttConfig, SttVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;
use tokio::time::{sleep, Duration};

/// Bench value (run 5, main -20 dBFS, bystander -15 dB, raw, AGC off) for (b) and (c).
const BENCH_B_AT_M15: f64 = 100.0;
const BENCH_C_AT_M15: f64 = 100.0;
const MARGIN_POINTS: f64 = 5.0;
/// Shortest scene gap is ~1.4 s; VAD SpeechEnd comes `minSilenceDurationMs` after the turn ends.
const FIXTURE_UTTERANCE_FINALIZE_MS: u32 = 100;

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
    /// Utterances matched by more than one manifest turn (one final spans several turns).
    merged: usize,
    /// Turns whose utterance got no final.
    no_final: usize,
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
    // End-of-turn finalize below the scenes' shortest pause after VAD SpeechEnd: with the
    // default 1500 ms, a next turn that starts within 1.5 s of SpeechEnd continues the same STT
    // utterance, so several manifest turns share one final (one verdict) and the rest get none.
    // The bench scored each turn on its own; this makes every VAD utterance finalize separately.
    let vad = VadConfig {
        provider: provider.to_string(),
        utterance_finalize_timeout_ms: FIXTURE_UTTERANCE_FINALIZE_MS,
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

    // Events are drained after every frame and stamped with the media clock (ms of audio fed),
    // so VAD events (emitted inside `process_inbound_pcm`) get the same stamp on every run.
    let mut rx = agent.subscribe_events();
    let mut events: Vec<(u64, SpeechEvent)> = Vec::new();
    let mut fed = 0_u64;

    // Frames are paced at real time from an anchor. The agent's STT finalize and gate-hold
    // timers run on the wall clock: fed faster than real time (the old 1 ms sleep = 20x), a
    // turn's final had not arrived when the next turn started, so turns merged into one final
    // or lost theirs. The bench scored every turn on its own; real time keeps that.
    let pace = std::env::var("SPEAKER_FIXTURE_SPEEDUP")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1)
        .max(1);
    let pcm = read_wav_48k_stereo(&scene.wav48);
    let silent = vec![0_u8; 3840];
    let (speech_frames, _) = pcm.as_chunks::<3840>();
    let frames = speech_frames
        .iter()
        .map(|f| &f[..])
        .chain(std::iter::repeat_n(&silent[..], 250));
    let anchor = tokio::time::Instant::now();
    for (n, frame) in frames.enumerate() {
        let due = anchor + Duration::from_millis(n as u64 * 20 / u64::from(pace));
        tokio::time::sleep_until(due).await;
        agent
            .process_inbound_pcm(Bytes::copy_from_slice(frame), 20)
            .await
            .unwrap();
        fed += 20;
        while let Ok(e) = rx.try_recv() {
            events.push((fed, e));
        }
    }
    assert!(agent.wait_stt_idle(Duration::from_secs(120)).await);
    sleep(Duration::from_millis(500)).await;
    while let Ok(e) = rx.try_recv() {
        events.push((fed, e));
    }
    agent.stop().await.unwrap();

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
    let verbose = std::env::var("SPEAKER_FIXTURE_VERBOSE").is_ok_and(|v| v == "1");
    let mut hits_per_utt = vec![0_usize; utts.len()];
    for turn in &scene.turns {
        // First utterance starting inside the turn (onset slack for the VAD pre-roll).
        let hit_idx = utts
            .iter()
            .enumerate()
            .filter(|(_, (_, u))| u.start_ms + 500 >= turn.start_ms && u.start_ms <= turn.end_ms)
            .min_by_key(|(_, (_, u))| u.start_ms.abs_diff(turn.start_ms))
            .map(|(i, _)| i);
        if let Some(i) = hit_idx {
            hits_per_utt[i] += 1;
        }
        let hit = hit_idx.map(|i| &utts[i].1);
        st.no_final += usize::from(hit.is_some_and(|u| !u.has_final));
        if verbose {
            println!(
                "  turn {:?} {}-{} -> {}",
                turn.role,
                turn.start_ms,
                turn.end_ms,
                hit_idx.map_or("none".to_string(), |i| format!(
                    "{} start={} final={} speaker={:?}",
                    utts[i].0, utts[i].1.start_ms, utts[i].1.has_final, utts[i].1.speaker
                ))
            );
        }
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
    st.merged = hits_per_utt.iter().filter(|n| **n > 1).count();
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

    // Debug aid: `SPEAKER_FIXTURE_LEVEL=-15` runs one level only.
    let only_level = std::env::var("SPEAKER_FIXTURE_LEVEL")
        .ok()
        .and_then(|v| v.parse::<i64>().ok());
    let mut failures = Vec::new();
    for provider in ["energy", "adaptive"] {
        println!("\nVAD {provider}: level | utt | merged | no final | main turns (missed) | (b) main->caller % | bystander turns (missed) | (c) other % | (d) caller %");
        for scene in &scenes {
            if only_level.is_some_and(|l| l != scene.level_db) {
                continue;
            }
            let s = run_scene(scene, provider, &stt_model, &speaker_model).await;
            let (b, c, d) = (
                pct(s.main_caller, s.main),
                pct(s.by_other, s.by),
                pct(s.by_caller, s.by),
            );
            println!(
                "| {} dB ({}) | {} | {} | {} | {} ({}) | {:.1} | {} ({}) | {:.1} | {:.1} |",
                scene.level_db,
                scene.id,
                s.utterances,
                s.merged,
                s.no_final,
                s.main,
                s.missed_main,
                b,
                s.by,
                s.missed_by,
                c,
                d
            );
            // Safety, every VAD and level: a bystander is never labelled caller.
            if s.by_caller > 0 {
                failures.push(format!("{provider} {} dB: (d) {d:.1} > 0", scene.level_db));
            }
            // The gate is paired with the adaptive VAD. The energy VAD splits turns in babble
            // into short pieces, whose first checkSpeechMs are often too short (unknown) or
            // score as other, so only (d) is asserted for it.
            if provider == "adaptive" && scene.level_db == -15 {
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
