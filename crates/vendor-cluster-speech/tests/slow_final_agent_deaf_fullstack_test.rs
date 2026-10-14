//! Full-stack repro: a VoiceAgent must keep processing inbound audio after a slow cluster-STT
//! final, and while the next per-utterance Transcribe stream is slow to open.
//!
//! Staging incident (45-conversation load, runner nwr 0.10.0): sessions went silent after one
//! turn whose final arrived ~3.3 s late. The native inbound loop awaits
//! `agent.process_inbound_pcm(pcm, 20)` once per 20 ms frame, sequentially, so any await inside
//! it that does not return stops VAD and every later speech event.
//!
//! The harness mirrors that loop: one task, frame n due at `anchor + n * 20 ms`, never two
//! concurrent calls. Per-utterance stream mode is switched on through the environment
//! (`CLUSTER_STT_STREAM_PER_UTTERANCE`, read once per process, so this file is its own binary).

mod mock_server;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use mock_server::{spawn_mock_speech, MockSpeechState};
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_cluster_speech::ClusterSherpaFactory;
use tokio::time::{sleep, sleep_until, Instant as TokioInstant};

const FRAME_MS: u64 = 20;
const TURN_PERIOD_MS: u64 = 14_000;
const SPEECH_MS: u64 = 2_500;
const MAX_CALL_MS: u128 = 1_000;
/// Longest a single inbound call may run before the script gives up and reports it as hung.
const HUNG_CALL_BOUND: Duration = Duration::from_secs(20);

fn loud_stereo_frame() -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
    }
    pcm
}

fn silent_stereo_frame() -> Vec<u8> {
    vec![0_u8; 3840]
}

fn set_per_utterance_env() {
    // `SPEECH_STT_OPEN_WAIT_MAX_MS` must stay unset: the 120 s default is part of the repro.
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var("CLUSTER_STT_STREAM_PER_UTTERANCE", "1");
        std::env::remove_var("SPEECH_STT_OPEN_WAIT_MAX_MS");
    }
}

/// What happened in one turn window (`[turn * period, (turn + 1) * period)` since the anchor).
#[derive(Debug, Default, Clone)]
struct TurnReport {
    speaking_start: bool,
    speech_final: bool,
    final_text: Option<String>,
    /// Longest `process_inbound_pcm` call that started in this window.
    max_call_ms: u128,
    /// `transcribe_streams` on the mock when the turn ended (= next turn started).
    streams_at_end: usize,
}

struct RunReport {
    turns: Vec<TurnReport>,
    /// `(ms since anchor, call duration ms)` of every call over 500 ms.
    slow_calls: Vec<(u128, u128)>,
}

