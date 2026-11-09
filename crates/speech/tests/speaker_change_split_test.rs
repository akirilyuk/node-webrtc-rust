//! Speaker-change split (`speakerGate.splitOnSpeakerChange`): the agent splits the live utterance
//! at the gate's confirmed change, re-decodes U1 from its own PCM and continues U2 live.
//!
//! The scripted gate and STT read the audio itself: caller frames have amplitude `A`, the other
//! speaker's frames amplitude `B`. The fake embedder returns the caller's vector for an `A` clip
//! and the other vector for a `B` clip; the fake STT's final text counts the `A` and `B` frames
//! of the stream it decoded (`"a<n> b<m>"`). So U1's text proves which PCM it was decoded from.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SpeakerGateConfig, SpeakerMismatchAction, SttVendor, TtsConfig, TtsVendor, VadConfig,
    VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{
    FinalizeMode, SpeakerEmbedder, SttOutput, SttProvider, SttTranscript, VendorFactory,
};
use node_webrtc_rust_speech::{PcmWriter, SpeechResult, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::sync::{broadcast, Notify};
use tokio::time::{sleep, Duration};

const REF: [f32; 4] = [0.9, 0.1, 0.2, 0.3];
const OTHER: [f32; 4] = [-0.2, 0.9, -0.3, 0.1];
/// Caller amplitude.
const A: i16 = i16::MAX / 3;
/// Other speaker's amplitude (about 6 dB under the caller).
const B: i16 = i16::MAX / 6;

fn stereo_frame(level: i16) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..1920 {
        pcm.extend_from_slice(&level.to_le_bytes());
    }
    pcm
}

/// `'A'`, `'B'` or `'.'` (silence) for one mono s16le frame, by mean absolute level.
fn class_of(frame: &[u8]) -> char {
    let samples: Vec<i64> = frame
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i64::from(i16::from_le_bytes(*b)).abs())
        .collect();
    if samples.is_empty() {
        return '.';
    }
    let mean = samples.iter().sum::<i64>() / samples.len() as i64;
    if mean > 8_000 {
        'A'
    } else if mean > 2_500 {
        'B'
    } else {
        '.'
    }
}

/// One class per 20 ms (640-byte) frame of `pcm`.
fn classes(pcm: &[u8]) -> String {
    pcm.chunks(640).map(class_of).collect()
}

fn count(s: &str, c: char) -> usize {
    s.chars().filter(|x| *x == c).count()
}

// ---- scripted gate (embedder) ---------------------------------------------------------------

struct LevelEmbedder;

#[async_trait]
impl SpeakerEmbedder for LevelEmbedder {
    async fn embed(&self, pcm: Bytes, _rate: u32) -> SpeechResult<Option<Vec<f32>>> {
        let c = classes(&pcm);
        let (a, b) = (count(&c, 'A'), count(&c, 'B'));
        if a == 0 && b == 0 {
            return Ok(None);
        }
        Ok(Some(if a >= b { REF.to_vec() } else { OTHER.to_vec() }))
    }
}

// ---- fake STT -------------------------------------------------------------------------------

#[derive(Default)]
struct Script {
    /// `finalize_utterance` calls so far (1-based index of the current call while it runs).
    finalize_calls: usize,
    /// The finalize call with this index waits for the notify before it answers.
    block_on_call: Option<(usize, Arc<Notify>)>,
    /// The finalize call with this index never produces a final (the actor's wait times out).
    no_final_on_call: Option<usize>,
    /// Finalize wait the provider reports (`finalize_wait_ms`).
    finalize_wait_ms: Option<u64>,
    /// Frame classes of every finalized stream, in finalize order.
    streams: Vec<String>,
    /// The provider ends an utterance like Deepgram or ElevenLabs: `request_finalize` returns at
    /// once and the answer (the final, then `FinalizeDone`) arrives through `poll_output`, in
    /// request order.
    deferred: bool,
    /// Deferred mode: the answer to the finalize call with this index is withheld (and so is every
    /// later one, since answers keep request order) until the index leaves this list.
    held_calls: Vec<usize>,
    /// Frame classes of every abandoned (never decoded) stream, in abandon order.
    abandoned: Vec<String>,
}

struct LevelStt {
    script: Arc<Mutex<Script>>,
    cur: Vec<u8>,
    queue: VecDeque<SttTranscript>,
    /// Deferred mode: finalizes not yet answered, oldest first: (call index, frame classes).
    pending: VecDeque<(usize, String)>,
    /// Deferred mode: transcripts and markers ready for `poll_output`.
    outputs: VecDeque<SttOutput>,
}

