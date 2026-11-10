//! Deferred finalize on the cluster STT client: `request_finalize` returns at once and the
//! answer arrives as exactly one `SttOutput::FinalizeDone` per request, in request order, on
//! every path (answered, delayed, refused open, stream ended, idle).

mod mock_server;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use bytes::Bytes;
use mock_server::{spawn_mock_speech, MockSpeechState};
use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pipeline::{FinalizeMode, SttOutput, SttProvider, SttTranscript};
use node_webrtc_rust_vendor_cluster_speech::{ClusterSherpaStt, ClusterSttOptions};
use tokio::time::{sleep, timeout};

const CHUNK_BYTES: usize = 640;
/// 16 kHz mono s16le: 32 bytes per millisecond.
const BYTES_PER_MS: usize = 32;

fn stt_cfg(endpoint: &str) -> SttConfig {
    SttConfig {
        provider: SttVendor::ClusterSherpa,
        model: None,
        model_path: Some("/models/sherpa/stt/en".into()),
        language: Some("en".into()),
        api_key: Some("test-token".into()),
        endpoint: Some(endpoint.to_string()),
        transport: None,
    }
}

/// Loud 440 Hz tone, 16 kHz mono s16le, `ms` long.
fn tone(ms: usize) -> Vec<u8> {
    let samples = ms * BYTES_PER_MS / 2;
    let mut pcm = Vec::with_capacity(samples * 2);
    for n in 0..samples {
        let v = (f64::sin(2.0 * std::f64::consts::PI * 440.0 * n as f64 / 16_000.0)
            * (i16::MAX as f64 / 3.0)) as i16;
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    pcm
}

async fn push_tone(stt: &mut ClusterSherpaStt, ms: usize) -> usize {
    let pcm = tone(ms);
    for chunk in pcm.chunks(CHUNK_BYTES) {
        stt.push_audio(Bytes::copy_from_slice(chunk)).await.unwrap();
    }
    pcm.len()
}

async fn new_stt(url: &str) -> ClusterSherpaStt {
    let mut stt = ClusterSherpaStt::new_with_options(
        &stt_cfg(url),
        ClusterSttOptions {
            stream_per_utterance: true,
        },
    )
    .unwrap();
    stt.start().await.unwrap();
    stt
}

/// Poll `poll_output` every 10 ms until `n` `FinalizeDone` markers were seen or `max_ms` passed.
async fn collect_until_n_finalize_done(
    stt: &mut ClusterSherpaStt,
    n: usize,
    max_ms: u64,
) -> Vec<SttOutput> {
    let mut out = Vec::new();
    let mut markers = 0;
    let deadline = Instant::now() + Duration::from_millis(max_ms);
    while markers < n && Instant::now() < deadline {
        match stt.poll_output().await.unwrap() {
            Some(o) => {
                if o == SttOutput::FinalizeDone {
                    markers += 1;
                }
                out.push(o);
            }
            None => sleep(Duration::from_millis(10)).await,
        }
    }
    out
}

async fn collect_until_finalize_done(stt: &mut ClusterSherpaStt, max_ms: u64) -> Vec<SttOutput> {
    collect_until_n_finalize_done(stt, 1, max_ms).await
}

fn is_final(o: &SttOutput) -> bool {
    matches!(o, SttOutput::Transcript(SttTranscript::Final(_)))
}

fn count_markers(outputs: &[SttOutput]) -> usize {
    outputs
        .iter()
        .filter(|o| **o == SttOutput::FinalizeDone)
        .count()
}

async fn timed_request_finalize(stt: &mut ClusterSherpaStt) -> Duration {
    let t0 = Instant::now();
    timeout(Duration::from_secs(5), stt.request_finalize())
        .await
        .expect("request_finalize bounded")
        .unwrap();
    t0.elapsed()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_finalize_returns_while_final_is_delayed() {
    let state = MockSpeechState::default();
    state.final_delay_ms.store(5_000, Ordering::SeqCst);
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let mut stt = new_stt(&url).await;
    assert_eq!(stt.finalize_mode(), FinalizeMode::Deferred);

    push_tone(&mut stt, 1_000).await;
    let took = timed_request_finalize(&mut stt).await;
    assert!(took < Duration::from_millis(200), "took {took:?}");

    let outputs = collect_until_finalize_done(&mut stt, 8_000).await;
    assert_eq!(
        outputs.last(),
        Some(&SttOutput::FinalizeDone),
        "{outputs:?}"
    );
    assert_eq!(count_markers(&outputs), 1, "{outputs:?}");
    assert!(outputs.iter().any(is_final), "{outputs:?}");
    stt.stop().await.unwrap();
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_finalize_returns_during_refused_open_and_audio_arrives() {
    let state = MockSpeechState::default();
    state.reject_first_opens.store(25, Ordering::SeqCst);
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let mut stt = new_stt(&url).await;

    let pushed = push_tone(&mut stt, 2_000).await;
    let took = timed_request_finalize(&mut stt).await;
    assert!(took < Duration::from_millis(200), "took {took:?}");

    let outputs = collect_until_finalize_done(&mut stt, 15_000).await;
    assert_eq!(
        outputs.last(),
        Some(&SttOutput::FinalizeDone),
        "{outputs:?}"
    );
    assert!(outputs.iter().any(is_final), "{outputs:?}");
    assert_eq!(state.audio_bytes_received.load(Ordering::SeqCst), pushed);
    assert_eq!(state.rejected_opens.load(Ordering::SeqCst), 25);
    stt.stop().await.unwrap();
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_outstanding_finalizes_answer_in_order() {
    let state = MockSpeechState::default();
    state.numbered_finals.store(true, Ordering::SeqCst);
    // Both finalizes are slow, so B and its finalize are requested while A is still pending on
    // the first stream. (The delay is not reset: the mock reads it when it reads the Finalize.)
    state.final_delay_ms.store(1_500, Ordering::SeqCst);
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let mut stt = new_stt(&url).await;

    let a_bytes = push_tone(&mut stt, 1_000).await;
    assert!(timed_request_finalize(&mut stt).await < Duration::from_millis(200));
    let b_bytes = push_tone(&mut stt, 1_000).await;
    assert!(timed_request_finalize(&mut stt).await < Duration::from_millis(200));

    let outputs = collect_until_n_finalize_done(&mut stt, 2, 15_000).await;
    assert_eq!(count_markers(&outputs), 2, "{outputs:?}");
    assert_eq!(
        outputs.last(),
        Some(&SttOutput::FinalizeDone),
        "{outputs:?}"
    );
    let first_marker = outputs
        .iter()
        .position(|o| *o == SttOutput::FinalizeDone)
        .unwrap();
    let finals_before_first = outputs[..first_marker]
        .iter()
        .filter(|o| is_final(o))
        .count();
    let finals_after_first = outputs[first_marker..]
        .iter()
        .filter(|o| is_final(o))
        .count();
    assert_eq!(finals_before_first, 1, "{outputs:?}");
    assert_eq!(finals_after_first, 1, "{outputs:?}");
    assert!(
        outputs.contains(&SttOutput::Transcript(SttTranscript::Final(
            "final-1".into()
        ))),
        "{outputs:?}"
    );
    assert!(
        outputs.contains(&SttOutput::Transcript(SttTranscript::Final(
            "final-2".into()
        ))),
        "{outputs:?}"
    );

    // B and its finalize went to a second stream, A's audio stayed on the first.
    assert_eq!(state.transcribe_streams.load(Ordering::SeqCst), 2);
    assert_eq!(state.finalize_count.load(Ordering::SeqCst), 2);
    let (on_stream_1, on_stream_2) = {
        let log = state.audio_log.lock().unwrap();
        let on_stream = |idx: usize| -> usize {
            log.iter()
                .filter(|(i, _)| *i == idx)
                .map(|(_, b)| b.len())
                .sum()
        };
        (on_stream(1), on_stream(2))
    };
    assert_eq!(on_stream_1, a_bytes);
    assert_eq!(on_stream_2, b_bytes);
    stt.stop().await.unwrap();
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_error_with_pending_finalize_emits_finalize_done() {
    let state = MockSpeechState::default();
    state.drop_stream_on_finalize.store(true, Ordering::SeqCst);
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let mut stt = new_stt(&url).await;

    push_tone(&mut stt, 500).await;
    assert!(timed_request_finalize(&mut stt).await < Duration::from_millis(200));

    // Wait the full window: exactly one marker, and no second one afterwards.
    let outputs = collect_until_n_finalize_done(&mut stt, 2, 3_000).await;
    assert_eq!(count_markers(&outputs), 1, "{outputs:?}");
    assert_eq!(
        outputs.last(),
        Some(&SttOutput::FinalizeDone),
        "{outputs:?}"
    );
    stt.stop().await.unwrap();
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finalize_while_idle_answers_immediately() {
    let state = MockSpeechState::default();
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let mut stt = new_stt(&url).await;

    push_tone(&mut stt, 1_000).await;
    assert!(timed_request_finalize(&mut stt).await < Duration::from_millis(200));
    let outputs = collect_until_finalize_done(&mut stt, 5_000).await;
    assert_eq!(
        outputs.last(),
        Some(&SttOutput::FinalizeDone),
        "{outputs:?}"
    );

    // Per-utterance mode released the stream after Finalized; nothing is open now.
    sleep(Duration::from_millis(300)).await;
    let t0 = Instant::now();
    assert!(timed_request_finalize(&mut stt).await < Duration::from_millis(200));
    let outputs = collect_until_finalize_done(&mut stt, 500).await;
    assert_eq!(outputs, vec![SttOutput::FinalizeDone]);
    assert!(t0.elapsed() < Duration::from_millis(500));
    assert_eq!(state.finalize_count.load(Ordering::SeqCst), 1);
    stt.stop().await.unwrap();
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_finalize_utterance_still_waits_and_poll_transcript_skips_markers() {
    let state = MockSpeechState::default();
    state.final_delay_ms.store(300, Ordering::SeqCst);
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let mut stt = new_stt(&url).await;

    push_tone(&mut stt, 1_000).await;
    let t0 = Instant::now();
    timeout(Duration::from_secs(15), stt.finalize_utterance())
        .await
        .expect("finalize_utterance bounded")
        .unwrap();
    // It waited for the delayed final instead of returning at once.
    assert!(
        t0.elapsed() >= Duration::from_millis(300),
        "{:?}",
        t0.elapsed()
    );

    let mut transcripts = Vec::new();
    while let Some(t) = stt.poll_transcript().await.unwrap() {
        transcripts.push(t);
    }
    assert!(
        transcripts.contains(&SttTranscript::Final("final text".into())),
        "{transcripts:?}"
    );
    stt.stop().await.unwrap();
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_finalize_after_worker_exit_returns_error() {
    // Nothing listens on this port, so the worker's connect fails and the worker ends.
    let mut stt = new_stt("http://127.0.0.1:1").await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while stt.stream_open_pending() {
        assert!(Instant::now() < deadline, "worker did not exit within 5 s");
        sleep(Duration::from_millis(10)).await;
    }

    let t0 = Instant::now();
    let res = timeout(Duration::from_secs(5), stt.request_finalize())
        .await
        .expect("request_finalize bounded");
    assert!(res.is_err(), "{res:?}");
    assert!(
        t0.elapsed() < Duration::from_millis(200),
        "{:?}",
        t0.elapsed()
    );
    stt.stop().await.unwrap();
}

/// Speaker split on a Deferred vendor while the speech pod lags.
/// The agent ends the mixed stream, re-sends U1's PCM and ends it, then seeds U2 with the
/// post-split PCM, all while the mixed stream's finalize is still pending. Live U2 audio follows
/// while U1's finalize is pending on the next stream. U1's final holds only U1's audio, and U2's
/// final holds the seed plus the live audio: nothing sent after a Finalize is left on a stream
/// that closes at its Finalized.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn speaker_split_seed_reaches_u2_final_behind_pending_finalize() {
    let state = MockSpeechState::default();
    state
        .final_reports_audio_bytes
        .store(true, Ordering::SeqCst);
    // Every final lags 1.5 s behind its Finalize (pod behind), so the split's commands queue.
    state.final_delay_ms.store(1_500, Ordering::SeqCst);
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let mut stt = new_stt(&url).await;

    // Mixed stream (caller 4.7 s + bystander 2.8 s), then its end (discarded by the agent).
    let mixed = push_tone(&mut stt, 7_540).await;
    timed_request_finalize(&mut stt).await;
    // U1 re-decode: caller PCM + endpoint tail, then its end.
    let u1 = push_tone(&mut stt, 4_740).await + push_tone(&mut stt, 500).await;
    timed_request_finalize(&mut stt).await;
    // U2 seed: the bystander's 2.8 s already heard.
    let seed = push_tone(&mut stt, 2_800).await;

    // The mixed finalize is answered and the queued items move to the next stream.
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.transcribe_streams.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
        sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(state.transcribe_streams.load(Ordering::SeqCst), 2);
    // Live U2 audio while U1's finalize is pending on that stream.
    let live = push_tone(&mut stt, 1_000).await;
    timed_request_finalize(&mut stt).await;

    let outputs = collect_until_n_finalize_done(&mut stt, 3, 20_000).await;
    assert_eq!(count_markers(&outputs), 3, "{outputs:?}");
    let finals: Vec<String> = outputs
        .iter()
        .filter_map(|o| match o {
            SttOutput::Transcript(SttTranscript::Final(t)) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        finals,
        vec![
            format!("bytes-{mixed}"),
            format!("bytes-{u1}"),
            format!("bytes-{}", seed + live),
        ],
        "{outputs:?}"
    );
    stt.stop().await.unwrap();
    let _ = shutdown.send(());
}