/// Run `turns` scripted turns against a VoiceAgent wired to the mock speech service.
/// `on_turn_start(turn, &state)` runs on the inbound task right before the first frame of a turn.
async fn run_script(
    turns: usize,
    state: MockSpeechState,
    speech_ms_for_turn: impl Fn(usize) -> u64 + Send + 'static,
    on_turn_start: impl Fn(usize, &MockSpeechState) + Send + 'static,
) -> RunReport {
    set_per_utterance_env();
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();

    let mut registry = VendorRegistry::new();
    let factory = Arc::new(ClusterSherpaFactory);
    registry.register_stt(SttVendor::ClusterSherpa, factory.clone());
    registry.register_tts(TtsVendor::ClusterSherpa, factory);

    let mut vad = VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 300,
        speech_pad_ms: 1500,
        gate_stt: true,
        stt_gate_hold_ms: 1500,
        ..VadConfig::default()
    };
    vad.barge_in.enabled = false;

    let config = VoiceAgentConfig {
        stt: Some(node_webrtc_rust_speech::SttConfig {
            provider: SttVendor::ClusterSherpa,
            model: None,
            model_path: Some("/models/sherpa/stt/en".into()),
            language: Some("en".into()),
            api_key: Some("test-token".into()),
            endpoint: Some(url.clone()),
        }),
        tts: Some(TtsConfig {
            provider: TtsVendor::ClusterSherpa,
            model: None,
            model_path: Some("/models/sherpa/tts/en".into()),
            voice: Some("amy".into()),
            api_key: Some("test-token".into()),
            endpoint: Some(url.clone()),
        }),
        vad,
        ..Default::default()
    };

    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    sleep(Duration::from_millis(80)).await;

    let anchor = Instant::now();
    let events: Arc<Mutex<Vec<(u128, SpeechEventKind, Option<String>)>>> =
        Arc::new(Mutex::new(Vec::new()));

    // Event consumer: records every event and replies like the echo agent.
    let mut rx = agent.subscribe_events();
    let consumer_agent = Arc::clone(&agent);
    let consumer_events = Arc::clone(&events);
    let consumer = tokio::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            consumer_events.lock().unwrap().push((
                anchor.elapsed().as_millis(),
                event.kind.clone(),
                event.text.clone(),
            ));
            if event.kind == SpeechEventKind::UserSpeechFinal {
                let text = event
                    .text
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| "okay".to_string());
                let _ = consumer_agent
                    .send_text_to_tts_with_options(
                        &text,
                        SendTextToTtsOptions {
                            non_blocking: true,
                            ..SendTextToTtsOptions::default()
                        },
                    )
                    .await;
            }
        }
    });

    // Inbound task: exactly like the native loop (sequential awaits, paced from an anchor).
    let loud = Bytes::from(loud_stereo_frame());
    let silent = Bytes::from(silent_stereo_frame());
    let frames_per_turn = (TURN_PERIOD_MS / FRAME_MS) as usize;
    let inbound_agent = Arc::clone(&agent);
    let inbound_state = state.clone();
    let inbound = tokio::spawn(async move {
        let tokio_anchor = TokioInstant::from_std(anchor);
        let mut per_turn_max = vec![0_u128; turns];
        let mut streams_at_end = vec![0_usize; turns];
        let mut slow_calls = Vec::new();
        for n in 0..turns * frames_per_turn {
            let turn = n / frames_per_turn;
            let in_turn = n % frames_per_turn;
            if in_turn == 0 {
                if turn > 0 {
                    streams_at_end[turn - 1] =
                        inbound_state.transcribe_streams.load(Ordering::SeqCst);
                }
                on_turn_start(turn, &inbound_state);
            }
            sleep_until(tokio_anchor + Duration::from_millis(n as u64 * FRAME_MS)).await;
            let speech_frames = (speech_ms_for_turn(turn) / FRAME_MS) as usize;
            let frame = if in_turn < speech_frames {
                loud.clone()
            } else {
                silent.clone()
            };
            let started = Instant::now();
            // A call that never returns is the failure under test: bound it so the test ends.
            let call = tokio::time::timeout(
                HUNG_CALL_BOUND,
                inbound_agent.process_inbound_pcm(frame, FRAME_MS as u32),
            )
            .await;
            let took = started.elapsed().as_millis();
            let Ok(result) = call else {
                eprintln!(
                    "process_inbound_pcm call did not return within {} ms (turn {}, frame {})",
                    HUNG_CALL_BOUND.as_millis(),
                    turn + 1,
                    in_turn
                );
                per_turn_max[turn] = per_turn_max[turn].max(took);
                slow_calls.push((anchor.elapsed().as_millis() - took, took));
                break;
            };
            result.unwrap();
            per_turn_max[turn] = per_turn_max[turn].max(took);
            if took > 500 {
                slow_calls.push((anchor.elapsed().as_millis() - took, took));
            }
        }
        let last = streams_at_end.len() - 1;
        if streams_at_end[last] == 0 {
            streams_at_end[last] = inbound_state.transcribe_streams.load(Ordering::SeqCst);
        }
        (per_turn_max, streams_at_end, slow_calls)
    });

    let (per_turn_max, streams_at_end, slow_calls) = inbound.await.unwrap();
    sleep(Duration::from_millis(300)).await;
    consumer.abort();
    // `stop()` can hang behind a stuck stream open; the test must still end.
    if tokio::time::timeout(Duration::from_secs(5), agent.stop())
        .await
        .is_err()
    {
        eprintln!("agent.stop() did not return within 5 s");
    }
    let _ = shutdown.send(());

    let mut reports = vec![TurnReport::default(); turns];
    for (i, report) in reports.iter_mut().enumerate() {
        report.max_call_ms = per_turn_max[i];
        report.streams_at_end = streams_at_end[i];
    }
    for (at_ms, kind, text) in events.lock().unwrap().iter() {
        let turn = (*at_ms as u64 / TURN_PERIOD_MS) as usize;
        if turn >= turns {
            continue;
        }
        match kind {
            SpeechEventKind::UserSpeakingStart => reports[turn].speaking_start = true,
            SpeechEventKind::UserSpeechFinal => {
                reports[turn].speech_final = true;
                reports[turn].final_text = text.clone();
            }
            _ => {}
        }
    }
    RunReport {
        turns: reports,
        slow_calls,
    }
}