#[async_trait]
impl SttProvider for LevelStt {
    fn vendor_name(&self) -> &'static str {
        "scripted-speaker-split"
    }
    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }
    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }
    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        self.cur.extend_from_slice(&pcm);
        Ok(())
    }
    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        Ok(self.queue.pop_front())
    }
    fn finalize_mode(&self) -> FinalizeMode {
        if self.script.lock().unwrap().deferred {
            FinalizeMode::Deferred
        } else {
            FinalizeMode::Inline
        }
    }
    fn discard_queued_transcripts(&mut self) -> usize {
        let before = self.outputs.len();
        self.outputs
            .retain(|o| !matches!(o, SttOutput::Transcript(_)));
        before - self.outputs.len()
    }
    async fn request_finalize(&mut self) -> SpeechResult<()> {
        if !self.script.lock().unwrap().deferred {
            return self.finalize_utterance().await;
        }
        let c = classes(&std::mem::take(&mut self.cur));
        let call = {
            let mut script = self.script.lock().unwrap();
            script.finalize_calls += 1;
            script.streams.push(c.clone());
            script.finalize_calls
        };
        self.pending.push_back((call, c));
        Ok(())
    }
    async fn poll_output(&mut self) -> SpeechResult<Option<SttOutput>> {
        if !self.script.lock().unwrap().deferred {
            return Ok(self.poll_transcript().await?.map(SttOutput::Transcript));
        }
        // Answers keep request order: a held or never-answered front blocks the ones behind it.
        loop {
            if let Some(o) = self.outputs.pop_front() {
                return Ok(Some(o));
            }
            let Some((call, c)) = self.pending.front().cloned() else {
                return Ok(None);
            };
            {
                let script = self.script.lock().unwrap();
                if script.held_calls.contains(&call) || script.no_final_on_call == Some(call) {
                    return Ok(None);
                }
            }
            self.pending.pop_front();
            self.outputs
                .push_back(SttOutput::Transcript(SttTranscript::Final(format!(
                    "a{} b{}",
                    count(&c, 'A'),
                    count(&c, 'B')
                ))));
            self.outputs.push_back(SttOutput::FinalizeDone);
        }
    }
    async fn abandon_utterance(&mut self) -> SpeechResult<bool> {
        let c = classes(&std::mem::take(&mut self.cur));
        self.script.lock().unwrap().abandoned.push(c);
        self.queue.clear();
        Ok(true)
    }
    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let c = classes(&std::mem::take(&mut self.cur));
        let (call, block, silent) = {
            let mut script = self.script.lock().unwrap();
            script.finalize_calls += 1;
            let call = script.finalize_calls;
            script.streams.push(c.clone());
            let block = script
                .block_on_call
                .as_ref()
                .filter(|(n, _)| *n == call)
                .map(|(_, notify)| Arc::clone(notify));
            (call, block, script.no_final_on_call == Some(call))
        };
        let _ = call;
        if let Some(notify) = block {
            notify.notified().await;
        }
        if !silent {
            self.queue.push_back(SttTranscript::Final(format!(
                "a{} b{}",
                count(&c, 'A'),
                count(&c, 'B')
            )));
        }
        Ok(())
    }
    fn finalize_wait_ms(&self) -> u64 {
        self.script
            .lock()
            .unwrap()
            .finalize_wait_ms
            .unwrap_or(10_000)
    }
}

struct SplitFactory {
    script: Arc<Mutex<Script>>,
}

impl VendorFactory for SplitFactory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(LevelStt {
            script: Arc::clone(&self.script),
            cur: Vec::new(),
            queue: VecDeque::new(),
            pending: VecDeque::new(),
            outputs: VecDeque::new(),
        }))
    }
    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
    fn create_speaker_embedder(
        &self,
        _config: &SpeakerGateConfig,
    ) -> SpeechResult<Option<Box<dyn SpeakerEmbedder>>> {
        Ok(Some(Box::new(LevelEmbedder)))
    }
}

// ---- rig ------------------------------------------------------------------------------------

struct Rig {
    agent: Arc<VoiceAgent>,
    script: Arc<Mutex<Script>>,
    events: broadcast::Receiver<SpeechEvent>,
    log: Vec<SpeechEvent>,
}

fn agent_config(split: bool) -> VoiceAgentConfig {
    let mut vad = VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 40,
        gate_stt: true,
        stt_gate_hold_ms: 80,
        ..VadConfig::default()
    };
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = true;
    vad.barge_in.flush_tts = true;
    vad.barge_in.agent_playback_guard_ms = 0;
    vad.barge_in.require_stt_partial = false;
    VoiceAgentConfig {
        stt: Some(node_webrtc_rust_speech::SttConfig {
            provider: SttVendor::LocalSherpa,
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
        speaker_gate: SpeakerGateConfig {
            enabled: true,
            model_path: Some("fake.onnx".into()),
            threshold: 0.459,
            enroll_speech_ms: 400,
            check_speech_ms: 200,
            min_check_speech_ms: 100,
            suppress_short_barge_in: false,
            recheck_on_other: true,
            recheck_max_windows: 10,
            split_on_speaker_change: split,
            on_mismatch: SpeakerMismatchAction::Flag,
            verify_timeout_ms: 2_000,
            ..SpeakerGateConfig::default()
        },
        ..Default::default()
    }
}

async fn rig(split: bool) -> Rig {
    rig_with(agent_config(split)).await
}

/// A rig whose STT ends utterances in [`FinalizeMode::Deferred`] (Deepgram, ElevenLabs, Mistral,
/// Fish, cluster-speech shape).
async fn rig_deferred(split: bool) -> Rig {
    rig_with_script(
        agent_config(split),
        Script {
            deferred: true,
            ..Script::default()
        },
    )
    .await
}

async fn rig_with(config: VoiceAgentConfig) -> Rig {
    rig_with_script(config, Script::default()).await
}

async fn rig_with_script(config: VoiceAgentConfig, initial: Script) -> Rig {
    let script = Arc::new(Mutex::new(initial));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::LocalSherpa,
        Arc::new(SplitFactory {
            script: Arc::clone(&script),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let events = agent.subscribe_events();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| {
        tokio::task::block_in_place(|| std::thread::sleep(Duration::from_millis(4)));
        Ok(())
    });
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    Rig {
        agent,
        script,
        events,
        log: Vec::new(),
    }
}

impl Rig {
    fn drain(&mut self) {
        while let Ok(e) = self.events.try_recv() {
            self.log.push(e);
        }
    }

    async fn feed(&self, level: i16, n: usize) {
        let frame = stereo_frame(level);
        for _ in 0..n {
            self.agent
                .process_inbound_pcm(Bytes::from(frame.clone()), 20)
                .await
                .unwrap();
        }
    }

    /// Feeds `n` frames, one verdict round trip at a time, so the verdicts of earlier windows
    /// are applied while the speech goes on (as in real time).
    async fn feed_paced(&self, level: i16, n: usize) {
        for _ in 0..n {
            self.feed(level, 1).await;
            sleep(Duration::from_millis(3)).await;
        }
    }

    async fn idle(&self) {
        assert!(self.agent.wait_stt_idle(Duration::from_secs(20)).await);
        sleep(Duration::from_millis(50)).await;
    }

    /// Two caller utterances enroll the caller.
    async fn enroll(&mut self) {
        for _ in 0..2 {
            self.feed(A, 20).await;
            self.feed(0, 30).await;
            self.idle().await;
        }
        for _ in 0..300 {
            if self.agent.speaker_gate_enrolled() {
                self.drain();
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
        panic!("caller not enrolled");
    }

    /// Starts a long agent reply and returns once the agent is speaking (`agent_speaking_start`),
    /// so the next user speech starts during playback. A fixed sleep is not enough: when TTS
    /// starts after the user's onset, no barge-in is held at `SpeechStart` and the utterance runs
    /// into C1 (`user_stt_not_found`) instead.
    async fn start_speaking(&self) {
        let mut events = self.agent.subscribe_events();
        let agent = Arc::clone(&self.agent);
        let text = "interrupt this mock playback please ".repeat(40);
        tokio::spawn(async move {
            let _ = agent.send_text_to_tts(&text).await;
        });
        let started = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match events.recv().await {
                    Ok(e) if e.kind == SpeechEventKind::AgentSpeakingStart => return,
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => panic!("event stream closed"),
                }
            }
        })
        .await;
        assert!(started.is_ok(), "agent did not start speaking within 5 s");
    }

    async fn wait_splits(&self, n: u64) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.agent.speaker_split_counts().0 < n {
            assert!(Instant::now() < deadline, "no speaker split");
            sleep(Duration::from_millis(5)).await;
        }
    }

    fn speaking_start_ids(&self) -> Vec<String> {
        self.log
            .iter()
            .filter(|e| e.kind == SpeechEventKind::UserSpeakingStart)
            .filter_map(|e| e.utterance_id.clone())
            .collect()
    }

    /// Index and event of the final of `id`.
    fn final_of(&self, id: &str) -> (usize, SpeechEvent) {
        self.log
            .iter()
            .enumerate()
            .find(|(_, e)| {
                e.kind == SpeechEventKind::UserSpeechFinal && e.utterance_id.as_deref() == Some(id)
            })
            .map(|(i, e)| (i, e.clone()))
            .unwrap_or_else(|| panic!("no final for {id}"))
    }

    fn barge_ids(&self) -> Vec<Option<String>> {
        self.log
            .iter()
            .filter(|e| e.kind == SpeechEventKind::BargeIn)
            .map(|e| e.utterance_id.clone())
            .collect()
    }
}

/// `(a, b)` frame counts in a final's `"a<n> b<m>"` text.
fn ab(final_event: &SpeechEvent) -> (usize, usize) {
    let text = final_event.text.clone().unwrap_or_default();
    let mut it = text.split_whitespace();
    let a = it.next().unwrap()[1..].parse().unwrap();
    let b = it.next().unwrap()[1..].parse().unwrap();
    (a, b)
}

/// The utterance ids of the split: U1 (the third utterance after enrollment) and U2.
fn split_ids(rig: &Rig) -> (String, String) {
    let ids = rig.speaking_start_ids();
    assert!(ids.len() >= 4, "speaking starts: {ids:?}");
    (ids[2].clone(), ids[3].clone())
}

// ---- tests ----------------------------------------------------------------------------------