fn describe(report: &RunReport) -> String {
    let mut out = String::new();
    for (i, t) in report.turns.iter().enumerate() {
        out.push_str(&format!(
            "turn {}: speaking_start={} final={} ({:?}) max_call_ms={} streams_at_end={}\n",
            i + 1,
            t.speaking_start,
            t.speech_final,
            t.final_text,
            t.max_call_ms,
            t.streams_at_end
        ));
    }
    out.push_str(&format!(
        "slow calls (start_ms, took_ms): {:?}\n",
        report.slow_calls
    ));
    out
}

/// Turns 1-2 default, turn 3 has a 3.3 s late final, turns 4-6 default again. Every turn must
/// start speaking, open a fresh stream, produce a final, and never stall the inbound call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_keeps_hearing_after_a_slow_final() {
    set_per_utterance_env();
    let state = MockSpeechState::default();
    let report = run_script(
        6,
        state,
        |_| SPEECH_MS,
        |turn, state| match turn {
            2 => state.final_delay_ms.store(3_300, Ordering::SeqCst),
            3 => state.final_delay_ms.store(0, Ordering::SeqCst),
            _ => {}
        },
    )
    .await;
    assert_every_turn_heard(&report);
}

/// Same as the slow-final test, with `Finalized` also arriving 3 s after the final.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_keeps_hearing_after_a_slow_final_and_slow_finalized() {
    set_per_utterance_env();
    let state = MockSpeechState::default();
    let report = run_script(
        6,
        state,
        |_| SPEECH_MS,
        |turn, state| match turn {
            2 => {
                state.final_delay_ms.store(3_300, Ordering::SeqCst);
                state.finalized_delay_ms.store(3_000, Ordering::SeqCst);
            }
            3 => {
                state.final_delay_ms.store(0, Ordering::SeqCst);
                state.finalized_delay_ms.store(0, Ordering::SeqCst);
            }
            _ => {}
        },
    )
    .await;
    assert_every_turn_heard(&report);
}

/// Same as the slow-final test, with turn 3 carrying only 1.5 s of speech (less audio, as on
/// the staging stream that came up ~1 s short).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_keeps_hearing_after_a_slow_final_on_a_short_turn() {
    set_per_utterance_env();
    let state = MockSpeechState::default();
    let report = run_script(
        6,
        state,
        |turn| if turn == 2 { 1_500 } else { SPEECH_MS },
        |turn, state| match turn {
            2 => state.final_delay_ms.store(3_300, Ordering::SeqCst),
            3 => state.final_delay_ms.store(0, Ordering::SeqCst),
            _ => {}
        },
    )
    .await;
    assert_every_turn_heard(&report);
}

fn assert_every_turn_heard(report: &RunReport) {
    let summary = describe(report);
    eprintln!("{summary}");

    for (i, t) in report.turns.iter().enumerate() {
        assert!(
            t.speaking_start,
            "turn {}: no user_speaking_start\n{summary}",
            i + 1
        );
        assert!(
            t.speech_final,
            "turn {}: no user_speech_final\n{summary}",
            i + 1
        );
        assert!(
            t.max_call_ms < MAX_CALL_MS,
            "turn {}: process_inbound_pcm took {} ms\n{summary}",
            i + 1,
            t.max_call_ms
        );
        let prev = if i == 0 {
            0
        } else {
            report.turns[i - 1].streams_at_end
        };
        assert!(
            t.streams_at_end > prev,
            "turn {}: no new transcribe stream opened on the mock ({} -> {})\n{summary}",
            i + 1,
            prev,
            t.streams_at_end
        );
    }
}

/// Turns 1-3 default. The Transcribe open for turn 4 never returns its response headers. The
/// inbound loop must keep processing frames and turn 5 must still start speaking.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_keeps_hearing_while_the_next_stream_open_hangs() {
    set_per_utterance_env();
    let state = MockSpeechState::default();
    let report = run_script(
        5,
        state,
        |_| SPEECH_MS,
        |turn, state| {
            if turn == 3 {
                let so_far = state.transcribe_streams.load(Ordering::SeqCst);
                state
                    .open_header_delay_once_after
                    .store(so_far, Ordering::SeqCst);
                state.open_header_delay_ms.store(600_000, Ordering::SeqCst);
            }
        },
    )
    .await;
    let summary = describe(&report);
    eprintln!("{summary}");

    for i in [3_usize, 4] {
        let t = &report.turns[i];
        assert!(
            t.speaking_start,
            "turn {}: no user_speaking_start\n{summary}",
            i + 1
        );
        assert!(
            t.max_call_ms < MAX_CALL_MS,
            "turn {}: process_inbound_pcm took {} ms\n{summary}",
            i + 1,
            t.max_call_ms
        );
    }
}