/// Caller then other: U1 is decoded only from the caller's PCM, U2 only from the other's, and no
/// frame is lost or duplicated at the split (±1 frame). U1's final comes before U2's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn caller_then_other_splits_and_redecodes_each_side() {
    let mut rig = rig(true).await;
    rig.enroll().await;
    rig.feed_paced(A, 20).await;
    rig.feed_paced(0, 1).await;
    rig.feed_paced(B, 40).await;
    rig.wait_splits(1).await;
    rig.feed(0, 30).await;
    rig.idle().await;
    rig.drain();
    let (u1, u2) = split_ids(&rig);
    let (i1, f1) = rig.final_of(&u1);
    let (i2, f2) = rig.final_of(&u2);
    assert!(i1 < i2, "U1 final before U2 final");
    assert_eq!(f1.reason.as_deref(), Some("speaker_split"));
    assert_eq!(f1.speaker.as_deref(), Some("caller"));
    assert_eq!(f2.speaker.as_deref(), Some("other"));
    let ((a1, b1), (a2, b2)) = (ab(&f1), ab(&f2));
    assert!(b1 <= 1, "U1 holds other-speaker frames: a{a1} b{b1}");
    assert!(a2 <= 1, "U2 holds caller frames: a{a2} b{b2}");
    assert!((19..=21).contains(&(a1 + a2)), "caller frames {a1}+{a2}");
    assert!((39..=41).contains(&(b1 + b2)), "other frames {b1}+{b2}");
    // No U2 transcript event precedes U1's final.
    assert!(!rig.log[..i1].iter().any(|e| {
        matches!(
            e.kind,
            SpeechEventKind::UserSpeechFinal | SpeechEventKind::UserSpeechPartial
        ) && e.utterance_id.as_deref() == Some(u2.as_str())
    }));
    // U2's lifecycle events may precede U1's final, but each carries U2's own id.
    let u2_first = rig
        .log
        .iter()
        .position(|e| e.utterance_id.as_deref() == Some(u2.as_str()))
        .expect("U2 events");
    for e in &rig.log[u2_first..i1] {
        // `vad_triggered` carries no utterance id on any path, so it is not checked here.
        if matches!(
            e.kind,
            SpeechEventKind::UserSpeakingStart | SpeechEventKind::UserSttStart
        ) {
            assert_eq!(
                e.utterance_id.as_deref(),
                Some(u2.as_str()),
                "U2 lifecycle event before U1's final carries U2's id: {:?}",
                e.kind
            );
        }
    }
    // After U2's speaking_start, U1's id appears only on its terminal: the stream close of its
    // re-decode (`stt_stream_end`, `user_stt_end`, the same shape every utterance ends with)
    // and the speaker_split final, in that order. No partial: live partials were discarded.
    let u2_start = rig
        .log
        .iter()
        .position(|e| {
            e.kind == SpeechEventKind::UserSpeakingStart
                && e.utterance_id.as_deref() == Some(u2.as_str())
        })
        .expect("U2 speaking_start");
    let late_u1: Vec<_> = rig.log[u2_start..]
        .iter()
        .filter(|e| e.utterance_id.as_deref() == Some(u1.as_str()))
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        late_u1,
        vec![
            SpeechEventKind::SttStreamEnd,
            SpeechEventKind::UserSttEnd,
            SpeechEventKind::UserSpeechFinal
        ],
        "U1 events after U2 started"
    );
    // The split event names both utterances, comes after U2's speaking_start and before any U2
    // barge-in, and is emitted exactly once.
    let split_evs: Vec<usize> = rig
        .log
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == SpeechEventKind::UserUtteranceSplit)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(split_evs.len(), 1, "one user_utterance_split");
    let se = &rig.log[split_evs[0]];
    assert_eq!(se.from_utterance_id.as_deref(), Some(u1.as_str()));
    assert_eq!(se.to_utterance_id.as_deref(), Some(u2.as_str()));
    assert_eq!(se.split_from.as_deref(), Some("caller"));
    assert_eq!(se.split_to.as_deref(), Some("other"));
    assert!(se.split_at_ms.is_some_and(|ms| ms > 0));
    assert!(
        u2_start < split_evs[0],
        "split after U2's user_speaking_start"
    );
    assert!(
        !rig.log[..split_evs[0]]
            .iter()
            .any(|e| e.kind == SpeechEventKind::BargeIn
                && e.utterance_id.as_deref() == Some(u2.as_str())),
        "no U2 barge-in before the split event"
    );
    let (splits, _, _, _, discarded, timeouts) = rig.agent.speaker_split_counts();
    assert_eq!(splits, 1);
    // Inline providers abandon the mixed stream: there is no result to drop.
    assert_eq!(
        discarded, 0,
        "the mixed live stream is abandoned, not decoded"
    );
    assert_eq!(timeouts, 0);
    rig.agent.stop().await.unwrap();
}

/// Inline split: the mixed live stream is dropped without a decode (one abandon), so only
/// enrollment, U1 and U2 are finalized.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inline_split_abandons_the_mixed_stream_without_a_decode() {
    let mut rig = rig(true).await;
    rig.enroll().await;
    rig.feed_paced(A, 20).await;
    rig.feed_paced(0, 1).await;
    rig.feed_paced(B, 40).await;
    rig.wait_splits(1).await;
    rig.feed(0, 30).await;
    rig.idle().await;
    rig.drain();
    {
        let script = rig.script.lock().unwrap();
        assert_eq!(script.finalize_calls, 4, "2 enrollment + U1 + U2");
        assert_eq!(script.abandoned.len(), 1);
    }
    // Abandoned, not decoded: no discarded result exists.
    assert_eq!(rig.agent.speaker_split_counts().4, 0);
    rig.agent.stop().await.unwrap();
}

/// A brief VAD gap (resumed inside the gate hold) before the change: the gate's media clock
/// counts the gap, and U1's re-decoded PCM still ends at the split within a frame. The gap frames
/// are shared between U1's end and U2's start without loss or duplication.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_after_a_brief_vad_gap_maps_onto_the_replay_pcm() {
    let mut rig = rig(true).await;
    rig.enroll().await;
    rig.feed_paced(A, 20).await;
    rig.feed_paced(0, 3).await;
    rig.feed_paced(B, 40).await;
    rig.wait_splits(1).await;
    rig.feed(0, 30).await;
    rig.idle().await;
    rig.drain();
    let (u1, u2) = split_ids(&rig);
    let (_, f1) = rig.final_of(&u1);
    let (_, f2) = rig.final_of(&u2);
    let ((a1, b1), (a2, b2)) = (ab(&f1), ab(&f2));
    assert!(b1 <= 1, "U1 holds other-speaker frames: a{a1} b{b1}");
    assert!(a2 <= 1, "U2 holds caller frames: a{a2} b{b2}");
    assert!((19..=21).contains(&(a1 + a2)), "caller frames {a1}+{a2}");
    assert!((39..=41).contains(&(b1 + b2)), "other frames {b1}+{b2}");
    // The re-decoded U1 stream ends at the split ± 1 frame, followed only by the agent's
    // endpoint tail (`agent::stt_endpoint_tail_ms`). Streams in order: enrollment, U1's
    // re-decode, U2 (the mixed live stream is abandoned).
    let (streams, abandoned) = {
        let script = rig.script.lock().unwrap();
        (script.streams.clone(), script.abandoned.clone())
    };
    assert_eq!(
        abandoned.len(),
        1,
        "one abandoned live stream: {abandoned:?}"
    );
    assert!(
        count(&abandoned[0], 'A') >= 18 && count(&abandoned[0], 'B') >= 1,
        "abandoned stream mixes both speakers: {abandoned:?}"
    );
    let after = &streams[..];
    // Last match: the enrollment streams are caller-only too and come first.
    let u1_stream = after
        .iter()
        .rev()
        .find(|s| count(s, 'A') >= 18 && count(s, 'B') == 0)
        .unwrap_or_else(|| panic!("re-decoded U1 stream: {streams:?}"));
    let u2_stream = after
        .iter()
        .find(|s| count(s, 'B') >= 38 && count(s, 'A') == 0)
        .unwrap_or_else(|| panic!("U2 stream: {streams:?}"));
    let vad = agent_config(true).vad;
    let tail_frames = node_webrtc_rust_speech::agent::stt_endpoint_tail_ms(&vad) as usize / 20;
    let gap_frames = 3;
    let last_a = u1_stream.rfind('A').unwrap();
    let u1_trailing = u1_stream.len() - last_a - 1;
    let u2_leading = u2_stream.find('B').unwrap();
    // The split falls inside the 3-frame gap: U1 keeps the gap frames before it, U2 the rest.
    let u1_gap = u1_trailing
        .checked_sub(tail_frames)
        .unwrap_or_else(|| panic!("U1 lost its endpoint tail: {u1_stream}"));
    assert!(
        u1_gap <= gap_frames + 1,
        "U1 ends {u1_gap} frames past the caller (split ± 1): U1 {u1_stream}"
    );
    assert!(
        (u1_gap + u2_leading).abs_diff(gap_frames) <= 1,
        "gap split {u1_gap}+{u2_leading} vs {gap_frames}: U1 {u1_stream} U2 {u2_stream}"
    );
    rig.agent.stop().await.unwrap();
}

/// U1's re-decode answers only after U2 closed: its result goes to U1 only, U2 keeps its own,
/// and U1's final still comes first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_redecode_after_u2_closed_is_applied_to_u1_only() {
    let mut rig = rig(true).await;
    rig.enroll().await;
    let release = Arc::new(Notify::new());
    // Finalize calls: 2 enrollment, 3 U1's re-decode; the mixed live stream is abandoned, not finalized.
    rig.script.lock().unwrap().block_on_call = Some((3, Arc::clone(&release)));
    rig.feed_paced(A, 20).await;
    rig.feed_paced(0, 1).await;
    rig.feed_paced(B, 40).await;
    rig.wait_splits(1).await;
    rig.feed(0, 30).await;
    sleep(Duration::from_millis(300)).await;
    rig.drain();
    assert!(
        !rig.log
            .iter()
            .any(
                |e| e.kind == SpeechEventKind::UserSpeechFinal && !rig.log.is_empty() && {
                    let ids = rig.speaking_start_ids();
                    ids.len() >= 3 && e.utterance_id.as_deref() == Some(ids[2].as_str())
                }
            ),
        "U1's final waits for its re-decode"
    );
    release.notify_one();
    rig.idle().await;
    rig.drain();
    let (u1, u2) = split_ids(&rig);
    let (i1, f1) = rig.final_of(&u1);
    let (i2, f2) = rig.final_of(&u2);
    assert!(i1 < i2);
    let ((a1, b1), (a2, b2)) = (ab(&f1), ab(&f2));
    assert!(a1 >= 18 && b1 <= 1, "U1 text a{a1} b{b1}");
    assert!(b2 >= 38 && a2 <= 1, "U2 text a{a2} b{b2}");
    assert_eq!(
        rig.log
            .iter()
            .filter(|e| e.kind == SpeechEventKind::UserSpeechFinal
                && e.utterance_id.as_deref() == Some(u1.as_str()))
            .count(),
        1
    );
    rig.agent.stop().await.unwrap();
}

/// The re-decode gets no final within the vendor's finalize wait: the named error is emitted for
/// U1, U1 ends without a final (no text from the mixed live stream), and U2 is not held up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redecode_timeout_emits_named_error_and_does_not_hang() {
    let mut rig = rig(true).await;
    rig.enroll().await;
    {
        let mut script = rig.script.lock().unwrap();
        // 2 enrollment, 3 U1's re-decode; the mixed live stream is abandoned, not finalized.
        script.block_on_call = Some((3, Arc::new(Notify::new())));
        script.finalize_wait_ms = Some(400);
    }
    rig.feed_paced(A, 20).await;
    rig.feed_paced(0, 1).await;
    rig.feed_paced(B, 40).await;
    rig.wait_splits(1).await;
    rig.feed(0, 30).await;
    rig.idle().await;
    rig.drain();
    let (u1, u2) = split_ids(&rig);
    // Exactly one named error for U1: the STT actor's bound is the only emitter.
    let errors: Vec<_> = rig
        .log
        .iter()
        .filter(|e| {
            e.kind == SpeechEventKind::Error
                && e.reason.as_deref()
                    == Some(node_webrtc_rust_speech::agent::SPEAKER_SPLIT_REDECODE_TIMEOUT)
        })
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].utterance_id.as_deref(), Some(u1.as_str()));
    assert!(!rig
        .log
        .iter()
        .any(|e| e.kind == SpeechEventKind::UserSpeechFinal
            && e.utterance_id.as_deref() == Some(u1.as_str())));
    assert!(rig
        .log
        .iter()
        .any(|e| e.kind == SpeechEventKind::UserSttNotFound
            && e.utterance_id.as_deref() == Some(u1.as_str())));
    let (_, f2) = rig.final_of(&u2);
    assert!(ab(&f2).1 >= 38);
    assert_eq!(rig.agent.speaker_split_counts().5, 1);
    rig.agent.stop().await.unwrap();
}

/// Feeds 25 silent frames, then `a` caller frames, `gap` silent frames and `b` other frames, and
/// checks the split event against the inbound media clock built from the frames the test fed:
/// `from_start_ms + split_at_ms` is the first `B` frame's media time (±1 frame), `from_start_ms`
/// is U1's speech onset (first `A` frame plus at most the VAD's `min_speech_duration_ms` + 1
/// frame), and `inbound_media_ms()` equals frames fed × 20.
async fn assert_split_on_media_clock(gap: usize) {
    const FRAME_MS: u64 = 20;
    let mut rig = rig(true).await;
    rig.enroll().await;
    // Enrollment fed 2 × (20 + 30) frames.
    let mut fed: u64 = 2 * (20 + 30);
    rig.feed(0, 25).await;
    fed += 25;
    let first_a_ms = fed * FRAME_MS;
    rig.feed_paced(A, 20).await;
    rig.feed_paced(0, gap).await;
    fed += 20 + gap as u64;
    let first_b_ms = fed * FRAME_MS;
    rig.feed_paced(B, 40).await;
    fed += 40;
    rig.wait_splits(1).await;
    rig.feed(0, 30).await;
    fed += 30;
    rig.idle().await;
    rig.drain();
    assert_eq!(rig.agent.inbound_media_ms().await, fed * FRAME_MS);
    let splits: Vec<_> = rig
        .log
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserUtteranceSplit)
        .collect();
    assert_eq!(splits.len(), 1, "{splits:?}");
    let from = u64::from(splits[0].from_start_ms.expect("from_start_ms"));
    let split_at = u64::from(splits[0].split_at_ms.expect("split_at_ms"));
    let min_speech = u64::from(agent_config(true).vad.min_speech_duration_ms);
    assert!(
        (first_a_ms..=first_a_ms + min_speech + FRAME_MS).contains(&from),
        "from_start_ms {from} vs first A {first_a_ms} (+{min_speech} + {FRAME_MS})"
    );
    let split_media = from + split_at;
    assert!(
        split_media.abs_diff(first_b_ms) <= FRAME_MS,
        "split {from}+{split_at}={split_media} vs first B {first_b_ms}"
    );
    rig.agent.stop().await.unwrap();
}

/// The split event places the split on the inbound media clock (`from_start_ms + split_at_ms`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_event_from_start_ms_maps_onto_the_media_clock() {
    assert_split_on_media_clock(1).await;
}

/// Same after a brief VAD gap: the gate's media clock counts the gap frames, so does the inbound
/// clock, and the onset stays the first speech start of the utterance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_event_from_start_ms_maps_onto_the_media_clock_after_a_brief_vad_gap() {
    assert_split_on_media_clock(3).await;
}

/// Caller then other while the agent speaks: the caller's U1 barges in, then a new reply plays,
/// and the other speaker's U2 never barges in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_u2_never_barges_in() {
    let mut rig = rig(true).await;
    rig.enroll().await;
    rig.start_speaking().await;
    rig.feed_paced(A, 20).await;
    rig.start_speaking().await;
    rig.feed_paced(B, 40).await;
    rig.wait_splits(1).await;
    rig.feed_paced(B, 10).await;
    rig.agent.flush_tts().await.unwrap();
    rig.feed(0, 30).await;
    rig.idle().await;
    rig.drain();
    let (u1, u2) = split_ids(&rig);
    let barges = rig.barge_ids();
    assert!(
        barges.contains(&Some(u1.clone())),
        "caller U1 barges in: {barges:?}"
    );
    assert!(
        !barges.contains(&Some(u2.clone())),
        "other U2 barged in: {barges:?}"
    );
    assert_eq!(rig.final_of(&u2).1.speaker.as_deref(), Some("other"));
    rig.agent.stop().await.unwrap();
}

/// Flag off: the same audio stays one utterance with one final, and nothing is split.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flag_off_keeps_one_utterance() {
    let mut rig = rig(false).await;
    rig.enroll().await;
    rig.feed_paced(A, 20).await;
    rig.feed_paced(B, 40).await;
    rig.feed(0, 30).await;
    rig.idle().await;
    rig.drain();
    let ids = rig.speaking_start_ids();
    assert_eq!(ids.len(), 3, "{ids:?}");
    let (_, f) = rig.final_of(&ids[2]);
    assert_eq!(f.reason, None);
    let (a, b) = ab(&f);
    assert!(a >= 19 && b >= 39, "a{a} b{b}");
    assert_eq!(rig.agent.speaker_split_counts(), (0, 0, 0, 0, 0, 0));
    {
        let script = rig.script.lock().unwrap();
        assert!(script.abandoned.is_empty());
        assert_eq!(script.finalize_calls, 3, "2 enrollment + 1");
    }
    rig.agent.stop().await.unwrap();
}

/// The replay ring overflowed before the change: no split, never a partial re-decode. The
/// overflow skip is counted and the turn stays one utterance. A test-only 600 ms cap keeps the
/// turn short (a 15 s turn would also run into the C1 no-partial listen timeout).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_overflow_skips_the_split() {
    let mut rig = rig(true).await;
    rig.enroll().await;
    rig.agent.set_utterance_replay_max_ms_for_test(600).await;
    rig.feed_paced(A, 40).await;
    rig.feed_paced(0, 1).await;
    rig.feed_paced(B, 40).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while rig.agent.speaker_split_counts().2 == 0 {
        assert!(Instant::now() < deadline, "no overflow skip counted");
        sleep(Duration::from_millis(5)).await;
    }
    rig.feed(0, 30).await;
    rig.idle().await;
    rig.drain();
    let (splits, skipped, overflow, stale, _, _) = rig.agent.speaker_split_counts();
    assert_eq!((splits, overflow, stale), (0, 1, 0));
    assert!(skipped >= overflow, "skipped {skipped} overflow {overflow}");
    let ids = rig.speaking_start_ids();
    assert_eq!(ids.len(), 3, "one utterance after enrollment: {ids:?}");
    let (_, f) = rig.final_of(&ids[2]);
    assert_eq!(f.reason, None);
    let (a, b) = ab(&f);
    assert!(
        a >= 39 && b >= 39,
        "the live stream decodes the whole turn: a{a} b{b}"
    );
    rig.agent.stop().await.unwrap();
}

/// The C1 no-partial listen timeout ends the live record (`user_stt_not_found`) while the caller
/// is still speaking: the record keeps its key and the gate keeps watching the same key. A
/// change confirmed after that is for a terminal record, so it is dropped as stale and counted,
/// never split.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn change_on_a_c1_terminal_record_is_dropped_as_stale() {
    let mut config = agent_config(true);
    config.vad.stt_listen_timeout_ms = 200;
    let mut rig = rig_with(config).await;
    rig.enroll().await;
    rig.feed_paced(A, 20).await;
    // Wall clock and media both past the C1 timeout, with no partial from the fake STT.
    sleep(Duration::from_millis(400)).await;
    rig.feed_paced(A, 10).await;
    rig.feed_paced(0, 1).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while rig.agent.speaker_split_counts().3 == 0 {
        assert!(Instant::now() < deadline, "no stale change counted");
        rig.feed_paced(B, 1).await;
    }
    rig.feed(0, 30).await;
    rig.idle().await;
    rig.drain();
    let (splits, _, overflow, stale, _, _) = rig.agent.speaker_split_counts();
    assert_eq!((splits, overflow), (0, 0));
    assert_eq!(stale, 1);
    let ids = rig.speaking_start_ids();
    assert!(
        rig.log
            .iter()
            .any(|e| e.kind == SpeechEventKind::UserSttNotFound
                && e.utterance_id.as_deref() == Some(ids[2].as_str())),
        "C1 ended the turn's record"
    );
    assert!(!rig
        .log
        .iter()
        .any(|e| e.reason.as_deref() == Some("speaker_split")));
    rig.agent.stop().await.unwrap();
}

/// Other then caller while the agent speaks: the caller stays caller. The first `caller` re-check
/// window upgrades the whole utterance to `caller` and releases the barge-in, exactly as with the
/// flag off. Nothing is split.
async fn other_then_caller(
    split: bool,
) -> (
    Vec<Option<String>>,
    Option<String>,
    Option<String>,
    usize,
    (u64, u64, u64, u64, u64, u64),
) {
    let mut rig = rig(split).await;
    rig.enroll().await;
    rig.start_speaking().await;
    rig.feed_paced(B, 20).await;
    rig.drain();
    assert!(rig.barge_ids().is_empty(), "other speaker barged in");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        rig.drain();
        if !rig.barge_ids().is_empty() || Instant::now() > deadline {
            break;
        }
        rig.feed(A, 1).await;
        sleep(Duration::from_millis(5)).await;
    }
    rig.feed_paced(A, 10).await;
    rig.agent.flush_tts().await.unwrap();
    rig.feed(0, 30).await;
    rig.idle().await;
    rig.drain();
    let ids = rig.speaking_start_ids();
    let (_, f) = rig.final_of(&ids[2]);
    let out = (
        rig.barge_ids(),
        f.speaker.clone(),
        f.reason.clone(),
        ids.len(),
        rig.agent.speaker_split_counts(),
    );
    assert!(!rig
        .log
        .iter()
        .any(|e| e.reason.as_deref() == Some("speaker_split")));
    rig.agent.stop().await.unwrap();
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_then_caller_upgrades_and_barges_in_without_a_split() {
    let (barges, speaker, reason, utterances, counts) = other_then_caller(true).await;
    assert_eq!(utterances, 3, "one utterance after enrollment");
    assert_eq!(barges.len(), 1, "{barges:?}");
    assert_eq!(speaker.as_deref(), Some("caller"));
    assert_eq!(reason, None);
    assert_eq!(counts.0, 0);
    assert_eq!(counts, (0, 0, 0, 0, 0, 0));
    // Same behaviour with the flag off.
    let (barges_off, speaker_off, reason_off, utterances_off, counts_off) =
        other_then_caller(false).await;
    assert_eq!(barges_off.len(), barges.len());
    assert_eq!(
        (speaker_off, reason_off, utterances_off),
        (Some("caller".into()), None, 3)
    );
    assert_eq!(counts_off, counts);
}

// ---- Deferred finalize mode -----------------------------------------------------------------

/// The same split with a provider that answers each finalize later through `poll_output`: U1 is
/// decoded only from its own PCM, U2 only from the other speaker's, U1's final comes before U2's,
/// and the mixed live stream's final is discarded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferred_provider_redecodes_each_side_and_orders_the_finals() {
    let mut rig = rig_deferred(true).await;
    rig.enroll().await;
    rig.feed_paced(A, 20).await;
    rig.feed_paced(0, 1).await;
    rig.feed_paced(B, 40).await;
    rig.wait_splits(1).await;
    rig.feed(0, 30).await;
    rig.idle().await;
    rig.drain();
    let (u1, u2) = split_ids(&rig);
    let (i1, f1) = rig.final_of(&u1);
    let (i2, f2) = rig.final_of(&u2);
    assert!(i1 < i2, "U1 final before U2 final");
    assert_eq!(f1.reason.as_deref(), Some("speaker_split"));
    assert_eq!(f1.speaker.as_deref(), Some("caller"));
    assert_eq!(f2.speaker.as_deref(), Some("other"));
    let ((a1, b1), (a2, b2)) = (ab(&f1), ab(&f2));
    assert!(b1 <= 1, "U1 holds other-speaker frames: a{a1} b{b1}");
    assert!(a2 <= 1, "U2 holds caller frames: a{a2} b{b2}");
    assert!((19..=21).contains(&(a1 + a2)), "caller frames {a1}+{a2}");
    assert!((39..=41).contains(&(b1 + b2)), "other frames {b1}+{b2}");
    assert_eq!(
        rig.log
            .iter()
            .filter(|e| e.kind == SpeechEventKind::UserSpeechFinal
                && e.utterance_id.as_deref() == Some(u1.as_str()))
            .count(),
        1,
        "exactly one final for U1"
    );
    let (splits, _, _, _, discarded, timeouts) = rig.agent.speaker_split_counts();
    assert_eq!(splits, 1);
    assert!(discarded >= 1, "the mixed live stream's final is dropped");
    assert_eq!(timeouts, 0);
    // Deferred providers are not abandoned: the actor falls back to `request_finalize`.
    assert!(rig.script.lock().unwrap().abandoned.is_empty());
    rig.agent.stop().await.unwrap();
}

/// U1's re-decode answer arrives after U2 closed (answers keep request order, so U2's finalize
/// waits behind it): the late result is applied to U1 only, and U1's final still precedes U2's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferred_late_redecode_is_applied_to_u1_only() {
    let mut rig = rig_deferred(true).await;
    rig.enroll().await;
    // Finalize calls: 2 enrollment, 3 the discarded live stream, 4 U1's re-decode.
    rig.script.lock().unwrap().held_calls = vec![4];
    rig.feed_paced(A, 20).await;
    rig.feed_paced(0, 1).await;
    rig.feed_paced(B, 40).await;
    rig.wait_splits(1).await;
    rig.feed(0, 30).await;
    sleep(Duration::from_millis(300)).await;
    rig.drain();
    let ids = rig.speaking_start_ids();
    assert!(ids.len() >= 4, "speaking starts: {ids:?}");
    assert!(
        !rig.log
            .iter()
            .any(|e| e.kind == SpeechEventKind::UserSpeechFinal
                && e.utterance_id.as_deref() == Some(ids[2].as_str())),
        "U1's final waits for its re-decode"
    );
    rig.script.lock().unwrap().held_calls.clear();
    rig.idle().await;
    rig.drain();
    let (u1, u2) = split_ids(&rig);
    let (i1, f1) = rig.final_of(&u1);
    let (i2, f2) = rig.final_of(&u2);
    assert!(i1 < i2, "U1 final before U2 final");
    let ((a1, b1), (a2, b2)) = (ab(&f1), ab(&f2));
    assert!(a1 >= 18 && b1 <= 1, "U1 text a{a1} b{b1}");
    assert!(b2 >= 38 && a2 <= 1, "U2 text a{a2} b{b2}");
    assert_eq!(
        rig.log
            .iter()
            .filter(|e| e.kind == SpeechEventKind::UserSpeechFinal
                && e.utterance_id.as_deref() == Some(u1.as_str()))
            .count(),
        1
    );
    rig.agent.stop().await.unwrap();
}

/// The vendor never answers the re-decode finalize: the named timeout error is emitted once for
/// U1, U1 ends without a final, and the timeout is counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferred_redecode_timeout_emits_named_error_and_does_not_hang() {
    let mut rig = rig_deferred(true).await;
    rig.enroll().await;
    {
        let mut script = rig.script.lock().unwrap();
        script.no_final_on_call = Some(4);
        script.finalize_wait_ms = Some(400);
    }
    rig.feed_paced(A, 20).await;
    rig.feed_paced(0, 1).await;
    rig.feed_paced(B, 40).await;
    rig.wait_splits(1).await;
    rig.feed(0, 30).await;
    // The fake never answers call 4, and answers keep request order, so nothing behind it is
    // answered; the actor's finalize bound closes the entry.
    sleep(Duration::from_millis(1500)).await;
    rig.drain();
    let (u1, _u2) = split_ids(&rig);
    let errors: Vec<_> = rig
        .log
        .iter()
        .filter(|e| {
            e.kind == SpeechEventKind::Error
                && e.reason.as_deref()
                    == Some(node_webrtc_rust_speech::agent::SPEAKER_SPLIT_REDECODE_TIMEOUT)
        })
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].utterance_id.as_deref(), Some(u1.as_str()));
    assert!(!rig
        .log
        .iter()
        .any(|e| e.kind == SpeechEventKind::UserSpeechFinal
            && e.utterance_id.as_deref() == Some(u1.as_str())));
    assert_eq!(rig.agent.speaker_split_counts().5, 1);
    rig.agent.stop().await.unwrap();
}
