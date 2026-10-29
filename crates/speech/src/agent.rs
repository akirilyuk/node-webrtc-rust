//! Voice agent orchestration (attach, start/stop, TTS injection, inbound PCM).
//!
//! [`VoiceAgent`] is the main entry point: one instance per WebRTC session. Inbound audio
//! is processed in [`VoiceAgent::process_inbound_pcm`]; the TypeScript SDK calls that from
//! `RemoteAudioTrack.readSample()` in a loop after [`VoiceAgent::start`].

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::{broadcast, oneshot, Mutex, Notify};

use crate::audio_tap::{audio_tap_dir_from_env, next_tap_agent_id, ContinuousTap, TtsOutTap};
use crate::config::{
    effective_stt_listen_hard_timeout_ms, language_id_allowlist_accepts, language_id_continuous,
    language_id_early, language_id_enabled, resolved_language_id_min_speech_ms,
    resolved_post_utterance_silence_ms, BargeInConfig, EventDeliveryMode, NoiseSuppressionProvider,
    SendTextToTtsOptions, SttConfig, TtsConfig, TtsVendor, UpdateTtsConfigOptions, VadConfig,
    VoiceAgentConfig, VoiceSessionContext,
};
use crate::error::{SpeechError, SpeechResult};
use crate::events::{SpeechEvent, SpeechEventBus};
use crate::otel::{self, SttMetricAttrs};
use crate::pcm::i16_samples_to_bytes;
use crate::pipeline::{
    tts_stream_chunks_enabled, LanguageIdProvider, SpeakerEmbedder, SttProvider, SttTranscript,
    TtsAudioChunk, TtsProgressiveSink, TtsProvider,
};
use crate::registry::VendorRegistry;
use crate::speaker_gate::{BargeDecision, Outcome, SpeakerGate, Verdict};
use crate::stt_actor::{
    default_stt_latency_sink, spawn_stt_actor, test_final_delay_ms_from_env, SttActorHandle,
    SttActorOptions, SttCommand, SttResult, STT_ACTOR_START_MS, STT_ACTOR_STOP_MS,
};
use crate::stt_hold::{
    resolved_hold_max_buffer_ms, resolved_replay_max_age_ms, BeginSttHoldOptions,
    ReleaseSttHoldOptions, SttHold,
};
use crate::stt_pre_roll::{ShortBurstRetainer, SttPreRollBuffer, SHORT_BURST_SEPARATOR_MS};
use crate::tts_buffer::{ReplyTransition, TtsBuffer};
use crate::tts_cache::{normalize_phrase_text, wrap_tts_for_agent};
use crate::tts_text::prepare_engine_text;
use crate::turn_latency::TurnLatencyTracker;
use crate::utterance_replay::{UtteranceReplayBuffer, UtteranceReplaySnapshot};
use crate::vad::{VadEngine, VadTransition};
use node_webrtc_rust_denoise::Stereo48kRnnoise;

/// Callback invoked when PCM should be written to the outbound track.
pub type PcmWriter = Arc<dyn Fn(Bytes, u32) -> SpeechResult<()> + Send + Sync>;

/// Callback invoked to read inbound PCM from the attached remote track.
pub type PcmReader = Arc<dyn Fn() -> SpeechResult<Option<(Bytes, u32)>> + Send + Sync>;

static INBOUND_PCM_FRAMES: AtomicU64 = AtomicU64::new(0);
static UTTERANCE_ID_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_utterance_id() -> String {
    let n = UTTERANCE_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("utt-{n}")
}

struct ReplayFinalContext {
    replaces_utterance_id: Option<String>,
    /// When the context was armed. A context older than [`REPLAY_CONTEXT_MAX_AGE`] is dropped so a
    /// lost replay cannot mislabel a much later user utterance.
    set_at: tokio::time::Instant,
}

/// Maximum age of a pending [`ReplayFinalContext`] when the next final is emitted.
const REPLAY_CONTEXT_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(15);

/// Default wait for a new STT to become ready before replaying audio into it.
const DEFAULT_STT_READY_WAIT_MS: u64 = 15_000;

/// Decrements `tts_worker_tasks_alive` on task exit (including abort).
struct TtsWorkerAliveGuard(Arc<AtomicUsize>);

impl TtsWorkerAliveGuard {
    fn enter(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(counter))
    }
}

impl Drop for TtsWorkerAliveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// `VOICE_DEBUG=1|true|yes`, read once per process.
fn voice_debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("VOICE_DEBUG").ok().as_deref(),
            Some("1") | Some("true") | Some("yes")
        )
    })
}

pub(crate) fn voice_debug(message: impl AsRef<str>) {
    if voice_debug_enabled() {
        eprintln!("[voice-debug] {}", message.as_ref());
    }
}

/// `VOICE_DEBUG=1` or `VOICE_FINALIZE_TRACE=1`: the finalize timing line only, without the
/// per-frame `[voice-debug]` noise that shifts timing on a starved host.
fn finalize_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        voice_debug_enabled()
            || matches!(
                std::env::var("VOICE_FINALIZE_TRACE").ok().as_deref(),
                Some("1") | Some("true") | Some("yes")
            )
    })
}

/// One timing line per finalize decision (no transcript text). `path` is `media_c2`,
/// `wallclock_c2`, `c1`, `gate_finalize`, `complete_pending` (finalize before new speech) or
/// `vendor_final` (a final the STT vendor produced; `closing=false` means no finalize was asked); `wall_ms` is the age of the C2 arm (C1: the listen
/// start), `since_last_pcm_ms` the time since the last inbound frame entered
/// `process_inbound_pcm`.
fn finalize_trace(path: &str, inner: &AgentInner) {
    if !finalize_trace_enabled() {
        return;
    }
    let wall_ms = inner
        .utterance_finalize_armed_at
        .or(inner.stt_listen_started_at)
        .map_or(-1, |t| t.elapsed().as_millis() as i64);
    let since_last_pcm_ms = inner
        .last_inbound_pcm_at
        .map_or(-1, |t| t.elapsed().as_millis() as i64);
    eprintln!(
        "[voice-debug] finalize path={path} media_ms={} wall_ms={wall_ms} since_last_pcm_ms={since_last_pcm_ms} hold_ms={} closing={} vad_speaking={}",
        inner.utterance_media_ms,
        inner.stt_gate_hold_ms,
        inner.stt_endpoint_closing_started,
        VoiceAgent::vad_is_speaking(inner),
    );
}

/// Prefer the inner `Internal` payload so callers see the original drain/writer text once.
fn playback_failure_message(error: &SpeechError) -> String {
    match error {
        SpeechError::Internal(message) => message.clone(),
        other => other.to_string(),
    }
}

/// Synthetic silence fed to STT before finalize — aligned with Sherpa roundtrip harness.
fn stt_endpoint_tail_ms(vad: &VadConfig) -> u32 {
    vad.min_silence_duration_ms.clamp(400, 600)
}

/// True when most of the post–speech-end gate hold has elapsed (90%), i.e. resume is a new phrase not a digit gap.
fn gate_hold_long_pause_elapsed(hold_total: u32, hold_elapsed: u32) -> bool {
    hold_total > 0 && hold_elapsed.saturating_mul(10) > hold_total.saturating_mul(9)
}

/// Time-to-first-audio bookkeeping for the reply whose first outbound frame will emit
/// `agent_speaking_start`.
#[derive(Debug, Clone, Copy)]
struct FirstAudioTiming {
    /// Moment `send_text_to_tts_with_options` accepted the text.
    requested_at: Instant,
    /// Speak request to first PCM chunk from the TTS vendor, once that chunk arrived.
    first_chunk_ms: Option<u32>,
    /// Letters capitalized at the TTS engine input for this reply (`None` for non-Sherpa vendors).
    sentence_case_adjusted: Option<u32>,
}

/// Letters the Sherpa TTS engine input gets capitalized for `text` under `config`.
///
/// `Some(count)` when the active TTS vendor applies sentence-start capitalization (local Sherpa
/// and the in-cluster Sherpa transport; both normalise whitespace first), `None` otherwise or
/// when no TTS is configured.
pub fn sentence_case_adjusted_for(config: Option<&TtsConfig>, text: &str) -> Option<u32> {
    let config = config?;
    match config.provider {
        TtsVendor::LocalSherpa | TtsVendor::ClusterSherpa => {
            let normalized = normalize_phrase_text(text);
            Some(prepare_engine_text(&normalized, config.sentence_case).adjusted)
        }
        _ => None,
    }
}

/// LID state of one user utterance.
#[derive(Default)]
struct UttLid {
    /// PCM collected for offline spoken-language ID during the current user utterance.
    pcm_buffer: Vec<u8>,
    /// Speech from the first voiced frame to the last voiced frame of the utterance, including
    /// gaps between words; silence after the last voiced frame is not counted. `minSpeechMs`
    /// compares against this, not the buffer length.
    voiced_ms: u32,
    /// Gap (silence while the VAD is still speaking) since the last voiced frame; folded into
    /// `voiced_ms` only if speech resumes.
    pending_silence_ms: u32,
    /// True while buffering user PCM for LID after `user_speaking_start`.
    buffering: bool,
    /// Avoid overlapping background LID identify tasks for the same utterance.
    identify_in_flight: bool,
    /// LID identify was requested during TTS; run after outbound playback drains.
    identify_deferred: bool,
    /// True once inbound speech started or deferred LID for this utterance (skip hang-up force identify).
    identify_started: bool,
    /// `language_id_skipped` already emitted for this utterance (exactly once).
    skip_emitted: bool,
    /// `user_language` already emitted for this utterance (first outcome reported).
    language_emitted: bool,
}

/// What kind of utterance a record stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum UttKind {
    /// Audio the user is speaking (or just spoke) live.
    Live,
    /// Audio fed again after an STT swap (`replay_last_utterance`, `release_stt_hold`). It only
    /// ever produces a replay final (`replaces`: the utterance it stands in for).
    Replay { replaces: Option<String> },
}

/// Where a record is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UttPhase {
    /// Being spoken (live) or fed (replay).
    Open,
    /// `EndUtterance` was sent to the STT actor; the answer has not been fully applied yet.
    Closing,
}

/// Outcome of [`VoiceAgent::begin_utterance_close`].
enum CloseStarted {
    /// An STT hold is active: nothing was sent to the STT; the caller closes the record inline.
    Held,
    /// There was no live record to close.
    NoRecord,
    /// The live record moved to `closing`; its `EndUtterance` was sent.
    Closing,
}

/// STT/turn state of one user utterance. `key` is a per-agent sequence number that identifies
/// the utterance for asynchronous results (LID and the STT actor); `id` is the id every event of
/// the utterance carries. Both are allocated once, when the record is created.
struct SttUtterance {
    key: u64,
    /// Id of the utterance (`utt-N`); carried by every event about it.
    id: String,
    kind: UttKind,
    phase: UttPhase,
    /// STT vendor PCM feed open for the current VAD-triggered utterance.
    stream_open: bool,
    /// User STT session open (`user_stt_start` … `user_stt_end` / `user_stt_not_found`).
    user_stt_session_open: bool,
    /// `user_speaking_start` already emitted for the current STT utterance.
    speaking_start_emitted: bool,
    /// `user_speaking_end` paired with the next `user_speech_final` for this utterance.
    speaking_end_emitted: bool,
    /// `user_speech_final` already emitted for this utterance.
    final_emitted: bool,
    /// `user_stt_not_found` already emitted for this utterance (it is emitted at most once).
    not_found_emitted: bool,
    /// A terminal event (final or not_found) was emitted. Later results for the record are late.
    terminal: bool,
    /// At least one `user_speech_partial` this utterance.
    partials_emitted: bool,
    /// Last partial text for C2 forced final fallback.
    last_partial_text: Option<String>,
    /// Set on each VAD `SpeechStart` when `vad.enabled` (barge prerequisite).
    vad_triggered: bool,
    /// `stt_stream_end` reason used when the close is applied (`FinalizeDone`).
    close_reason: &'static str,
    /// The next final is a replay final (held release while the utterance was still open).
    replay_ctx: Option<ReplayFinalContext>,
    /// Fired with `true` when a final was emitted, `false` when the record ended without one.
    terminal_waiter: Option<oneshot::Sender<bool>>,
    lid: UttLid,
}

impl SttUtterance {
    fn new(key: u64, id: String) -> Self {
        Self {
            key,
            id,
            kind: UttKind::Live,
            phase: UttPhase::Open,
            stream_open: false,
            user_stt_session_open: false,
            speaking_start_emitted: false,
            speaking_end_emitted: false,
            final_emitted: false,
            not_found_emitted: false,
            terminal: false,
            partials_emitted: false,
            last_partial_text: None,
            vad_triggered: false,
            close_reason: "end_of_utterance",
            replay_ctx: None,
            terminal_waiter: None,
            lid: UttLid::default(),
        }
    }

    /// Marks the record terminal (a final or a not_found was emitted).
    fn mark_terminal(&mut self) {
        self.terminal = true;
    }

    /// Resolves the host waiting for this record's outcome.
    fn fire_terminal_waiter(&mut self, final_emitted: bool) {
        if let Some(waiter) = self.terminal_waiter.take() {
            let _ = waiter.send(final_emitted);
        }
    }
}

struct AgentInner {
    config: VoiceAgentConfig,
    attached: bool,
    running: bool,
    otel: AgentOtelState,
    vad: Option<VadEngine>,
    stt_pre_roll: Option<SttPreRollBuffer>,
    /// Short voiced bursts that ended without VAD `SpeechStart`, kept for the next new stream.
    short_burst: ShortBurstRetainer,
    /// VAD-pending onset frames held back from the closing STT stream during the gate hold.
    /// Counted by exactly one stream (handoff-counting). Mono s16le 16 kHz.
    hold_pending_pcm: Vec<u8>,
    /// Milliseconds of inbound audio still forwarded to STT after VAD speech end.
    stt_gate_hold_ms: u32,
    /// When true, finalize STT once gate hold drains (after trailing speech is relayed).
    stt_finalize_pending: bool,
    /// Endpoint tail + `finalize_utterance` already started for the current pending close.
    stt_endpoint_closing_started: bool,
    /// True while agent TTS is synthesizing or playing outbound audio.
    agent_speaking: bool,
    agent_speaking_since: Option<Instant>,
    /// Timing for the reply that will start playback; taken when `agent_speaking_start` is emitted.
    pending_first_audio: Option<FirstAudioTiming>,
    /// Turn latency (speech end to first reply audio) bookkeeping for OTel.
    turn_latency: TurnLatencyTracker,
    /// C1 armed when > 0 (wall-clock expiry uses `stt_listen_started_at`).
    stt_listen_deadline_ms: u32,
    /// Wall-clock anchor for C1 from VAD `SpeechStart` (not PCM `duration_ms`).
    stt_listen_started_at: Option<Instant>,
    /// Inbound media ms seen since the C1 listen window armed (C1 expires on this, not on wall).
    stt_listen_media_ms: u32,
    /// One `voice_debug` line per utterance when C1 is deferred due to STT decode backlog.
    c1_backlog_deferred_logged: bool,
    /// When the STT stream open was first seen refused for this wait (`None` = not pending).
    stt_open_wait_started_at: Option<Instant>,
    /// C2: ms remaining until forced `user_speech_final` after last partial or `SpeechEnd`.
    utterance_finalize_deadline_ms: u32,
    /// Wall-clock anchor for C2 when inbound PCM stops (see `c2_wall_clock_ticker`).
    utterance_finalize_armed_at: Option<Instant>,
    /// Last inbound frame wall time (real PCM received, including gate-closed skips).
    last_inbound_pcm_at: Option<Instant>,
    /// Inbound media ms seen since the utterance started (finalize trace only).
    utterance_media_ms: u64,
    /// Start C2 only after gate hold drains when `SpeechEnd` preceded partials.
    defer_utterance_finalize_until_hold: bool,
    /// VAD saw speech during agent TTS; defer immediate flush until STT partial (if required).
    barge_awaiting_stt_partial: bool,
    /// Semantic barge already fired for the current agent playback generation.
    stt_barge_fired_this_agent_playback: bool,
    /// Bumped when TTS drain fails or playback wait times out so blocking jobs observe the error.
    tts_playback_failure_gen: u64,
    /// Original failure text for the latest playback abort (paired with `tts_playback_failure_gen`).
    tts_playback_last_error: Option<String>,
    pcm_writer: Option<PcmWriter>,
    pcm_reader: Option<PcmReader>,
    denoise: Option<Stereo48kRnnoise>,
    /// When false, inbound PCM still runs VAD but skips STT push/poll and speech/STT events.
    stt_enabled: bool,
    /// Last emitted ISO 639-1 code (`user_language`); re-emit only when it changes.
    lid_last_emitted: Option<String>,
    /// Swapped onto the STT provider after the current utterance finalizes.
    pending_stt_config: Option<SttConfig>,
    /// Swapped onto the TTS provider before the next synthesis job.
    pending_tts_config: Option<TtsConfig>,
    /// Post-RNNoise STT feed captured for `replay_last_utterance`.
    utterance_replay_buffer: UtteranceReplayBuffer,
    /// Last finalized utterance eligible for replay.
    last_replay_snapshot: Option<UtteranceReplaySnapshot>,
    /// Active host STT hold (`begin_stt_hold`): STT is not fed, results of the old model are ignored,
    /// PCM is buffered.
    stt_hold: Option<SttHold>,
    /// The replay ring was started at the SpeechStart pre-roll flush (keeps the onset audio).
    replay_begun_at_pre_roll: bool,
    /// The user utterance currently being assembled (STT/turn and LID state). `None` until the
    /// first utterance and after its record moved to `closing`.
    live: Option<SttUtterance>,
    /// Next `SttUtterance::key`.
    next_utterance_key: u64,
    /// Utterances whose `EndUtterance` was sent to the STT actor and whose `FinalizeDone` has not
    /// been applied yet, oldest first. Replay records live here too.
    closing: VecDeque<SttUtterance>,
    /// Records that finished closing but still have a spoken-language identify deferred or running:
    /// the outcome is written to the record by key, so `user_language` / `language_id_skipped`
    /// still carry the id of their own utterance. Dropped once the identify finished.
    lid_retained: VecDeque<SttUtterance>,
    /// Utterance key for which the old-model-result-ignored line was already logged during the
    /// current hold (one line per utterance).
    held_ignore_logged_key: Option<u64>,
    /// Test-only: audio tap directory set by `set_audio_tap_dir_for_test` (wins over the env).
    audio_tap_dir_override: Option<&'static Path>,
    /// Test-only: agent id in tap file names; 0 while the tap is off.
    audio_tap_agent_id: u64,
    /// Test-only: the listener's inbound mono 16 kHz PCM (see [`crate::audio_tap`]).
    inbound_tap: Option<ContinuousTap>,
    /// Test-only: outbound TTS frames (see [`crate::audio_tap`]).
    tts_out_tap: Option<Arc<std::sync::Mutex<TtsOutTap>>>,
}

impl AgentInner {
    /// New record with a fresh key and id. Not stored.
    fn new_record(&mut self) -> SttUtterance {
        let key = self.next_utterance_key;
        self.next_utterance_key += 1;
        SttUtterance::new(key, next_utterance_id())
    }

    /// The live record, created when there is none (VAD-disabled / vendor-endpointed flows).
    fn ensure_live(&mut self) -> &mut SttUtterance {
        if self.live.is_none() {
            let rec = self.new_record();
            self.live = Some(rec);
        }
        self.live.as_mut().expect("live record present")
    }

    /// The record `key` names: the live one first, then the closing queue, then records kept
    /// only for their LID outcome.
    fn record_mut(&mut self, key: u64) -> Option<&mut SttUtterance> {
        if self.live.as_ref().is_some_and(|r| r.key == key) {
            return self.live.as_mut();
        }
        if let Some(pos) = self.closing.iter().position(|r| r.key == key) {
            return self.closing.get_mut(pos);
        }
        self.lid_retained.iter_mut().find(|r| r.key == key)
    }

    fn record_ref(&self, key: u64) -> Option<&SttUtterance> {
        if let Some(live) = self.live.as_ref().filter(|r| r.key == key) {
            return Some(live);
        }
        self.closing
            .iter()
            .chain(self.lid_retained.iter())
            .find(|r| r.key == key)
    }

    /// The record `key` names when STT results can still be applied to it (live or closing).
    fn stt_record_ref(&self, key: u64) -> Option<&SttUtterance> {
        if let Some(live) = self.live.as_ref().filter(|r| r.key == key) {
            return Some(live);
        }
        self.closing.iter().find(|r| r.key == key)
    }

    /// Keeps a finished record while its LID outcome is still to be written (at most 4).
    fn retain_for_lid(&mut self, rec: SttUtterance) {
        if !(rec.lid.identify_deferred || rec.lid.identify_in_flight) {
            return;
        }
        if self.lid_retained.len() >= 4 {
            self.lid_retained.pop_front();
        }
        self.lid_retained.push_back(rec);
    }

    fn live_key(&self) -> Option<u64> {
        self.live.as_ref().map(|r| r.key)
    }

    fn live_id(&self) -> Option<String> {
        self.live.as_ref().map(|r| r.id.clone())
    }

    /// Reads a flag of the live record; `false` when there is none.
    fn live_flag(&self, f: impl Fn(&SttUtterance) -> bool) -> bool {
        self.live.as_ref().is_some_and(f)
    }

    /// True when any record (live or closing) has a LID identify running.
    fn any_lid_in_flight(&self) -> bool {
        self.live_flag(|r| r.lid.identify_in_flight)
            || self
                .closing
                .iter()
                .chain(self.lid_retained.iter())
                .any(|r| r.lid.identify_in_flight)
    }

    /// Keys of the records with a deferred LID identify (live first).
    fn deferred_lid_keys(&self) -> Vec<u64> {
        let mut keys = Vec::new();
        if self.live_flag(|r| r.lid.identify_deferred) {
            keys.extend(self.live_key());
        }
        keys.extend(
            self.closing
                .iter()
                .chain(self.lid_retained.iter())
                .filter(|r| r.lid.identify_deferred)
                .map(|r| r.key),
        );
        keys
    }
}

/// Per-session OpenTelemetry state (public for the `otel` module).
#[derive(Default)]
pub struct AgentOtelState {
    pub session_context: VoiceSessionContext,
    #[cfg(feature = "otel")]
    pub session_span: Option<tracing::Span>,
    #[cfg(feature = "otel")]
    pub stt_vendor: Option<crate::config::SttVendor>,
    #[cfg(feature = "otel")]
    pub tts_vendor: Option<crate::config::TtsVendor>,
}

struct TtsSynthesisJob {
    text: String,
    done: Option<tokio::sync::oneshot::Sender<SpeechResult<()>>>,
    /// When false, VAD / STT-partial barge-in must not flush or cancel this job (explicit
    /// `flush_tts` / `stop` still do).
    interruptible: bool,
    /// When the text was accepted by `send_text_to_tts_with_options` (time-to-first-audio anchor).
    requested_at: Instant,
    /// Host-chosen id of this reply; tags the reply events and the job's `tts_*` events.
    reply_id: Option<Arc<str>>,
}

/// One voice agent session bound to a single peer connection.
///
/// Holds VAD (optional), STT/TTS providers, TTS outbound buffer, and utterance state
/// (`stt_gate_hold_ms`, finalize pending, barge-in flags). Thread-safe via internal `Mutex` / `Mutex`es.
pub struct VoiceAgent {
    event_bus: SpeechEventBus,
    tts_buffer: TtsBuffer,
    #[allow(dead_code)]
    registry: Arc<VendorRegistry>,
    inner: Arc<Mutex<AgentInner>>,
    /// The STT provider until `start` hands it to the STT actor (the actor needs a tokio runtime,
    /// which a sync `new()` caller such as the NAPI constructor does not have).
    stt_provider: std::sync::Mutex<Option<Box<dyn SttProvider>>>,
    /// The per-agent STT actor, spawned by `start`. It is the only caller of the provider.
    stt_actor: std::sync::Mutex<Option<Arc<SttActorHandle>>>,
    /// Applies the actor's results to utterance records (holds only a `Weak<VoiceAgent>`).
    stt_pump: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Serialises one whole `process_inbound_pcm` call, one pumped STT result, one C1/C2 ticker
    /// check and the state-changing parts of host APIs. Lock order: step, then `inner`.
    utterance_step: Mutex<()>,
    /// Host/test override (ms) for the STT readiness wait before a replay; 0 = use
    /// `SPEECH_STT_READY_WAIT_MS` (default 15000).
    stt_ready_wait_override_ms: AtomicU64,
    /// Test-only delay (ms, 0 = none) between the barge-in TTS flush and the `barge_in` event.
    barge_in_flush_delay_ms: AtomicU64,
    language_id: Mutex<Option<Arc<dyn LanguageIdProvider>>>,
    /// Opt-in speaker gate (`None` unless `speakerGate.enabled`).
    speaker_gate: Option<Arc<SpeakerGate>>,
    /// The embedder until `start` hands it to the gate's verifier task.
    speaker_embedder: std::sync::Mutex<Option<Box<dyn SpeakerEmbedder>>>,
    tts: Arc<Mutex<Option<Box<dyn TtsProvider>>>>,
    tts_drain_wake: Arc<Notify>,
    tts_drain_worker: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    tts_synthesis_queue: Arc<Mutex<VecDeque<TtsSynthesisJob>>>,
    tts_synthesis_wake: Arc<Notify>,
    tts_synthesis_worker: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    tts_synthesis_busy: Arc<AtomicBool>,
    /// True from the moment the worker pops a job (under the queue lock) until it marks
    /// `tts_synthesis_busy`. Closes the window in which a job is in neither the queue nor busy.
    tts_job_dequeued: Arc<AtomicBool>,
    /// True while a non-interruptible job (`interruptible: false`) is synthesizing or playing.
    tts_protected_active: Arc<AtomicBool>,
    /// Woken when an in-flight LID identify task completes (STT close gate + TTS synthesis wait).
    lid_completion_notify: Arc<Notify>,
    /// Count of fault-path `lid_gate_max_wait_ms` expirations (tests only).
    lid_gate_bound_hits: Arc<AtomicUsize>,
    /// Incremented on barge/flush/cancel so in-flight ONNX synthesis can drop late PCM.
    tts_synthesis_epoch: Arc<AtomicU64>,
    /// Set on barge/flush so progressive generators (Sherpa callback) can stop early.
    tts_generate_cancel: Arc<AtomicBool>,
    /// Set on stop/drop paths so synthesis/drain loops exit (no detached tasks).
    tts_workers_shutdown: Arc<AtomicBool>,
    /// Wakes workers blocked in `notified()` so stop cannot hang on idle wait.
    tts_workers_shutdown_wake: Arc<Notify>,
    /// True when a worker join timed out (blocked vendor) — host should recycle.
    tts_shutdown_unhealthy: Arc<AtomicBool>,
    /// Live synthesis + drain tokio tasks (tests / capacity diagnostics).
    tts_worker_tasks_alive: Arc<AtomicUsize>,
    /// In-flight async vendor `synthesize` calls (may outlive aborted Tokio workers).
    tts_vendor_calls_inflight: Arc<AtomicUsize>,
    weak_self: Weak<VoiceAgent>,
    c2_ticker_started: AtomicBool,
    /// True while a `process_inbound_pcm` call is running. The wall-clock ticker skips its C1/C2
    /// checks then: a long call (slow decode inside finalize) is not "PCM stopped".
    inbound_in_flight: AtomicBool,
    /// Mirror of `AgentInner::running` readable without the async lock (host inbound loops).
    running_flag: AtomicBool,
    /// Reference point for `last_inbound_activity_ms`.
    epoch: Instant,
    /// Ms since `epoch` of the last entry into OR exit from `process_inbound_pcm`. The ticker's
    /// "PCM stopped" age counts from here, so a call that ran for seconds does not look stalled
    /// the moment it returns. Atomic because the call's drop guard cannot take the async lock.
    last_inbound_activity_ms: AtomicU64,
}

/// Marks a `process_inbound_pcm` call as running; on every exit path clears the flag and stamps
/// the exit time.
struct InboundInFlight<'a>(&'a VoiceAgent);

impl<'a> InboundInFlight<'a> {
    fn enter(agent: &'a VoiceAgent) -> Self {
        agent.stamp_inbound_activity();
        agent.inbound_in_flight.store(true, Ordering::SeqCst);
        Self(agent)
    }
}

impl Drop for InboundInFlight<'_> {
    fn drop(&mut self) {
        self.0.stamp_inbound_activity();
        self.0.inbound_in_flight.store(false, Ordering::SeqCst);
    }
}

impl VoiceAgent {
    /// Builds agents with STT/TTS from `registry` and VAD/pre-roll from `config.vad`.
    pub fn new(config: VoiceAgentConfig, registry: Arc<VendorRegistry>) -> SpeechResult<Arc<Self>> {
        let mut stt = None;
        let mut tts = None;

        if let Some(stt_cfg) = &config.stt {
            stt = Some(registry.create_stt(stt_cfg)?);
        }
        if let Some(tts_cfg) = &config.tts {
            tts = Some(wrap_tts_for_agent(registry.create_tts(tts_cfg)?, tts_cfg));
        }
        let language_id = if language_id_enabled(&config.language_id) {
            registry
                .create_language_id(config.language_id.as_ref().expect("checked"))?
                .map(Arc::from)
        } else {
            None
        };

        config
            .speaker_gate
            .validate()
            .map_err(SpeechError::Config)?;
        let (speaker_gate, speaker_embedder) = if config.speaker_gate.enabled {
            let embedder = registry.create_speaker_embedder(&config.speaker_gate)?;
            if embedder.is_none() {
                return Err(SpeechError::Config(
                    "speakerGate.enabled but the vendor provides no speaker embedder".into(),
                ));
            }
            (
                Some(SpeakerGate::new(config.speaker_gate.clone())),
                embedder,
            )
        } else {
            (None, None)
        };

        let vad = if config.vad.enabled {
            Some(VadEngine::new(config.vad.clone())?)
        } else {
            None
        };
        let stt_pre_roll = if config.vad.enabled && config.vad.gate_stt {
            Some(SttPreRollBuffer::from_vad_config(&config.vad))
        } else {
            None
        };

        let denoise = match config.noise_suppression.provider {
            NoiseSuppressionProvider::Rnnoise => Some(Stereo48kRnnoise::new()),
            NoiseSuppressionProvider::None => None,
        };

        Ok(Arc::new_cyclic(|weak| Self {
            event_bus: SpeechEventBus::new(),
            tts_buffer: TtsBuffer::new(),
            registry,
            inner: Arc::new(Mutex::new(AgentInner {
                config,
                attached: false,
                running: false,
                otel: AgentOtelState::default(),
                vad,
                stt_pre_roll,
                short_burst: ShortBurstRetainer::new(),
                hold_pending_pcm: Vec::new(),
                stt_gate_hold_ms: 0,
                stt_finalize_pending: false,
                stt_endpoint_closing_started: false,
                agent_speaking: false,
                agent_speaking_since: None,
                pending_first_audio: None,
                turn_latency: TurnLatencyTracker::new(),
                stt_listen_deadline_ms: 0,
                stt_listen_started_at: None,
                stt_listen_media_ms: 0,
                c1_backlog_deferred_logged: false,
                stt_open_wait_started_at: None,
                utterance_finalize_deadline_ms: 0,
                utterance_finalize_armed_at: None,
                last_inbound_pcm_at: None,
                utterance_media_ms: 0,
                defer_utterance_finalize_until_hold: false,
                barge_awaiting_stt_partial: false,
                stt_barge_fired_this_agent_playback: false,
                tts_playback_failure_gen: 0,
                tts_playback_last_error: None,
                pcm_writer: None,
                pcm_reader: None,
                denoise,
                stt_enabled: true,
                lid_last_emitted: None,
                pending_stt_config: None,
                pending_tts_config: None,
                utterance_replay_buffer: UtteranceReplayBuffer::new(),
                last_replay_snapshot: None,
                stt_hold: None,
                replay_begun_at_pre_roll: false,
                live: None,
                next_utterance_key: 1,
                closing: VecDeque::new(),
                lid_retained: VecDeque::new(),
                held_ignore_logged_key: None,
                audio_tap_dir_override: None,
                audio_tap_agent_id: 0,
                inbound_tap: None,
                tts_out_tap: None,
            })),
            stt_provider: std::sync::Mutex::new(stt),
            stt_actor: std::sync::Mutex::new(None),
            stt_pump: std::sync::Mutex::new(None),
            utterance_step: Mutex::new(()),
            stt_ready_wait_override_ms: AtomicU64::new(0),
            barge_in_flush_delay_ms: AtomicU64::new(0),
            language_id: Mutex::new(language_id),
            speaker_gate,
            speaker_embedder: std::sync::Mutex::new(speaker_embedder),
            tts: Arc::new(Mutex::new(tts)),
            tts_drain_wake: Arc::new(Notify::new()),
            tts_drain_worker: Arc::new(Mutex::new(None)),
            tts_synthesis_queue: Arc::new(Mutex::new(VecDeque::new())),
            tts_synthesis_wake: Arc::new(Notify::new()),
            tts_synthesis_worker: Arc::new(Mutex::new(None)),
            tts_synthesis_busy: Arc::new(AtomicBool::new(false)),
            lid_completion_notify: Arc::new(Notify::new()),
            lid_gate_bound_hits: Arc::new(AtomicUsize::new(0)),
            tts_synthesis_epoch: Arc::new(AtomicU64::new(0)),
            tts_generate_cancel: Arc::new(AtomicBool::new(false)),
            tts_protected_active: Arc::new(AtomicBool::new(false)),
            tts_job_dequeued: Arc::new(AtomicBool::new(false)),
            tts_workers_shutdown: Arc::new(AtomicBool::new(false)),
            tts_workers_shutdown_wake: Arc::new(Notify::new()),
            tts_shutdown_unhealthy: Arc::new(AtomicBool::new(false)),
            tts_worker_tasks_alive: Arc::new(AtomicUsize::new(0)),
            tts_vendor_calls_inflight: Arc::new(AtomicUsize::new(0)),
            weak_self: weak.clone(),
            c2_ticker_started: AtomicBool::new(false),
            inbound_in_flight: AtomicBool::new(false),
            running_flag: AtomicBool::new(false),
            epoch: Instant::now(),
            last_inbound_activity_ms: AtomicU64::new(0),
        }))
    }

    /// Number of live TTS synthesis/drain tokio tasks (0 when workers have exited).
    /// Does **not** include still-running `spawn_blocking` vendor work after abort.
    pub fn tts_worker_tasks_alive(&self) -> usize {
        self.tts_worker_tasks_alive.load(Ordering::SeqCst)
    }

    /// In-flight vendor `synthesize` calls observed by the agent (async layer).
    pub fn tts_vendor_calls_inflight(&self) -> usize {
        self.tts_vendor_calls_inflight.load(Ordering::SeqCst)
    }

    /// True when stop could not join a TTS worker within the bound (recycle signal).
    pub fn is_tts_shutdown_unhealthy(&self) -> bool {
        self.tts_shutdown_unhealthy.load(Ordering::SeqCst)
    }

    /// `std::sync::Mutex` poisoning cannot leave these slots inconsistent; take the value anyway.
    fn actor(&self) -> Option<Arc<SttActorHandle>> {
        self.stt_actor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(Arc::clone)
    }

    /// The active STT provider emits `SttTranscript::Partial` (`true` before the actor exists).
    /// Read at each decision so an `update_stt` swap takes effect mid-session.
    fn stt_emits_partials(&self) -> bool {
        self.actor().is_none_or(|actor| actor.emits_partials())
    }

    /// Queued audio plus the vendor's own decode backlog; `0` before the actor exists.
    fn stt_backlog_ms(&self) -> u32 {
        self.actor().map_or(0, |actor| actor.backlog_ms())
    }

    /// The vendor is still opening its stream (refused and retried).
    fn stt_open_pending(&self) -> bool {
        self.actor()
            .is_some_and(|actor| actor.status.open_pending.load(Ordering::SeqCst))
    }

    /// Hands audio to the STT actor. Never waits, never drops.
    fn send_stt_audio(&self, pcm: Bytes) {
        if let Some(actor) = self.actor() {
            actor.send_audio(pcm);
        }
    }

    /// When `false`, [`Self::process_inbound_pcm`] still runs VAD but skips STT and user speech events.
    pub async fn stt_enabled(&self) -> bool {
        self.inner.lock().await.stt_enabled
    }

    /// Enables or disables STT for inbound PCM. TTS is unchanged.
    pub async fn set_stt_enabled(&self, enabled: bool) {
        let _step = self.utterance_step.lock().await;
        let mut inner = self.inner.lock().await;
        inner.stt_enabled = enabled;
        if !enabled {
            Self::reset_stt_pipeline_state(&mut inner);
        }
    }

    /// Queue a new STT vendor config; applies after the current utterance finalizes (or immediately when idle).
    pub async fn update_stt_config(&self, config: SttConfig) -> SpeechResult<()> {
        let apply_now = {
            let mut inner = self.inner.lock().await;
            inner.pending_stt_config = Some(config);
            !Self::utterance_in_progress(&inner)
        };
        if apply_now {
            self.apply_pending_stt_config_if_any().await?;
        }
        Ok(())
    }

    /// Queue a new TTS vendor config; applies before the next synthesis job.
    pub async fn update_tts_config(
        &self,
        config: TtsConfig,
        options: UpdateTtsConfigOptions,
    ) -> SpeechResult<()> {
        if options.cancel_inflight {
            self.cancel_pending_tts_synthesis().await;
            self.flush_tts().await?;
        }
        self.inner.lock().await.pending_tts_config = Some(config);
        Ok(())
    }

    /// Override how long a replay waits for the new STT to become ready (default: env
    /// `SPEECH_STT_READY_WAIT_MS`, else 15 s). `0` restores the default.
    pub fn set_stt_ready_wait_ms(&self, ms: u64) {
        self.stt_ready_wait_override_ms.store(ms, Ordering::Relaxed);
    }

    /// Test-only: sleep this long between the barge-in TTS flush and the `barge_in` event, so a
    /// test can make the TTS drain observe the flush first. `None` (default) disables the delay.
    #[doc(hidden)]
    pub fn set_barge_in_flush_delay_for_test(&self, delay: Option<Duration>) {
        let ms = delay.map_or(0, |d| d.as_millis() as u64);
        self.barge_in_flush_delay_ms.store(ms, Ordering::Relaxed);
    }

    /// Test-only: write the listener's inbound audio and the STT input as WAV files into `dir`
    /// (see [`crate::audio_tap`]). Must be called before [`start`](Self::start).
    #[doc(hidden)]
    pub async fn set_audio_tap_dir_for_test(&self, dir: &'static Path) {
        self.inner.lock().await.audio_tap_dir_override = Some(dir);
    }

    fn stt_ready_wait_ms(&self) -> u64 {
        let over = self.stt_ready_wait_override_ms.load(Ordering::Relaxed);
        if over > 0 {
            return over;
        }
        std::env::var("SPEECH_STT_READY_WAIT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_STT_READY_WAIT_MS)
    }

    /// Wait for the (new) STT to be ready; on timeout log and continue (the replay still runs).
    /// The wait runs inside the STT actor, so live audio queues behind it and is not dropped.
    async fn wait_stt_ready(&self, actor: &SttActorHandle) -> SpeechResult<()> {
        let wait_ms = self.stt_ready_wait_ms();
        let (done, ready_rx) = oneshot::channel();
        if !actor.send(SttCommand::WaitReady {
            timeout: std::time::Duration::from_millis(wait_ms),
            done,
        }) {
            return Err(SpeechError::Stt("stt actor stopped".into()));
        }
        let ready = matches!(
            tokio::time::timeout(std::time::Duration::from_millis(wait_ms + 1_000), ready_rx).await,
            Ok(Ok(true))
        );
        if !ready {
            voice_debug(format!(
                "stt not ready after {wait_ms} ms; replaying anyway"
            ));
            eprintln!("[speech] stt not ready after {wait_ms} ms; replaying anyway");
        }
        Ok(())
    }

    /// Longest wait for a replay's own final after its audio and `EndUtterance` were sent.
    fn replay_final_wait(&self) -> std::time::Duration {
        std::time::Duration::from_millis(10_000 + self.stt_ready_wait_ms())
    }

    /// Re-feed the last utterance's post-RNNoise PCM into the current STT stream after a config swap.
    pub async fn replay_last_utterance(&self) -> SpeechResult<()> {
        let running = self.inner.lock().await.running;
        if !running {
            return Err(SpeechError::NotRunning);
        }
        {
            let inner = self.inner.lock().await;
            if inner.stt_hold.is_some() {
                return Err(SpeechError::Internal(
                    "cannot replay while an STT hold is active (use release_stt_hold)".into(),
                ));
            }
            if Self::utterance_in_progress(&inner) {
                return Err(SpeechError::Internal(
                    "cannot replay while an utterance is in progress".into(),
                ));
            }
        }
        self.apply_pending_stt_config_if_any().await?;

        let snapshot = {
            let inner = self.inner.lock().await;
            inner.last_replay_snapshot.clone()
        };
        let snapshot = snapshot.ok_or_else(|| {
            SpeechError::Internal("no buffered utterance available for replay".into())
        })?;
        let max_age_ms = {
            let inner = self.inner.lock().await;
            resolved_replay_max_age_ms(&inner.config.replay)
        };
        if !snapshot.replayable_within(max_age_ms) {
            return Err(SpeechError::Internal(
                "utterance replay unavailable (overflow, empty, or too old)".into(),
            ));
        }

        let replaces_id = snapshot.utterance_id.clone();
        let stt_configured = self.inner.lock().await.config.stt.is_some();
        let actor = match self.actor() {
            Some(actor) if stt_configured => actor,
            _ => return Err(SpeechError::Stt("STT not configured".into())),
        };
        self.wait_stt_ready(&actor).await?;

        // The replay record is queued before its audio, so results of the replayed audio
        // (which carry no key until the finalize is handled) are attributed to it.
        let final_rx = {
            let _step = self.utterance_step.lock().await;
            let mut inner = self.inner.lock().await;
            let mut rec = inner.new_record();
            rec.kind = UttKind::Replay {
                replaces: Some(replaces_id),
            };
            rec.phase = UttPhase::Closing;
            let (tx, rx) = oneshot::channel();
            rec.terminal_waiter = Some(tx);
            let key = rec.key;
            inner.closing.push_back(rec);
            for chunk in snapshot.pcm.chunks(640) {
                actor.send_audio(Bytes::copy_from_slice(chunk));
            }
            actor.send(SttCommand::EndUtterance { key });
            rx
        };
        match tokio::time::timeout(self.replay_final_wait(), final_rx).await {
            Ok(Ok(true)) => Ok(()),
            _ => Err(SpeechError::Internal(
                "replay did not produce an STT final".into(),
            )),
        }
    }

    /// Start a host STT hold (see [`crate::stt_hold`]).
    ///
    /// From now on inbound user PCM is no longer fed to the current STT and its results are
    /// ignored, so no final or partial from the old model is emitted after this call
    /// returns. VAD and `user_speaking_*` events keep flowing; the utterance in progress is seeded
    /// into the hold buffer (bounded by the 15 s utterance replay ring). Semantic STT-partial
    /// barge-in is inactive while held (VAD barge-in still works).
    pub async fn begin_stt_hold(&self, options: BeginSttHoldOptions) -> SpeechResult<()> {
        let max_ms = resolved_hold_max_buffer_ms(options.max_buffer_ms);
        let generation = self
            .actor()
            .map_or(0, |actor| actor.status.generation.load(Ordering::SeqCst));
        let buffered_ms = {
            let _step = self.utterance_step.lock().await;
            let mut inner = self.inner.lock().await;
            if !inner.running {
                return Err(SpeechError::NotRunning);
            }
            if inner.stt_hold.is_some() {
                return Err(SpeechError::Internal("STT hold already active".into()));
            }
            let open_utterance = inner.utterance_replay_buffer.is_active();
            let seed = if open_utterance {
                inner.utterance_replay_buffer.snapshot_pcm()
            } else {
                None
            };
            let mut hold = SttHold::new(options.mode, max_ms, seed.as_deref());
            hold.gen_at_begin = generation;
            let from_live = if open_utterance || Self::utterance_in_progress(&inner) {
                inner.live_id()
            } else {
                None
            };
            // An utterance already waiting for its final from the old model is the one the
            // replay will stand in for.
            hold.replaces_utterance_id = from_live.or_else(|| {
                inner
                    .closing
                    .iter()
                    .rev()
                    .find(|r| !r.terminal && r.kind == UttKind::Live)
                    .map(|r| r.id.clone())
            });
            let buffered_ms = hold.buffered_ms();
            inner.stt_hold = Some(hold);
            inner.held_ignore_logged_key = None;
            // Old-model text must not resurface; the last-utterance snapshot is stale after a hold.
            if let Some(live) = inner.live.as_mut() {
                live.last_partial_text = None;
            }
            inner.last_replay_snapshot = None;
            inner.utterance_replay_buffer.invalidate();
            buffered_ms
        };
        self.emit(SpeechEvent::stt_hold_started(
            options.mode.as_str(),
            buffered_ms,
        ));
        Ok(())
    }

    /// End the hold after the host swapped the STT (`update_stt_config`).
    ///
    /// `replay: true` decodes the held audio through the current (new) STT and emits finals with
    /// `replay: true` (and `replaces_utterance_id` when an original existed), then resumes live
    /// audio. The held audio is handed to the STT actor while live audio keeps being diverted
    /// into the same buffer and the hold is cleared atomically once the buffer is empty (under
    /// the agent state lock), so ordering is held audio first, then live, with no gap and no
    /// duplicate. `replay: false` drops the buffer.
    ///
    /// With `replay: true`, first waits up to `SPEECH_STT_READY_WAIT_MS` (default 15 s) for the new
    /// STT to be ready ([`SttProvider::wait_ready`]) before feeding the held audio.
    pub async fn release_stt_hold(&self, options: ReleaseSttHoldOptions) -> SpeechResult<()> {
        if self.inner.lock().await.stt_hold.is_none() {
            return Err(SpeechError::Internal("no STT hold active".into()));
        }
        // The host normally swapped already; apply a config still queued behind the open utterance.
        self.apply_pending_stt_config_if_any().await?;

        if !options.replay {
            let hold = self.inner.lock().await.stt_hold.take();
            if let Some(hold) = hold {
                self.emit(SpeechEvent::stt_hold_ended(
                    "released_drop",
                    hold.buffered_ms(),
                    hold.dropped_ms(),
                ));
            }
            return Ok(());
        }

        let stt_configured = self.inner.lock().await.config.stt.is_some();
        let actor = match self.actor() {
            Some(actor) if stt_configured => actor,
            _ => return Err(SpeechError::Stt("STT not configured".into())),
        };
        // The new STT may still be opening its stream (e.g. a speech backend that rejects new
        // streams right after it starts): replaying into it now would stall in finalize.
        self.wait_stt_ready(&actor).await?;

        let mut fed_bytes: usize = 0;
        let mut failed = false;
        let (replaces, dropped_ms, replay_rx) = {
            let _step = self.utterance_step.lock().await;
            let mut replay_rx = None;
            let (replaces, dropped_ms) = loop {
                let mut inner = self.inner.lock().await;
                let pcm = match inner.stt_hold.as_mut() {
                    Some(hold) => hold.take_pcm(),
                    None => {
                        return Err(SpeechError::Internal(
                            "STT hold ended during release".into(),
                        ))
                    }
                };
                if pcm.is_empty() {
                    let hold = inner.stt_hold.take().expect("hold present");
                    let live_open =
                        inner.live_flag(|r| r.stream_open) || Self::vad_is_speaking(&inner);
                    let replaces = hold.replaces_utterance_id.clone();
                    if live_open {
                        // Utterance still open: its final comes from the live path, flagged as replay.
                        // Its partial text is kept: `begin_stt_hold` already cleared the old
                        // model's text and old-model results are ignored while held, so any text
                        // here is the new model's (a result that raced this release under load
                        // used to be wiped, and the utterance ended in `user_stt_not_found`).
                        if let Some(live) = inner.live.as_mut() {
                            live.replay_ctx = Some(ReplayFinalContext {
                                replaces_utterance_id: replaces.clone(),
                                set_at: tokio::time::Instant::now(),
                            });
                        }
                    } else if fed_bytes > 0 && !failed {
                        // The held audio is already queued in the actor; this record claims the
                        // results that follow it and the finalize that ends it.
                        let mut rec = inner.new_record();
                        rec.kind = UttKind::Replay {
                            replaces: replaces.clone(),
                        };
                        rec.phase = UttPhase::Closing;
                        let (tx, rx) = oneshot::channel();
                        rec.terminal_waiter = Some(tx);
                        let key = rec.key;
                        inner.closing.push_back(rec);
                        actor.send(SttCommand::EndUtterance { key });
                        replay_rx = Some(rx);
                    }
                    break (replaces, hold.dropped_ms());
                }
                fed_bytes += pcm.len();
                // Sent while the state lock is held: live frames cannot slip in between.
                for chunk in pcm.chunks(640) {
                    if !actor.send_audio(Bytes::copy_from_slice(chunk)) {
                        failed = true;
                        break;
                    }
                }
                if failed {
                    let hold = inner.stt_hold.take();
                    let dropped = hold.map(|h| h.dropped_ms()).unwrap_or(0);
                    drop(inner);
                    self.emit(SpeechEvent::stt_hold_ended(
                        "failed",
                        crate::stt_hold::pcm_bytes_to_ms(fed_bytes),
                        dropped,
                    ));
                    return Err(SpeechError::Stt("stt actor stopped".into()));
                }
            };
            (replaces, dropped_ms, replay_rx)
        };
        let _ = replaces;

        if let Some(rx) = replay_rx {
            // The final (or the end of the record without one) is applied by the result pump.
            if tokio::time::timeout(self.replay_final_wait(), rx)
                .await
                .is_err()
            {
                eprintln!("[speech] held replay produced no final within the wait bound");
            }
        }
        self.emit(SpeechEvent::stt_hold_ended(
            "released_replay",
            crate::stt_hold::pcm_bytes_to_ms(fed_bytes),
            dropped_ms,
        ));
        Ok(())
    }

    /// Abort the hold: resume the old STT; buffered audio is fed to it in order (switch failed).
    /// Drops a still-queued STT config so the old STT stays in place.
    pub async fn cancel_stt_hold(&self) -> SpeechResult<()> {
        if self.inner.lock().await.stt_hold.is_none() {
            return Err(SpeechError::Internal("no STT hold active".into()));
        }
        self.inner.lock().await.pending_stt_config = None;

        let mut fed_bytes: usize = 0;
        let dropped_ms = {
            let _step = self.utterance_step.lock().await;
            loop {
                let mut inner = self.inner.lock().await;
                let pcm = match inner.stt_hold.as_mut() {
                    Some(hold) => hold.take_pcm(),
                    None => {
                        return Err(SpeechError::Internal("STT hold ended during cancel".into()))
                    }
                };
                if pcm.is_empty() {
                    let hold = inner.stt_hold.take().expect("hold present");
                    let live_open =
                        inner.live_flag(|r| r.stream_open) || Self::vad_is_speaking(&inner);
                    let finalize_needed = hold.finalize_seen && !live_open && fed_bytes > 0;
                    if finalize_needed {
                        // Utterances that ended while held: close them on the old STT through the
                        // normal path (its endpoint tail is already in the held audio).
                        let started =
                            self.begin_utterance_close(&mut inner, "end_of_utterance", false);
                        debug_assert!(!matches!(started, CloseStarted::Held));
                    }
                    break hold.dropped_ms();
                }
                if let Some(actor) = self.actor() {
                    for chunk in pcm.chunks(640) {
                        fed_bytes += chunk.len();
                        actor.send_audio(Bytes::copy_from_slice(chunk));
                    }
                }
            }
        };
        self.emit(SpeechEvent::stt_hold_ended(
            "cancelled",
            crate::stt_hold::pcm_bytes_to_ms(fed_bytes),
            dropped_ms,
        ));
        Ok(())
    }

    /// Stores the snapshot of the closing utterance for `replay_last_utterance`.
    fn commit_utterance_replay_snapshot(inner: &mut AgentInner, utterance_id: String) {
        inner.utterance_replay_buffer.end_utterance();
        let overflow = inner.utterance_replay_buffer.overflowed();
        let pcm = inner.utterance_replay_buffer.snapshot_pcm();
        inner.last_replay_snapshot = Some(match pcm {
            Some(pcm) => UtteranceReplaySnapshot {
                pcm,
                utterance_id,
                finalized_at: Instant::now(),
                overflow,
            },
            None => UtteranceReplaySnapshot {
                pcm: Bytes::new(),
                utterance_id,
                finalized_at: Instant::now(),
                overflow: true,
            },
        });
    }

    /// Sends the queued STT config to the actor (not awaited). `None` when nothing is queued or
    /// the actor does not exist yet.
    fn send_pending_swap(
        &self,
        inner: &mut AgentInner,
    ) -> Option<oneshot::Receiver<SpeechResult<()>>> {
        let actor = self.actor()?;
        let config = inner.pending_stt_config.take()?;
        let (done, rx) = oneshot::channel();
        let sent = actor.send(SttCommand::Swap {
            config,
            ctx: inner.otel.session_context.clone(),
            running: inner.running,
            done,
        });
        sent.then_some(rx)
    }

    /// Applies a queued STT config and waits (bounded) until the actor swapped the provider.
    async fn apply_pending_stt_config_if_any(&self) -> SpeechResult<()> {
        if self.actor().is_none() {
            // Not started yet: swap the provider the actor will receive at `start`.
            let config = self.inner.lock().await.pending_stt_config.take();
            let Some(config) = config else {
                return Ok(());
            };
            let session_ctx = self.inner.lock().await.otel.session_context.clone();
            let new_stt = self.registry.create_stt(&config)?;
            new_stt.bind_session_context(&session_ctx);
            *self.stt_provider.lock().unwrap_or_else(|e| e.into_inner()) = Some(new_stt);
            self.inner.lock().await.config.stt = Some(config.clone());
            self.emit(SpeechEvent::stt_config_updated(&config));
            return Ok(());
        }
        let swap_rx = {
            let _step = self.utterance_step.lock().await;
            let mut inner = self.inner.lock().await;
            self.send_pending_swap(&mut inner)
        };
        let Some(swap_rx) = swap_rx else {
            return Ok(());
        };
        match tokio::time::timeout(std::time::Duration::from_millis(30_000), swap_rx).await {
            Ok(Ok(result)) => result?,
            Ok(Err(_)) => {
                return Err(SpeechError::Stt(
                    "stt actor stopped before the config swap finished".into(),
                ))
            }
            Err(_) => return Err(SpeechError::Stt("stt config swap timed out".into())),
        }
        // `stt_config_updated` is emitted by the result pump; wait until it ran.
        let _ = self
            .wait_stt_idle(std::time::Duration::from_millis(5_000))
            .await;
        Ok(())
    }

    /// Waits until every STT command sent before this call was processed by the STT actor and
    /// its results were emitted.
    ///
    /// `process_inbound_pcm` no longer emits STT results inline; they are emitted by the agent's
    /// result pump. Waits until every STT command sent before this call was processed and its
    /// results emitted. Returns `false` when the timeout passes first (or the actor is gone).
    pub async fn wait_stt_idle(&self, timeout: std::time::Duration) -> bool {
        let Some(actor) = self.actor() else {
            return true;
        };
        let (done, rx) = oneshot::channel();
        if !actor.send(SttCommand::Barrier { done }) {
            return false;
        }
        matches!(tokio::time::timeout(timeout, rx).await, Ok(Ok(())))
    }

    /// Total bytes the STT actor pushed to the provider (tests).
    #[doc(hidden)]
    pub fn stt_audio_bytes_sent(&self) -> u64 {
        self.actor().map_or(0, |actor| {
            actor.status.audio_bytes_pushed.load(Ordering::SeqCst)
        })
    }

    async fn apply_pending_tts_config_if_any(&self) -> SpeechResult<()> {
        let config = self.inner.lock().await.pending_tts_config.take();
        let Some(config) = config else {
            return Ok(());
        };
        let session_ctx = self.inner.lock().await.otel.session_context.clone();
        let new_tts = wrap_tts_for_agent(self.registry.create_tts(&config)?, &config);
        new_tts.bind_session_context(&session_ctx);
        *self.tts.lock().await = Some(new_tts);
        {
            let mut inner = self.inner.lock().await;
            inner.config.tts = Some(config.clone());
        }
        self.emit(SpeechEvent::tts_config_updated(&config));
        Ok(())
    }

    fn utterance_in_progress(inner: &AgentInner) -> bool {
        // A pending finalize for an utterance whose final was already emitted can never complete
        // (should_finalize_utterance needs !stt_final_emitted); it is not an utterance in progress.
        // Records waiting for their `FinalizeDone` (`closing`) are not in progress.
        (inner.stt_finalize_pending && !inner.live_flag(|r| r.final_emitted))
            || inner.live_flag(|r| r.vad_triggered || r.stream_open)
            || inner.stt_gate_hold_ms > 0
            || inner.stt_endpoint_closing_started
    }

    fn stt_pipeline_active(inner: &AgentInner) -> bool {
        inner.stt_enabled && inner.config.stt.is_some()
    }

    fn reset_stt_pipeline_state(inner: &mut AgentInner) {
        inner.stt_pre_roll.as_mut().map(SttPreRollBuffer::clear);
        inner.short_burst.clear();
        inner.hold_pending_pcm.clear();
        inner.stt_gate_hold_ms = 0;
        inner.stt_finalize_pending = false;
        inner.stt_endpoint_closing_started = false;
        Self::clear_stt_listen_timer(inner);
        Self::clear_utterance_finalize_timer(inner);
        inner.barge_awaiting_stt_partial = false;
        inner.defer_utterance_finalize_until_hold = false;
        // STT is off: the open utterance and the ones still waiting for the vendor end without a
        // terminal event, and their late results are ignored.
        inner.live = None;
        inner.closing.clear();
    }

    fn clear_stt_listen_timer(inner: &mut AgentInner) {
        inner.stt_listen_deadline_ms = 0;
        inner.stt_listen_started_at = None;
        inner.stt_listen_media_ms = 0;
        inner.c1_backlog_deferred_logged = false;
        inner.stt_open_wait_started_at = None;
    }

    /// Hold C1 while the STT stream open is refused and retried (latency instead of failure).
    /// Returns `true` when C1 must not fire this tick. While holding, the listen clock restarts,
    /// so once the stream opens the normal timeout and hard timeout count from that point.
    fn c1_hold_for_stt_open(inner: &mut AgentInner, open_pending: bool) -> bool {
        if !open_pending {
            inner.stt_open_wait_started_at = None;
            return false;
        }
        let max_ms = inner.config.vad.stt_open_wait_max_ms;
        if max_ms == 0 {
            return false;
        }
        let first = inner.stt_open_wait_started_at.is_none();
        let since = *inner
            .stt_open_wait_started_at
            .get_or_insert_with(Instant::now);
        if first {
            voice_debug("C1 deferred: STT stream open pending");
        }
        if since.elapsed() < std::time::Duration::from_millis(max_ms as u64) {
            inner.stt_listen_started_at = Some(Instant::now());
            inner.stt_listen_media_ms = 0;
            return true;
        }
        false
    }

    /// C1 expires only when BOTH `stt_listen_media_ms` of audio reached the agent without a
    /// partial (late or slow audio is never cut off for being late) AND the same time passed on
    /// the wall clock (a burst of audio gives the STT no time to decode). Wall time alone backs
    /// it up only when inbound PCM has stopped completely (`pcm_idle` >= the timeout).
    /// While VAD is in speech only the hard timeout applies to the pure-stall case (a stalled reader
    /// is not a stopped sender).
    /// `pcm_idle` is `Duration::ZERO` when called from inside `process_inbound_pcm`.
    fn c1_listen_expired(
        inner: &AgentInner,
        backlog_ms: u32,
        pcm_idle: std::time::Duration,
    ) -> bool {
        if inner.stt_listen_deadline_ms == 0 {
            return false;
        }
        let Some(started_at) = inner.stt_listen_started_at else {
            return false;
        };
        let timeout_ms = inner.config.vad.stt_listen_timeout_ms as u64;
        let hard_timeout_ms = effective_stt_listen_hard_timeout_ms(&inner.config.vad);
        let slack = inner.config.vad.stt_listen_backlog_slack_ms;
        let media_ms = inner.stt_listen_media_ms as u64;
        let wall = started_at.elapsed();
        let wall_at_least = |ms: u64| wall >= std::time::Duration::from_millis(ms);
        if media_ms >= hard_timeout_ms && wall_at_least(hard_timeout_ms) {
            return true;
        }
        if media_ms >= timeout_ms && wall_at_least(timeout_ms) && backlog_ms <= slack {
            return true;
        }
        // Pure-stall safety net: no PCM at all for a full timeout. A frozen inbound reader (CPU-starved
        // host event loop) looks exactly like a stopped sender while its audio waits in the track
        // buffer, so the normal timeout never closes a stream while VAD is in speech (same rule as C2);
        // only the hard timeout bounds that case.
        let stopped = pcm_idle >= std::time::Duration::from_millis(timeout_ms);
        if stopped && wall_at_least(hard_timeout_ms) {
            return true;
        }
        stopped && !Self::vad_is_speaking(inner) && wall_at_least(timeout_ms) && backlog_ms <= slack
    }

    fn maybe_log_c1_backlog_deferred(inner: &mut AgentInner, backlog_ms: u32) {
        if inner.c1_backlog_deferred_logged || inner.stt_listen_started_at.is_none() {
            return;
        }
        let Some(started_at) = inner.stt_listen_started_at else {
            return;
        };
        let elapsed = started_at.elapsed();
        let timeout_ms = inner.config.vad.stt_listen_timeout_ms as u64;
        let hard_timeout_ms = effective_stt_listen_hard_timeout_ms(&inner.config.vad);
        let slack = inner.config.vad.stt_listen_backlog_slack_ms;
        if elapsed >= std::time::Duration::from_millis(timeout_ms)
            && elapsed < std::time::Duration::from_millis(hard_timeout_ms)
            && backlog_ms > slack
        {
            inner.c1_backlog_deferred_logged = true;
            voice_debug(format!(
                "C1 deferred: decode backlog {backlog_ms}ms > slack"
            ));
        }
    }

    fn clear_utterance_finalize_timer(inner: &mut AgentInner) {
        inner.utterance_media_ms = 0;
        inner.utterance_finalize_deadline_ms = 0;
        inner.utterance_finalize_armed_at = None;
        inner.defer_utterance_finalize_until_hold = false;
    }

    fn vad_is_speaking(inner: &AgentInner) -> bool {
        inner
            .vad
            .as_ref()
            .map(VadEngine::is_speaking)
            .unwrap_or(false)
    }

    /// C2 applies only after the user turn has ended: hold drained and VAD not in speech.
    fn c2_end_of_turn_ready(inner: &AgentInner) -> bool {
        if !inner.live_flag(|r| r.partials_emitted && !r.final_emitted) {
            return false;
        }
        if inner.defer_utterance_finalize_until_hold || inner.stt_gate_hold_ms > 0 {
            return false;
        }
        !Self::vad_is_speaking(inner)
    }

    fn disarm_utterance_finalize_timer(inner: &mut AgentInner) {
        inner.utterance_finalize_deadline_ms = 0;
        inner.utterance_finalize_armed_at = None;
    }

    fn arm_utterance_finalize_timer(inner: &mut AgentInner) {
        if !Self::c2_end_of_turn_ready(inner) {
            Self::disarm_utterance_finalize_timer(inner);
            return;
        }
        inner.utterance_finalize_deadline_ms = inner.config.vad.utterance_finalize_timeout_ms;
        inner.utterance_finalize_armed_at = Some(Instant::now());
        voice_debug(format!(
            "utterance finalize timer: {} ms (end-of-turn)",
            inner.utterance_finalize_deadline_ms
        ));
    }

    fn refresh_utterance_finalize_after_partial(inner: &mut AgentInner) {
        if Self::c2_end_of_turn_ready(inner) {
            Self::arm_utterance_finalize_timer(inner);
        } else {
            Self::disarm_utterance_finalize_timer(inner);
        }
    }

    fn stamp_inbound_activity(&self) {
        self.last_inbound_activity_ms
            .store(self.epoch.elapsed().as_millis() as u64, Ordering::SeqCst);
    }

    /// Wall time since inbound PCM was last entered or finished (never since a call started
    /// running long).
    fn inbound_idle(&self) -> std::time::Duration {
        let now = self.epoch.elapsed().as_millis() as u64;
        let last = self.last_inbound_activity_ms.load(Ordering::SeqCst);
        std::time::Duration::from_millis(now.saturating_sub(last))
    }

    fn ensure_c2_wall_clock_ticker(self: &Arc<Self>) {
        if self
            .c2_ticker_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        // Hold only a Weak: a strong ref here would keep the agent alive after the last
        // external owner drops it without stop(). `this` is loop-scoped and never held
        // across the sleep.
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let Some(this) = weak.upgrade() else {
                    break;
                };
                let running = {
                    let inner = this.inner.lock().await;
                    inner.running
                };
                if !running {
                    break;
                }
                if let Err(error) = this.c2_wall_clock_tick().await {
                    voice_debug(format!("C2 wall-clock tick error: {error}"));
                }
            }
        });
    }

    async fn c2_wall_clock_tick(&self) -> SpeechResult<()> {
        if self.inbound_in_flight.load(Ordering::SeqCst) {
            // A call is running (possibly slow). Its own media-time checks decide.
            return Ok(());
        }
        // One check at a time with inbound frames, pumped STT results and host APIs.
        let _step = self.utterance_step.lock().await;
        if self.inbound_in_flight.load(Ordering::SeqCst) {
            return Ok(());
        }
        let pcm_idle = self.inbound_idle();
        let backlog_ms = self.stt_backlog_ms();
        let open_pending = self.stt_open_pending();
        let c1_expired = {
            let mut inner = self.inner.lock().await;
            if !inner.running || !inner.config.vad.enabled || !Self::stt_pipeline_active(&inner) {
                false
            } else if inner.live_flag(|r| r.stream_open && !r.partials_emitted) {
                // C1 also waits while an earlier utterance is still waiting for its final
                // (rt.stt-terminal-before-late-final): bounded, every closing entry is answered
                // or timed out by the STT actor.
                let hold_for_vendor = open_pending || !inner.closing.is_empty();
                if Self::c1_hold_for_stt_open(&mut inner, hold_for_vendor) {
                    false
                } else if Self::c1_listen_expired(&inner, backlog_ms, pcm_idle) {
                    true
                } else {
                    Self::maybe_log_c1_backlog_deferred(&mut inner, backlog_ms);
                    false
                }
            } else {
                false
            }
        };
        if c1_expired {
            voice_debug("C1 wall-clock timeout (no STT partial)");
            finalize_trace("c1", &*self.inner.lock().await);
            self.close_stt_stream_not_found().await?;
        }

        let should_force = {
            let inner = self.inner.lock().await;
            if !inner.running || inner.utterance_finalize_deadline_ms == 0 {
                return Ok(());
            }
            if inner.defer_utterance_finalize_until_hold || inner.stt_gate_hold_ms > 0 {
                return Ok(());
            }
            if Self::vad_is_speaking(&inner) {
                return Ok(());
            }
            let Some(armed_at) = inner.utterance_finalize_armed_at else {
                return Ok(());
            };
            let timeout_ms = inner.config.vad.utterance_finalize_timeout_ms as u64;
            if armed_at.elapsed() < std::time::Duration::from_millis(timeout_ms) {
                return Ok(());
            }
            // Safety net when RTP/inbound loop stops: only force if PCM actually stalled.
            pcm_idle >= std::time::Duration::from_millis(timeout_ms)
        };
        if should_force {
            voice_debug("C2 wall-clock timeout (inbound PCM stalled after end-of-turn grace)");
            finalize_trace("wallclock_c2", &*self.inner.lock().await);
            self.force_close_utterance("pcm_stall").await?;
        }
        Ok(())
    }

    fn invalidate_inflight_tts_synthesis(&self) {
        self.tts_synthesis_epoch.fetch_add(1, Ordering::SeqCst);
        self.tts_generate_cancel.store(true, Ordering::SeqCst);
    }

    pub fn event_bus(&self) -> &SpeechEventBus {
        &self.event_bus
    }

    pub fn tts_buffer(&self) -> &TtsBuffer {
        &self.tts_buffer
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<SpeechEvent> {
        self.event_bus.subscribe()
    }

    pub fn events_mode(&self) -> EventDeliveryMode {
        self.inner
            .try_lock()
            .map(|inner| inner.config.events.mode)
            .unwrap_or(EventDeliveryMode::Both)
    }

    /// Registers inbound (user) and outbound (agent TTS) PCM callbacks. Required before [`start`](Self::start).
    pub async fn attach(&self, pcm_reader: PcmReader, pcm_writer: PcmWriter) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        inner.pcm_reader = Some(pcm_reader);
        inner.pcm_writer = Some(pcm_writer);
        inner.attached = true;
        Ok(())
    }

    /// True between a successful `start` and `stop`; cheap, lock-free.
    pub fn is_running(&self) -> bool {
        self.running_flag.load(Ordering::Acquire)
    }

    /// Starts STT vendor and TTS drain worker. Inbound PCM is driven by the host via [`process_inbound_pcm`](Self::process_inbound_pcm).
    ///
    /// Optional `session_context` carries session labels and W3C `traceparent` for OpenTelemetry
    /// when the `otel` Cargo feature is enabled.
    pub async fn start(&self, session_context: Option<VoiceSessionContext>) -> SpeechResult<()> {
        {
            let mut inner = self.inner.lock().await;
            if !inner.attached {
                return Err(SpeechError::NotAttached);
            }
            if inner.running {
                return Err(SpeechError::AlreadyRunning);
            }
            inner.running = true;
            self.running_flag.store(true, Ordering::Release);
            let stt_vendor = inner.config.stt.as_ref().map(|cfg| cfg.provider);
            let tts_vendor = inner.config.tts.as_ref().map(|cfg| cfg.provider);
            otel::begin_session(
                &mut inner.otel,
                session_context.unwrap_or_default(),
                stt_vendor,
                tts_vendor,
            );
        }

        voice_debug("VoiceAgent running=true");

        self.tts_workers_shutdown.store(false, Ordering::SeqCst);
        self.tts_shutdown_unhealthy.store(false, Ordering::SeqCst);

        let session_ctx = self.inner.lock().await.otel.session_context.clone();
        self.start_stt_actor(session_ctx.clone()).await?;
        if let Some(tts) = self.tts.lock().await.as_ref() {
            tts.bind_session_context(&session_ctx);
        }
        self.ensure_tts_drain_worker().await;
        self.start_speaker_gate();
        if let Some(this) = self.weak_self.upgrade() {
            this.ensure_c2_wall_clock_ticker();
        }
        Ok(())
    }

    /// Spawns the STT actor with the agent's provider, starts the provider through it and spawns
    /// the pump that applies the actor's results.
    async fn start_stt_actor(&self, session_ctx: VoiceSessionContext) -> SpeechResult<()> {
        let held = self
            .stt_provider
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let (provider, open_wait_max_ms, tap_dir, tap_agent_id, metric_attrs) = {
            let mut inner = self.inner.lock().await;
            let provider = match (held, inner.config.stt.as_ref()) {
                (Some(provider), _) => Some(provider),
                // Started again after `stop`: the actor took the previous provider with it.
                (None, Some(config)) => Some(self.registry.create_stt(config)?),
                (None, None) => None,
            };
            // Test-only audio tap: off unless a test or `SPEECH_TEST_AUDIO_TAP_DIR` turns it on.
            let tap_dir = inner.audio_tap_dir_override.or_else(audio_tap_dir_from_env);
            if let Some(dir) = tap_dir {
                let id = next_tap_agent_id();
                inner.audio_tap_agent_id = id;
                inner.inbound_tap = Some(ContinuousTap::new(dir, id));
                inner.tts_out_tap = Some(Arc::new(std::sync::Mutex::new(TtsOutTap::new(dir, id))));
                eprintln!("[speech] audio tap on: agent a{id} -> {}", dir.display());
            }
            (
                provider,
                inner.config.vad.stt_open_wait_max_ms as u64,
                tap_dir,
                inner.audio_tap_agent_id,
                inner.config.stt.as_ref().map(|config| {
                    SttMetricAttrs::from_stt_config(
                        config,
                        session_ctx.project_id.as_deref().unwrap_or(""),
                    )
                }),
            )
        };
        let metric_attrs = metric_attrs.unwrap_or_default();
        let vendor = provider.as_ref().map(|p| p.vendor_name());
        let (handle, results) = spawn_stt_actor(
            provider,
            Arc::clone(&self.registry),
            SttActorOptions {
                open_wait_max_ms,
                test_final_delay_ms: test_final_delay_ms_from_env(),
                audio_tap_dir: tap_dir,
                audio_tap_agent_id: tap_agent_id,
                metric_attrs,
                stt_latency_sink: default_stt_latency_sink(),
            },
        );
        let handle = Arc::new(handle);
        if let Some(previous) = self
            .stt_actor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(Arc::clone(&handle))
        {
            previous.join.abort();
        }
        // The pump holds only a Weak: an agent dropped without `stop()` is not kept alive by it,
        // and it ends when the actor (which ends when the agent's handle is dropped) closes the
        // result channel.
        let weak = self.weak_self.clone();
        let pump = tokio::spawn(async move {
            let mut results = results;
            while let Some(result) = results.recv().await {
                let Some(agent) = weak.upgrade() else {
                    break;
                };
                agent.handle_stt_result(result).await;
            }
        });
        if let Some(previous) = self
            .stt_pump
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(pump)
        {
            previous.abort();
        }

        let (done, started) = oneshot::channel();
        if !handle.send(SttCommand::Start {
            ctx: session_ctx,
            done,
        }) {
            return Err(SpeechError::Stt("stt actor stopped".into()));
        }
        match tokio::time::timeout(
            std::time::Duration::from_millis(STT_ACTOR_START_MS + 1_000),
            started,
        )
        .await
        {
            Ok(Ok(result)) => result?,
            Ok(Err(_)) => return Err(SpeechError::Stt("stt actor stopped".into())),
            Err(_) => return Err(SpeechError::Stt("stt start timed out".into())),
        }
        if let Some(vendor) = vendor {
            voice_debug(format!("STT started ({vendor})"));
        }
        Ok(())
    }

    /// Stops the STT actor (bounded) and the result pump. Utterances still waiting for their
    /// final get no terminal event: the session ends.
    async fn stop_stt_actor(&self) {
        let actor = self
            .stt_actor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(actor) = actor {
            let (done, stopped) = oneshot::channel();
            if actor.send(SttCommand::Stop { done })
                && tokio::time::timeout(
                    std::time::Duration::from_millis(STT_ACTOR_STOP_MS + 500),
                    stopped,
                )
                .await
                .is_err()
                && !actor.join.is_finished()
            {
                eprintln!("[speech] stt actor did not stop within 3500 ms; aborted");
                actor.join.abort();
            }
        }
        if let Some(pump) = self
            .stt_pump
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            pump.abort();
        }
        self.inner.lock().await.closing.clear();
    }

    async fn ensure_tts_drain_worker(&self) {
        Self::ensure_tts_drain_worker_shared(
            &self.tts_drain_worker,
            &self.tts_drain_wake,
            &self.tts_buffer,
            &self.inner,
            &self.event_bus,
            &self.tts_workers_shutdown,
            &self.tts_workers_shutdown_wake,
            &self.tts_worker_tasks_alive,
            &self.weak_self,
            &self.tts_synthesis_busy,
            &self.tts_synthesis_queue,
        )
        .await;
    }

    /// Wait until outbound TTS playback finishes (for tests and explicit synchronization).
    pub async fn wait_tts_playback_idle(&self) -> SpeechResult<()> {
        voice_debug(
            "wait_tts_playback_idle: waiting for synthesis queue, agent_speaking=false, TTS buffer drained",
        );
        let deadline = Instant::now() + std::time::Duration::from_secs(45);
        loop {
            let agent_speaking = self.inner.lock().await.agent_speaking;
            let queued = self.tts_buffer.is_speaking().await;
            let synth_pending = !self.tts_synthesis_queue.lock().await.is_empty()
                || self.tts_job_dequeued.load(Ordering::SeqCst)
                || self.tts_synthesis_busy.load(Ordering::SeqCst);
            if !agent_speaking && !queued && !synth_pending {
                voice_debug("wait_tts_playback_idle: playback idle");
                return Ok(());
            }
            if Instant::now() >= deadline {
                voice_debug(format!(
                    "wait_tts_playback_idle: TIMEOUT agent_speaking={agent_speaking} tts_queued={queued} synth_pending={synth_pending}"
                ));
                return Err(SpeechError::Internal(
                    "timed out waiting for TTS playback to finish".into(),
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    pub async fn stop(&self) -> SpeechResult<()> {
        let was_running = {
            let mut inner = self.inner.lock().await;
            if !inner.running {
                // Idempotent diagnostic: prior unhealthy stop remains visible.
                if self.tts_shutdown_unhealthy.load(Ordering::SeqCst) {
                    return Err(SpeechError::TtsShutdownUnhealthy);
                }
                return Err(SpeechError::NotRunning);
            }
            inner.running = false;
            self.running_flag.store(false, Ordering::Release);
            inner.stt_hold = None;
            if let Some(tap) = inner.inbound_tap.as_mut() {
                tap.flush_segment();
            }
            inner.inbound_tap = None;
            if let Some(tap) = inner.tts_out_tap.take() {
                tap.lock().unwrap_or_else(|e| e.into_inner()).finish(true);
            }
            otel::end_session(&mut inner.otel);
            true
        };
        let _ = was_running;

        // Cancel producers, wake consumers, join workers within a bound (no detached tasks).
        self.tts_workers_shutdown.store(true, Ordering::SeqCst);
        let dropped = self.cancel_pending_tts_synthesis_collect().await;
        self.tts_buffer
            .flush_with_reply_events(|event| self.emit(event))
            .await;
        self.emit_dropped_reply_ends(dropped);
        self.tts_synthesis_wake.notify_waiters();
        self.tts_drain_wake.notify_waiters();
        self.tts_workers_shutdown_wake.notify_waiters();

        let synth = self.tts_synthesis_worker.lock().await.take();
        let drain = self.tts_drain_worker.lock().await.take();
        // Join in parallel so drain pacing cannot stall synthesis join (and vice versa).
        let (synth_join, drain_join) = tokio::join!(
            Self::join_tts_worker_bounded(synth, "synthesis", &self.tts_shutdown_unhealthy),
            Self::join_tts_worker_bounded(drain, "drain", &self.tts_shutdown_unhealthy),
        );
        let _ = (synth_join, drain_join);

        {
            let deferred_keys = self.inner.lock().await.deferred_lid_keys();
            for key in deferred_keys {
                self.spawn_identify_language(key, true).await;
            }
        }

        self.stop_stt_actor().await;

        voice_debug(format!(
            "VoiceAgent stopped workers_alive={} vendor_inflight={} unhealthy={}",
            self.tts_worker_tasks_alive(),
            self.tts_vendor_calls_inflight(),
            self.is_tts_shutdown_unhealthy()
        ));

        // Prefer recycle signal over STT stop errors so the JS host quarantines.
        if self.tts_shutdown_unhealthy.load(Ordering::SeqCst) {
            return Err(SpeechError::TtsShutdownUnhealthy);
        }
        Ok(())
    }

    async fn join_tts_worker_bounded(
        handle: Option<tokio::task::JoinHandle<()>>,
        name: &str,
        unhealthy: &AtomicBool,
    ) {
        let Some(mut handle) = handle else {
            return;
        };
        const JOIN_BOUND: std::time::Duration = std::time::Duration::from_millis(2_000);
        const ABORT_BOUND: std::time::Duration = std::time::Duration::from_millis(500);
        tokio::select! {
            result = &mut handle => {
                if let Err(err) = result {
                    voice_debug(format!("TTS {name} worker join error: {err}"));
                }
            }
            _ = tokio::time::sleep(JOIN_BOUND) => {
                handle.abort();
                // Never wait unboundedly on the aborted task: a worker parked inside a blocking
                // section (the NAPI PCM writer uses `block_in_place` + `block_on(write_sample)`,
                // which stalls when nothing consumes the outbound track) cannot be cancelled until
                // that call returns, so `handle.await` here would hang `stop()` with it.
                let _ = tokio::time::timeout(ABORT_BOUND, &mut handle).await;
                unhealthy.store(true, Ordering::SeqCst);
                voice_debug(format!(
                    "TTS {name} worker join timed out after {}ms — marked shutdown unhealthy (recycle)",
                    JOIN_BOUND.as_millis()
                ));
            }
        }
    }

    /// Synthesizes text and enqueues stereo 48 kHz PCM for real-time outbound drain.
    ///
    /// Default ([`SendTextToTtsOptions::default`]) waits until synthesis and playback for this
    /// utterance finish. Pass `non_blocking: true` to return once the job is queued.
    pub async fn send_text_to_tts(&self, text: &str) -> SpeechResult<()> {
        self.send_text_to_tts_with_options(text, SendTextToTtsOptions::default())
            .await
    }

    /// Like [`send_text_to_tts`](Self::send_text_to_tts) with explicit queue / wait behavior.
    pub async fn send_text_to_tts_with_options(
        &self,
        text: &str,
        options: SendTextToTtsOptions,
    ) -> SpeechResult<()> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        {
            let tts = self.tts.lock().await;
            if tts.is_none() {
                return Err(SpeechError::Config("TTS not configured".into()));
            }
        }

        let (done_tx, done_rx) = if options.non_blocking {
            (None, None)
        } else {
            let (tx, rx) = tokio::sync::oneshot::channel();
            (Some(tx), Some(rx))
        };

        {
            let mut queue = self.tts_synthesis_queue.lock().await;
            queue.push_back(TtsSynthesisJob {
                text: trimmed.to_string(),
                done: done_tx,
                interruptible: options.interruptible,
                requested_at: Instant::now(),
                reply_id: options.reply_id.as_deref().map(Arc::from),
            });
        }

        self.ensure_tts_synthesis_worker().await;
        self.tts_synthesis_wake.notify_one();

        if let Some(rx) = done_rx {
            rx.await
                .map_err(|_| SpeechError::Internal("TTS job cancelled".into()))??;
        }

        Ok(())
    }

    /// Fault-path LID gate bound hit count (mock/tests).
    #[doc(hidden)]
    pub fn lid_gate_bound_hits(&self) -> usize {
        self.lid_gate_bound_hits.load(Ordering::SeqCst)
    }

    /// LID PCM buffer length in bytes (tests).
    #[doc(hidden)]
    pub async fn lid_buffer_len(&self) -> usize {
        self.inner
            .lock()
            .await
            .live
            .as_ref()
            .map_or(0, |r| r.lid.pcm_buffer.len())
    }

    async fn ensure_tts_synthesis_worker(&self) {
        if self.tts_workers_shutdown.load(Ordering::SeqCst) {
            return;
        }
        let mut slot = self.tts_synthesis_worker.lock().await;
        if slot.is_some() {
            return;
        }
        let wake = Arc::clone(&self.tts_synthesis_wake);
        let queue = Arc::clone(&self.tts_synthesis_queue);
        let tts = Arc::clone(&self.tts);
        let tts_buffer = self.tts_buffer.clone();
        let tts_drain_wake = Arc::clone(&self.tts_drain_wake);
        let tts_drain_worker = Arc::clone(&self.tts_drain_worker);
        let inner = Arc::clone(&self.inner);
        let event_bus = self.event_bus.clone();
        let synthesis_busy = Arc::clone(&self.tts_synthesis_busy);
        let synthesis_epoch = Arc::clone(&self.tts_synthesis_epoch);
        let generate_cancel = Arc::clone(&self.tts_generate_cancel);
        let protected_active = Arc::clone(&self.tts_protected_active);
        let job_dequeued = Arc::clone(&self.tts_job_dequeued);
        let shutdown = Arc::clone(&self.tts_workers_shutdown);
        let shutdown_wake = Arc::clone(&self.tts_workers_shutdown_wake);
        let alive = Arc::clone(&self.tts_worker_tasks_alive);
        let vendor_inflight = Arc::clone(&self.tts_vendor_calls_inflight);
        let drain_shutdown = Arc::clone(&self.tts_workers_shutdown);
        let drain_alive = Arc::clone(&self.tts_worker_tasks_alive);
        let weak_self = self.weak_self.clone();
        let lid_notify = Arc::clone(&self.lid_completion_notify);
        *slot = Some(tokio::spawn(async move {
            let _alive_guard = TtsWorkerAliveGuard::enter(&alive);
            loop {
                // Level-triggered shutdown poll: notify_waiters is edge-triggered and
                // can be lost while the worker is inside a vendor call / drain pass.
                tokio::select! {
                    _ = wake.notified() => {}
                    _ = shutdown_wake.notified() => {}
                    _ = async {
                        // Also ends when the agent was dropped without stop(): this task
                        // owns the TTS buffer (and the PCM writer), so it must not outlive it.
                        while !shutdown.load(Ordering::SeqCst) && weak_self.strong_count() > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                    } => {}
                }
                if shutdown.load(Ordering::SeqCst) || weak_self.strong_count() == 0 {
                    break;
                }
                loop {
                    if shutdown.load(Ordering::SeqCst) || weak_self.strong_count() == 0 {
                        break;
                    }
                    let job = {
                        let mut pending = queue.lock().await;
                        let job = pending.pop_front();
                        // Mark while still holding the queue lock so idle checks never see the
                        // job in neither the queue nor a busy/dequeued state.
                        if job.is_some() {
                            job_dequeued.store(true, Ordering::SeqCst);
                        }
                        job
                    };
                    let Some(job) = job else {
                        break;
                    };

                    Self::await_in_flight_lid_before_tts(&weak_self, &inner, &lid_notify).await;

                    synthesis_busy.store(true, Ordering::SeqCst);
                    job_dequeued.store(false, Ordering::SeqCst);
                    protected_active.store(!job.interruptible, Ordering::SeqCst);
                    let result = Self::run_tts_synthesis_job(
                        &job.text,
                        job.requested_at,
                        job.reply_id.as_ref(),
                        &tts,
                        &tts_buffer,
                        &tts_drain_wake,
                        &tts_drain_worker,
                        &inner,
                        &event_bus,
                        &synthesis_epoch,
                        &generate_cancel,
                        &drain_shutdown,
                        &shutdown_wake,
                        &drain_alive,
                        &vendor_inflight,
                        &weak_self,
                        &synthesis_busy,
                        &queue,
                    )
                    .await;
                    protected_active.store(false, Ordering::SeqCst);
                    synthesis_busy.store(false, Ordering::SeqCst);
                    Self::maybe_spawn_deferred_lid_after_tts(&weak_self, &queue, &inner).await;

                    if let Some(done) = job.done {
                        let _ = done.send(result);
                    } else if let Err(error) = result {
                        voice_debug(format!("non-blocking TTS synthesis error: {error}"));
                    }
                }
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
            }
            // Cancel any leftovers so waiters are not stranded.
            let mut pending = queue.lock().await;
            for job in pending.drain(..) {
                if let Some(done) = job.done {
                    let _ = done.send(Err(SpeechError::Internal("TTS cancelled".into())));
                }
            }
        }));
    }

    #[allow(clippy::too_many_arguments)] // shared handles of the TTS worker, kept as plain params (hot path)
    async fn run_tts_synthesis_job(
        text: &str,
        requested_at: Instant,
        reply_id: Option<&Arc<str>>,
        tts: &Arc<Mutex<Option<Box<dyn TtsProvider>>>>,
        tts_buffer: &TtsBuffer,
        tts_drain_wake: &Arc<Notify>,
        tts_drain_worker: &Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        synthesis_epoch: &Arc<AtomicU64>,
        generate_cancel: &Arc<AtomicBool>,
        tts_workers_shutdown: &Arc<AtomicBool>,
        tts_workers_shutdown_wake: &Arc<Notify>,
        tts_worker_tasks_alive: &Arc<AtomicUsize>,
        tts_vendor_calls_inflight: &Arc<AtomicUsize>,
        weak_self: &Weak<VoiceAgent>,
        tts_synthesis_busy: &Arc<AtomicBool>,
        tts_synthesis_queue: &Arc<Mutex<VecDeque<TtsSynthesisJob>>>,
    ) -> SpeechResult<()> {
        if let Some(agent) = weak_self.upgrade() {
            agent.apply_pending_tts_config_if_any().await?;
        }
        Self::begin_first_audio_timing(inner, requested_at, text).await;
        if tts_stream_chunks_enabled() {
            Self::run_tts_synthesis_job_streaming(
                text,
                requested_at,
                reply_id,
                tts,
                tts_buffer,
                tts_drain_wake,
                tts_drain_worker,
                inner,
                event_bus,
                synthesis_epoch,
                generate_cancel,
                tts_workers_shutdown,
                tts_workers_shutdown_wake,
                tts_worker_tasks_alive,
                tts_vendor_calls_inflight,
                weak_self,
                tts_synthesis_busy,
                tts_synthesis_queue,
            )
            .await
        } else {
            Self::run_tts_synthesis_job_buffered(
                text,
                requested_at,
                reply_id,
                tts,
                tts_buffer,
                tts_drain_wake,
                tts_drain_worker,
                inner,
                event_bus,
                synthesis_epoch,
                tts_workers_shutdown,
                tts_workers_shutdown_wake,
                tts_worker_tasks_alive,
                tts_vendor_calls_inflight,
                weak_self,
                tts_synthesis_busy,
                tts_synthesis_queue,
            )
            .await
        }
    }

    /// Legacy path: fully synthesize, then enqueue all PCM, then drain.
    /// Enabled with `VOICE_TTS_STREAM_CHUNKS=0`.
    #[allow(clippy::too_many_arguments)] // shared handles of the TTS worker, kept as plain params (hot path)
    async fn run_tts_synthesis_job_buffered(
        text: &str,
        requested_at: Instant,
        reply_id: Option<&Arc<str>>,
        tts: &Arc<Mutex<Option<Box<dyn TtsProvider>>>>,
        tts_buffer: &TtsBuffer,
        tts_drain_wake: &Arc<Notify>,
        tts_drain_worker: &Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        synthesis_epoch: &Arc<AtomicU64>,
        tts_workers_shutdown: &Arc<AtomicBool>,
        tts_workers_shutdown_wake: &Arc<Notify>,
        tts_worker_tasks_alive: &Arc<AtomicUsize>,
        tts_vendor_calls_inflight: &Arc<AtomicUsize>,
        weak_self: &Weak<VoiceAgent>,
        tts_synthesis_busy: &Arc<AtomicBool>,
        tts_synthesis_queue: &Arc<Mutex<VecDeque<TtsSynthesisJob>>>,
    ) -> SpeechResult<()> {
        let epoch_at_start = synthesis_epoch.load(Ordering::SeqCst);
        let generation_at_start = tts_buffer.current_generation().await;
        let tts_started = Instant::now();
        let chunks = {
            let tts_guard = tts.lock().await;
            let provider = tts_guard
                .as_ref()
                .ok_or_else(|| SpeechError::Config("TTS not configured".into()))?;
            let (ctx, tts_vendor) = {
                let inner_guard = inner.lock().await;
                (
                    inner_guard.otel.session_context.clone(),
                    inner_guard.config.tts.as_ref().map(|cfg| cfg.provider),
                )
            };
            {
                let _span = otel::voice_span(
                    "voice.tts",
                    &ctx,
                    tts_vendor.map(crate::config::TtsVendor::as_str),
                );
            }
            tts_vendor_calls_inflight.fetch_add(1, Ordering::SeqCst);
            let synthesize_result = provider.synthesize(text).await;
            tts_vendor_calls_inflight.fetch_sub(1, Ordering::SeqCst);
            let cache_hit = if synthesize_result.is_ok() {
                provider.take_cache_hit()
            } else {
                None
            };
            (synthesize_result, provider.take_open_wait(), cache_hit)
        };
        let (chunks, open_wait, cache_hit) = chunks;
        let reply_str = reply_id.map(|id| &**id);
        Self::emit_tts_wait(event_bus, open_wait, reply_str);
        Self::emit_tts_phrase_cache_hit(event_bus, cache_hit, reply_str);
        Self::record_tts_job_latency(inner, tts_started).await;
        let chunks = match chunks {
            Ok(chunks) => chunks,
            Err(error) => {
                Self::fail_reply_keep_session(inner, event_bus, &error, reply_str).await;
                return Ok(());
            }
        };
        if chunks.iter().any(|chunk| !chunk.pcm.is_empty()) {
            Self::record_first_chunk(inner, requested_at).await;
        }

        if synthesis_epoch.load(Ordering::SeqCst) != epoch_at_start {
            voice_debug("TTS synthesis discarded (invalidated during synthesize)");
            Self::emit_discarded_reply_end(event_bus, reply_str);
            return Ok(());
        }

        if chunks.is_empty() {
            return Ok(());
        }

        let failure_gen_at_start = {
            let guard = inner.lock().await;
            guard.tts_playback_failure_gen
        };

        // The whole reply is enqueued at once, so it is complete as soon as it is booked.
        let first_chunk_ms = Self::elapsed_ms_u32(requested_at);
        if !tts_buffer
            .enqueue_tagged_if_generation(
                chunks,
                Some(generation_at_start),
                reply_id.map(|id| (id, Some(first_chunk_ms))),
            )
            .await
        {
            voice_debug("TTS synthesis discarded (buffer flushed during synthesize)");
            Self::emit_discarded_reply_end(event_bus, reply_str);
            return Ok(());
        }
        if let Some(id) = reply_str {
            tts_buffer.mark_reply_complete(id).await;
        }
        Self::ensure_tts_drain_worker_shared(
            tts_drain_worker,
            tts_drain_wake,
            tts_buffer,
            inner,
            event_bus,
            tts_workers_shutdown,
            tts_workers_shutdown_wake,
            tts_worker_tasks_alive,
            weak_self,
            tts_synthesis_busy,
            tts_synthesis_queue,
        )
        .await;
        tts_drain_wake.notify_one();
        Self::wait_job_playback_idle(
            tts_buffer,
            inner,
            event_bus,
            weak_self,
            failure_gen_at_start,
        )
        .await
    }

    /// Default path: stream PCM chunks into the buffer while synthesis runs so
    /// drain can start before the full utterance is ready (`VOICE_TTS_STREAM_CHUNKS`).
    #[allow(clippy::too_many_arguments)] // shared handles of the TTS worker, kept as plain params (hot path)
    async fn run_tts_synthesis_job_streaming(
        text: &str,
        requested_at: Instant,
        reply_id: Option<&Arc<str>>,
        tts: &Arc<Mutex<Option<Box<dyn TtsProvider>>>>,
        tts_buffer: &TtsBuffer,
        tts_drain_wake: &Arc<Notify>,
        tts_drain_worker: &Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        synthesis_epoch: &Arc<AtomicU64>,
        generate_cancel: &Arc<AtomicBool>,
        tts_workers_shutdown: &Arc<AtomicBool>,
        tts_workers_shutdown_wake: &Arc<Notify>,
        tts_worker_tasks_alive: &Arc<AtomicUsize>,
        tts_vendor_calls_inflight: &Arc<AtomicUsize>,
        weak_self: &Weak<VoiceAgent>,
        tts_synthesis_busy: &Arc<AtomicBool>,
        tts_synthesis_queue: &Arc<Mutex<VecDeque<TtsSynthesisJob>>>,
    ) -> SpeechResult<()> {
        let epoch_at_start = synthesis_epoch.load(Ordering::SeqCst);
        let generation_at_start = tts_buffer.current_generation().await;
        // Capture before drain can abort this job's PCM (streaming starts drain early).
        let failure_gen_at_start = {
            let guard = inner.lock().await;
            guard.tts_playback_failure_gen
        };
        generate_cancel.store(false, Ordering::SeqCst);

        Self::ensure_tts_drain_worker_shared(
            tts_drain_worker,
            tts_drain_wake,
            tts_buffer,
            inner,
            event_bus,
            tts_workers_shutdown,
            tts_workers_shutdown_wake,
            tts_worker_tasks_alive,
            weak_self,
            tts_synthesis_busy,
            tts_synthesis_queue,
        )
        .await;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TtsProgressiveSink {
            tx,
            cancel: Arc::clone(generate_cancel),
        };

        tts_buffer.set_producing(true).await;
        tts_drain_wake.notify_one();

        let enqueue_buffer = tts_buffer.clone();
        let enqueue_wake = Arc::clone(tts_drain_wake);
        let enqueue_inner = Arc::clone(inner);
        let enqueue_reply_id: Option<Arc<str>> = reply_id.cloned();
        // Returns (a chunk of this reply was booked, a chunk was discarded by a flush).
        // Audio held back before playback starts, for vendors that stream in bursts with idle
        // gaps (`TtsProvider::playout_prebuffer_ms`). 0 enqueues every chunk as it arrives.
        let prebuffer_bytes = {
            let guard = tts.lock().await;
            guard
                .as_ref()
                .map_or(0, |provider| provider.playout_prebuffer_ms() as usize)
                * 192 // 48 kHz stereo s16le bytes per ms
        };
        let enqueue_task = tokio::spawn(async move {
            let mut first_chunk_seen = false;
            let mut first_chunk_ms = None;
            let mut queued_any = false;
            let mut held: Vec<TtsAudioChunk> = Vec::new();
            let mut held_bytes = 0usize;
            let mut released = false;
            loop {
                let batch = match rx.recv().await {
                    Some(chunk) if released => vec![chunk],
                    Some(chunk) => {
                        held_bytes += chunk.pcm.len();
                        held.push(chunk);
                        if held_bytes < prebuffer_bytes {
                            continue;
                        }
                        released = true;
                        std::mem::take(&mut held)
                    }
                    // End of stream: a reply shorter than the prebuffer plays now.
                    None if !released && !held.is_empty() => std::mem::take(&mut held),
                    None => break,
                };
                if !first_chunk_seen && batch.iter().any(|chunk| !chunk.pcm.is_empty()) {
                    first_chunk_seen = true;
                    first_chunk_ms = Some(Self::elapsed_ms_u32(requested_at));
                    Self::record_first_chunk(&enqueue_inner, requested_at).await;
                }
                if !enqueue_buffer
                    .enqueue_tagged_if_generation(
                        batch,
                        Some(generation_at_start),
                        enqueue_reply_id.as_ref().map(|id| (id, first_chunk_ms)),
                    )
                    .await
                {
                    voice_debug("TTS progressive chunk discarded (buffer flushed)");
                    return (queued_any, true);
                }
                queued_any = true;
                enqueue_wake.notify_one();
                if !released {
                    // The stream ended while holding: nothing more will arrive.
                    break;
                }
            }
            (queued_any, false)
        });

        let tts_started = Instant::now();
        let synth_result = {
            let tts_guard = tts.lock().await;
            let provider = tts_guard
                .as_ref()
                .ok_or_else(|| SpeechError::Config("TTS not configured".into()))?;
            let (ctx, tts_vendor) = {
                let inner_guard = inner.lock().await;
                (
                    inner_guard.otel.session_context.clone(),
                    inner_guard.config.tts.as_ref().map(|cfg| cfg.provider),
                )
            };
            {
                let _span = otel::voice_span(
                    "voice.tts",
                    &ctx,
                    tts_vendor.map(crate::config::TtsVendor::as_str),
                );
            }
            tts_vendor_calls_inflight.fetch_add(1, Ordering::SeqCst);
            let result = provider.synthesize_progressive(text, Some(sink)).await;
            tts_vendor_calls_inflight.fetch_sub(1, Ordering::SeqCst);
            let cache_hit = if result.is_ok() {
                provider.take_cache_hit()
            } else {
                None
            };
            (result, provider.take_open_wait(), cache_hit)
        };
        let (synth_result, open_wait, cache_hit) = synth_result;
        let reply_str = reply_id.map(|id| &**id);
        Self::emit_tts_wait(event_bus, open_wait, reply_str);
        Self::emit_tts_phrase_cache_hit(event_bus, cache_hit, reply_str);
        Self::record_tts_job_latency(inner, tts_started).await;

        // Dropping the last sender happens when synthesize_progressive returns
        // (sink moved into the call). Wait for the enqueue task to drain.
        let (queued_any, chunk_discarded) = enqueue_task.await.unwrap_or((false, false));
        // Mark the reply complete before the drain can run out of bytes: the drain reports
        // the end once a complete reply is fully played.
        if let Some(id) = reply_str {
            tts_buffer.mark_reply_complete(id).await;
        }
        tts_buffer.set_producing(false).await;
        tts_drain_wake.notify_one();

        match synth_result {
            Ok(chunks) => {
                if synthesis_epoch.load(Ordering::SeqCst) != epoch_at_start {
                    voice_debug("TTS streaming synthesis discarded (invalidated)");
                    if !queued_any {
                        Self::emit_discarded_reply_end(event_bus, reply_str);
                    }
                    return Ok(());
                }
                if chunk_discarded && !queued_any {
                    Self::emit_discarded_reply_end(event_bus, reply_str);
                }
                if chunks.is_empty()
                    && !tts_buffer.is_speaking().await
                    && tts_buffer.pending_count().await == 0
                {
                    return Ok(());
                }
                Self::wait_job_playback_idle(
                    tts_buffer,
                    inner,
                    event_bus,
                    weak_self,
                    failure_gen_at_start,
                )
                .await
            }
            Err(error) => {
                tts_buffer.set_producing(false).await;
                tts_drain_wake.notify_one();
                Self::fail_reply_keep_session(inner, event_bus, &error, reply_str).await;
                Ok(())
            }
        }
    }

    fn emit_tts_wait(
        event_bus: &SpeechEventBus,
        open_wait: Option<crate::pipeline::TtsOpenWait>,
        reply_id: Option<&str>,
    ) {
        if let Some(wait) = open_wait {
            event_bus.emit(
                SpeechEvent::tts_wait(wait.wait_ms, wait.attempts, wait.reason)
                    .with_reply_id(reply_id),
            );
        }
    }

    fn emit_tts_phrase_cache_hit(
        event_bus: &SpeechEventBus,
        cache_hit: Option<u32>,
        reply_id: Option<&str>,
    ) {
        if let Some(chars) = cache_hit {
            event_bus.emit(SpeechEvent::tts_phrase_cache_hit(chars).with_reply_id(reply_id));
        }
    }

    /// A reply that never reached the buffer (flushed or invalidated while synthesizing, or
    /// dropped from the synthesis queue) ends with nothing played. Only for jobs with a reply id.
    fn emit_discarded_reply_end(event_bus: &SpeechEventBus, reply_id: Option<&str>) {
        if let Some(id) = reply_id {
            event_bus.emit(SpeechEvent::agent_reply_end(id, 0, true, 0, false));
        }
    }

    /// Emit the reply boundaries crossed by written PCM (see [`TtsBuffer::consume_played`]).
    fn emit_reply_transitions(event_bus: &SpeechEventBus, transitions: Vec<ReplyTransition>) {
        for transition in transitions {
            match transition {
                ReplyTransition::Started { id, first_chunk_ms } => {
                    event_bus.emit(SpeechEvent::agent_reply_start(&id, first_chunk_ms));
                }
                ReplyTransition::Ended {
                    id,
                    played_ms,
                    synthesized_ms,
                } => {
                    event_bus.emit(SpeechEvent::agent_reply_end(
                        &id,
                        played_ms,
                        false,
                        synthesized_ms,
                        true,
                    ));
                }
            }
        }
    }

    /// A reply failed to synthesize. Only this reply ends: emit `agent_speak_failed` and leave
    /// the agent listening. The caller of `send_text_to_tts` gets `Ok`, so a host does not treat
    /// the failure as session-fatal.
    async fn fail_reply_keep_session(
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        error: &SpeechError,
        reply_id: Option<&str>,
    ) {
        {
            let mut guard = inner.lock().await;
            Self::clear_pending_first_audio(&mut guard);
        }
        voice_debug(format!("TTS reply failed, session kept: {error}"));
        event_bus.emit(SpeechEvent::agent_speak_failed(error.to_string()).with_reply_id(reply_id));
    }

    fn elapsed_ms_u32(since: Instant) -> u32 {
        u32::try_from(since.elapsed().as_millis()).unwrap_or(u32::MAX)
    }

    fn project_id_label(inner: &AgentInner) -> &str {
        inner
            .otel
            .session_context
            .project_id
            .as_deref()
            .unwrap_or("")
    }

    fn active_stt_vendor_label(inner: &AgentInner) -> &'static str {
        inner
            .config
            .stt
            .as_ref()
            .map(|cfg| cfg.provider.as_str())
            .unwrap_or("unknown")
    }

    fn active_tts_vendor_label(inner: &AgentInner) -> &'static str {
        inner
            .config
            .tts
            .as_ref()
            .map(|cfg| cfg.provider.as_str())
            .unwrap_or("unknown")
    }

    /// A reply queued while the agent is already speaking produces no new `agent_speaking_start`,
    /// so only a job that starts on an idle agent gets pending timing. Overwrites a stale entry
    /// left by an earlier job that never reached playback.
    async fn begin_first_audio_timing(
        inner: &Arc<Mutex<AgentInner>>,
        requested_at: Instant,
        text: &str,
    ) {
        let mut guard = inner.lock().await;
        if !guard.agent_speaking {
            let sentence_case_adjusted =
                sentence_case_adjusted_for(guard.config.tts.as_ref(), text);
            guard.pending_first_audio = Some(FirstAudioTiming {
                requested_at,
                first_chunk_ms: None,
                sentence_case_adjusted,
            });
        }
    }

    /// Records speak request to first vendor PCM chunk (OTel + pending timing for this job).
    async fn record_first_chunk(inner: &Arc<Mutex<AgentInner>>, requested_at: Instant) {
        let ms = Self::elapsed_ms_u32(requested_at);
        let mut guard = inner.lock().await;
        otel::record_voice_tts_first_chunk_ms(
            f64::from(ms),
            Self::active_tts_vendor_label(&guard),
            Self::project_id_label(&guard),
        );
        if let Some(timing) = guard.pending_first_audio.as_mut() {
            if timing.requested_at == requested_at && timing.first_chunk_ms.is_none() {
                timing.first_chunk_ms = Some(ms);
            }
        }
    }

    fn clear_pending_first_audio(inner: &mut AgentInner) {
        inner.pending_first_audio = None;
    }

    /// Pending time-to-first-audio timing is set (tests).
    #[doc(hidden)]
    pub async fn has_pending_first_audio(&self) -> bool {
        self.inner.lock().await.pending_first_audio.is_some()
    }

    async fn record_tts_job_latency(inner: &Arc<Mutex<AgentInner>>, started: Instant) {
        let tts_attrs = {
            let guard = inner.lock().await;
            let project_id = guard
                .otel
                .session_context
                .project_id
                .as_deref()
                .unwrap_or("");
            guard
                .config
                .tts
                .as_ref()
                .map(|cfg| otel::SherpaTtsMetricAttrs::from_tts_config(cfg, project_id))
                .unwrap_or_default()
        };
        otel::record_tts_latency_ms(started.elapsed().as_secs_f64() * 1000.0, &tts_attrs);
    }

    #[allow(clippy::too_many_arguments)] // shared handles of the TTS worker, kept as plain params (hot path)
    async fn ensure_tts_drain_worker_shared(
        slot: &Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
        wake: &Arc<Notify>,
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        shutdown: &Arc<AtomicBool>,
        shutdown_wake: &Arc<Notify>,
        alive: &Arc<AtomicUsize>,
        weak_self: &Weak<VoiceAgent>,
        tts_synthesis_busy: &Arc<AtomicBool>,
        tts_synthesis_queue: &Arc<Mutex<VecDeque<TtsSynthesisJob>>>,
    ) {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        let mut guard = slot.lock().await;
        if guard.is_some() {
            return;
        }
        let wake = Arc::clone(wake);
        let tts_buffer = tts_buffer.clone();
        let inner = Arc::clone(inner);
        let event_bus = event_bus.clone();
        let shutdown = Arc::clone(shutdown);
        let shutdown_wake = Arc::clone(shutdown_wake);
        let alive = Arc::clone(alive);
        let weak_self = weak_self.clone();
        let tts_synthesis_busy = Arc::clone(tts_synthesis_busy);
        let tts_synthesis_queue = Arc::clone(tts_synthesis_queue);
        *guard = Some(tokio::spawn(async move {
            let _alive_guard = TtsWorkerAliveGuard::enter(&alive);
            loop {
                tokio::select! {
                    _ = wake.notified() => {}
                    _ = shutdown_wake.notified() => {}
                    _ = async {
                        // Also ends when the agent was dropped without stop(): this task
                        // owns the TTS buffer (and the PCM writer), so it must not outlive it.
                        while !shutdown.load(Ordering::SeqCst) && weak_self.strong_count() > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                    } => {}
                }
                if shutdown.load(Ordering::SeqCst) || weak_self.strong_count() == 0 {
                    break;
                }
                if let Err(error) = VoiceAgent::run_tts_drain(
                    &tts_buffer,
                    &inner,
                    &event_bus,
                    &weak_self,
                    &tts_synthesis_busy,
                    &tts_synthesis_queue,
                )
                .await
                {
                    voice_debug(format!("TTS drain error: {error}"));
                    let message = playback_failure_message(&error);
                    VoiceAgent::abort_tts_playback_failure(
                        &tts_buffer,
                        &inner,
                        &event_bus,
                        message,
                    )
                    .await;
                }
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
            }
        }));
    }

    /// Flush buffered TTS, reset speaking state, emit error (+ speaking_end when needed), and
    /// publish a failure generation so blocking waiters observe the original error.
    async fn abort_tts_playback_failure(
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        message: impl Into<String>,
    ) {
        let message = message.into();
        // Record failure before clearing idle flags so waiters never treat abort as success.
        let emit_speaking_end = {
            let mut guard = inner.lock().await;
            guard.tts_playback_failure_gen = guard.tts_playback_failure_gen.wrapping_add(1);
            guard.tts_playback_last_error = Some(message.clone());
            let was_speaking = guard.agent_speaking;
            Self::clear_pending_first_audio(&mut guard);
            guard.agent_speaking = false;
            guard.agent_speaking_since = None;
            guard.barge_awaiting_stt_partial = false;
            guard.stt_barge_fired_this_agent_playback = false;
            was_speaking
        };
        tts_buffer
            .flush_with_reply_events(|event| event_bus.emit(event))
            .await;
        event_bus.emit(SpeechEvent::error(message.clone()));
        if emit_speaking_end {
            event_bus.emit(SpeechEvent::agent_speaking_end());
        }
        voice_debug(format!(
            "TTS playback aborted: {message} (speaking_end={emit_speaking_end})"
        ));
    }

    async fn wait_job_playback_idle(
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        weak_self: &Weak<VoiceAgent>,
        failure_gen_at_start: u64,
    ) -> SpeechResult<()> {
        let deadline = Instant::now() + std::time::Duration::from_secs(45);
        loop {
            // Dropped without stop(): the drain ended silently, so nothing will ever clear
            // `agent_speaking`; do not hold the synthesis worker (and the writer) for 45 s.
            if weak_self.strong_count() == 0 {
                return Ok(());
            }
            {
                let guard = inner.lock().await;
                // stop() clears running before joining workers — do not block shutdown.
                if !guard.running {
                    return Ok(());
                }
                if guard.tts_playback_failure_gen != failure_gen_at_start {
                    let message = guard
                        .tts_playback_last_error
                        .clone()
                        .unwrap_or_else(|| "TTS playback failed".into());
                    return Err(SpeechError::Internal(message));
                }
                let agent_speaking = guard.agent_speaking;
                drop(guard);
                let queued = tts_buffer.is_speaking().await;
                if !agent_speaking && !queued {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                let message = "timed out waiting for TTS job playback";
                Self::abort_tts_playback_failure(tts_buffer, inner, event_bus, message).await;
                return Err(SpeechError::Internal(message.into()));
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    async fn cancel_pending_tts_synthesis(&self) {
        let dropped = self.cancel_pending_tts_synthesis_collect().await;
        self.emit_dropped_reply_ends(dropped);
    }

    /// Drop every queued synthesis job and return the reply ids of those that carried one.
    /// The caller emits their `agent_reply_end` ([`emit_dropped_reply_ends`](Self::emit_dropped_reply_ends))
    /// after it has flushed the buffer, so replies end in queue order.
    async fn cancel_pending_tts_synthesis_collect(&self) -> Vec<Arc<str>> {
        self.invalidate_inflight_tts_synthesis();
        Self::clear_pending_first_audio(&mut *self.inner.lock().await);
        let mut queue = self.tts_synthesis_queue.lock().await;
        let mut dropped = Vec::new();
        for job in queue.drain(..) {
            if let Some(id) = job.reply_id {
                dropped.push(id);
            }
            if let Some(done) = job.done {
                let _ = done.send(Err(SpeechError::Internal("TTS cancelled".into())));
            }
        }
        dropped
    }

    fn emit_dropped_reply_ends(&self, dropped: Vec<Arc<str>>) {
        for id in dropped {
            self.emit(SpeechEvent::agent_reply_end(&id, 0, true, 0, false));
        }
    }

    /// Clears pending outbound TTS (manual cancel or barge-in when `flush_tts` is enabled).
    pub async fn flush_tts(&self) -> SpeechResult<()> {
        let dropped = self.cancel_pending_tts_synthesis_collect().await;
        let barge_in = {
            let inner = self.inner.lock().await;
            inner.config.vad.barge_in.clone()
        };
        if barge_in.enabled && !barge_in.use_vad {
            self.barge_in_with_speaking_end(&barge_in).await;
            voice_debug("agent_speaking=false (manual barge-in flush)");
        } else {
            self.tts_buffer
                .flush_with_reply_events(|event| self.emit(event))
                .await;
            let was_speaking = self.end_agent_speaking(true).await;
            voice_debug("agent_speaking=false (flush_tts)");
            if was_speaking {
                self.emit(SpeechEvent::agent_speaking_end());
            }
        }
        self.emit_dropped_reply_ends(dropped);
        Ok(())
    }

    /// Non-blocking poll for stream-mode event delivery (NAPI / TS `speechEvents()`).
    pub async fn pull_speech_event(&self) -> Option<SpeechEvent> {
        None
    }

    /// Clears TTS playback state; optionally arms STT hold when playback ends and VAD is idle.
    /// Returns whether `agent_speaking` was true (read and cleared under one lock).
    async fn end_agent_speaking(&self, arm_stt_hold_after_playback: bool) -> bool {
        Self::end_agent_speaking_inner(&self.inner, arm_stt_hold_after_playback).await
    }

    /// Starts the speaker gate's verifier task (once). Verdicts are applied on their own task so
    /// the verifier never waits for the agent's step lock.
    fn start_speaker_gate(&self) {
        let Some(gate) = self.speaker_gate.as_ref() else {
            return;
        };
        let Some(embedder) = self
            .speaker_embedder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        else {
            return;
        };
        let weak = self.weak_self.clone();
        gate.spawn(
            embedder,
            Box::new(move |verdict| {
                if let Some(agent) = weak.upgrade() {
                    tokio::spawn(async move { agent.apply_speaker_verdict(verdict).await });
                }
            }),
        );
    }

    /// Forget the enrolled caller; the next utterances enroll again. No-op with the gate off.
    pub fn reset_speaker_enrollment(&self) {
        if let Some(gate) = self.speaker_gate.as_ref() {
            gate.reset();
        }
    }

    /// Verdicts dropped because their utterance was gone (tests and diagnostics).
    pub fn speaker_gate_stale_verdicts(&self) -> u64 {
        self.speaker_gate.as_ref().map_or(0, |g| g.stale_verdicts())
    }

    /// True once the caller's voice is enrolled (tests and diagnostics).
    pub fn speaker_gate_enrolled(&self) -> bool {
        self.speaker_gate.as_ref().is_some_and(|g| g.is_enrolled())
    }

    /// Warn lines of the speaker gate (tests and diagnostics).
    pub fn speaker_gate_warnings(&self) -> Vec<String> {
        self.speaker_gate
            .as_ref()
            .map_or_else(Vec::new, |g| g.warnings())
    }

    /// Applies one verdict to its own utterance only (`rt.stale-event-by-key`).
    async fn apply_speaker_verdict(&self, verdict: Verdict) {
        let Some(gate) = self.speaker_gate.as_ref() else {
            return;
        };
        let _step = self.utterance_step.lock().await;
        let (known, live, barge_in, speaking) = {
            let inner = self.inner.lock().await;
            (
                inner.record_ref(verdict.key).is_some(),
                inner.live_key() == Some(verdict.key),
                inner.config.vad.barge_in.clone(),
                inner.agent_speaking,
            )
        };
        if !known {
            gate.note_stale_verdict();
            voice_debug(format!(
                "speaker gate: verdict for gone utterance key={} dropped",
                verdict.key
            ));
            return;
        }
        if !gate.take_pending_barge(verdict.key) {
            return;
        }
        if verdict.outcome == Outcome::Other {
            eprintln!(
                "[speaker-gate] info: speaker gate suppressed barge-in key={}",
                verdict.key
            );
            return;
        }
        if !live || !speaking || self.protected_tts_active() {
            return;
        }
        {
            let mut inner = self.inner.lock().await;
            inner.barge_awaiting_stt_partial = false;
            inner.stt_barge_fired_this_agent_playback = true;
        }
        voice_debug(format!(
            "speaker gate: barge-in after verdict ({})",
            verdict.outcome.as_str()
        ));
        self.barge_in_with_speaking_end(&barge_in).await;
        self.cancel_pending_tts_synthesis().await;
    }

    /// True when a barge-in for the live utterance must wait for (or is vetoed by) the speaker
    /// verdict. Records the pending barge so the verdict handler fires it.
    async fn speaker_gate_holds_barge(&self) -> bool {
        let Some(gate) = self.speaker_gate.as_ref() else {
            return false;
        };
        let Some(key) = self.inner.lock().await.live_key() else {
            return false;
        };
        match gate.barge_decision(key) {
            BargeDecision::Proceed => false,
            BargeDecision::Suppress => true,
            BargeDecision::Defer => {
                gate.set_pending_barge(key);
                true
            }
        }
    }

    /// Barge-in with a single owner for `agent_speaking_end`: end speaking BEFORE the flush, so the
    /// TTS drain (which observes the flush) finds `agent_speaking == false` and does not emit; then
    /// flush + `agent_reply_end{interrupted}` + `barge_in`; then `agent_speaking_end` if this call
    /// ended speaking. Order is always reply_end(s) -> barge_in -> agent_speaking_end.
    async fn barge_in_with_speaking_end(&self, barge_in: &BargeInConfig) {
        if !barge_in.enabled {
            return;
        }
        let ended = if barge_in.flush_tts {
            self.end_agent_speaking(false).await
        } else {
            false
        };
        if barge_in.flush_tts {
            self.tts_buffer
                .flush_with_reply_events(|event| self.emit(event))
                .await;
        }
        let delay_ms = self.barge_in_flush_delay_ms.load(Ordering::Relaxed);
        if delay_ms > 0 {
            // Test-only hook (`set_barge_in_flush_delay_for_test`).
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        self.emit(SpeechEvent::barge_in());
        if ended {
            self.emit(SpeechEvent::agent_speaking_end());
        }
    }

    fn agent_playback_guard_active(inner: &AgentInner) -> bool {
        if !inner.agent_speaking {
            return false;
        }
        let Some(since) = inner.agent_speaking_since else {
            return false;
        };
        let guard_ms = inner.config.vad.barge_in.agent_playback_guard_ms;
        since.elapsed() < std::time::Duration::from_millis(guard_ms as u64)
    }

    fn stt_partial_qualifies_for_barge(inner: &AgentInner, text: &str) -> bool {
        let min_tokens = inner.config.vad.barge_in.min_stt_partial_tokens.max(1) as usize;
        crate::config::stt_partial_token_count(text) >= min_tokens
    }

    /// True while a job queued with `interruptible: false` is synthesizing or playing.
    fn protected_tts_active(&self) -> bool {
        self.tts_protected_active.load(Ordering::SeqCst)
    }

    async fn try_stt_gated_barge_in(&self, key: u64, partial_text: &str) -> SpeechResult<()> {
        if self.protected_tts_active() {
            voice_debug("STT-gated barge-in suppressed: non-interruptible TTS job active");
            return Ok(());
        }
        let (should_barge, barge_in) = {
            let inner = self.inner.lock().await;
            let barge_in_cfg = &inner.config.vad.barge_in;
            if !barge_in_cfg.enabled
                || !barge_in_cfg.require_stt_partial
                || inner.config.stt.is_none()
            {
                return Ok(());
            }
            if !inner.agent_speaking {
                return Ok(());
            }
            // Only the live utterance can barge in (stt-gated barge-in, D10). A closing (or
            // LID-retained) utterance's words were spoken before the current agent reply could
            // have started being interrupted by them.
            if inner.live_key() != Some(key) {
                return Ok(());
            }
            if !inner.record_ref(key).is_some_and(|r| r.vad_triggered) {
                return Ok(());
            }
            if !Self::stt_partial_qualifies_for_barge(&inner, partial_text) {
                return Ok(());
            }
            if inner.stt_barge_fired_this_agent_playback {
                return Ok(());
            }
            (true, barge_in_cfg.clone())
        };
        if !should_barge {
            return Ok(());
        }
        if self.speaker_gate_holds_barge().await {
            voice_debug("STT-gated barge-in held by speaker gate");
            return Ok(());
        }
        {
            let mut inner = self.inner.lock().await;
            inner.barge_awaiting_stt_partial = false;
            inner.stt_barge_fired_this_agent_playback = true;
        }
        voice_debug(format!(
            "STT-gated barge-in: partial {:?}",
            partial_text.trim()
        ));
        {
            let mut guard = self.inner.lock().await;
            let ctx = guard.otel.session_context.clone();
            otel::record_barge_in(&ctx);
            if guard.turn_latency.on_barge_in(Instant::now()).is_some() {
                otel::record_voice_turn_response_abandoned(Self::project_id_label(&guard));
            }
        }
        self.barge_in_with_speaking_end(&barge_in).await;
        self.cancel_pending_tts_synthesis().await;
        Ok(())
    }

    async fn emit_stt_stream_start_if_needed(&self) {
        let id = {
            let mut inner = self.inner.lock().await;
            let rec = inner.ensure_live();
            if rec.stream_open {
                None
            } else {
                rec.stream_open = true;
                Some(rec.id.clone())
            }
        };
        if let Some(id) = id {
            voice_debug("emit stt_stream_start");
            self.emit(SpeechEvent::stt_stream_start().with_utterance_id(Some(id)));
        }
    }

    async fn emit_user_stt_start_if_needed(&self) {
        let id = {
            let mut inner = self.inner.lock().await;
            let rec = inner.ensure_live();
            if rec.user_stt_session_open {
                None
            } else {
                rec.user_stt_session_open = true;
                Some(rec.id.clone())
            }
        };
        if let Some(id) = id {
            voice_debug("emit user_stt_start");
            self.emit(SpeechEvent::user_stt_start().with_utterance_id(Some(id)));
        }
    }

    async fn on_vad_speech_start(&self) -> SpeechResult<()> {
        let (barge_in, guard_active, agent_speaking, has_stt, vad_enabled) = {
            let inner = self.inner.lock().await;
            (
                inner.config.vad.barge_in.clone(),
                Self::agent_playback_guard_active(&inner),
                inner.agent_speaking,
                Self::stt_pipeline_active(&inner),
                inner.config.vad.enabled,
            )
        };
        if !vad_enabled {
            return Ok(());
        }

        voice_debug("emit vad_triggered (VAD SpeechStart)");
        self.emit(SpeechEvent::vad_triggered());
        // A finals-only provider (Fish, Groq, ...) answers only after finalize: C1 would cut a
        // long utterance before it, and no partial will ever qualify a barge-in.
        let emits_partials = self.stt_emits_partials();
        let require_stt_partial = barge_in.require_stt_partial && emits_partials;
        if has_stt {
            self.emit_user_stt_start_if_needed().await;
            self.emit_stt_stream_start_if_needed().await;
        }

        {
            let mut inner = self.inner.lock().await;
            let partials_emitted = {
                let rec = inner.ensure_live();
                rec.vad_triggered = true;
                rec.partials_emitted
            };
            inner.c1_backlog_deferred_logged = false;
            if has_stt && !partials_emitted && emits_partials {
                inner.stt_listen_deadline_ms = inner.config.vad.stt_listen_timeout_ms;
                inner.stt_listen_started_at = Some(Instant::now());
                inner.stt_listen_media_ms = 0;
            }
            if !partials_emitted {
                Self::clear_utterance_finalize_timer(&mut inner);
            }
        }

        if barge_in.enabled && barge_in.use_vad && agent_speaking {
            if self.protected_tts_active() {
                voice_debug("barge-in suppressed: non-interruptible TTS job active");
                return Ok(());
            }
            if guard_active {
                voice_debug(format!(
                    "barge-in suppressed: agent playback guard {} ms",
                    barge_in.agent_playback_guard_ms
                ));
                return Ok(());
            }
            if require_stt_partial && has_stt {
                let mut inner = self.inner.lock().await;
                inner.barge_awaiting_stt_partial = true;
                voice_debug("barge-in deferred until qualifying STT partial (require_stt_partial)");
                return Ok(());
            }
            if !require_stt_partial || !has_stt {
                if self.speaker_gate_holds_barge().await {
                    voice_debug("barge-in held by speaker gate (VAD SpeechStart)");
                    return Ok(());
                }
                voice_debug(format!(
                    "immediate barge-in on VAD SpeechStart (require_stt_partial={require_stt_partial}, emits_partials={emits_partials})"
                ));
                self.barge_in_with_speaking_end(&barge_in).await;
                self.cancel_pending_tts_synthesis().await;
            }
        }
        Ok(())
    }

    /// C1: no STT partial arrived in time. Emits the terminal `user_stt_not_found` at once and
    /// tells the vendor where the utterance ends (`EndUtterance`, answered by a `FinalizeDone`
    /// that emits nothing more because the record is terminal).
    async fn close_stt_stream_not_found(&self) -> SpeechResult<()> {
        let (stream_was_open, session_open, vad_speaking, utterance_id, key, held) = {
            let mut inner = self.inner.lock().await;
            let vad_speaking = Self::vad_is_speaking(&inner);
            let held = inner.stt_hold.is_some();
            let Some(rec) = inner.live.as_mut() else {
                return Ok(());
            };
            let stream_was_open = rec.stream_open;
            let session_open = rec.user_stt_session_open;
            let utterance_id = rec.id.clone();
            let key = rec.key;
            if session_open {
                rec.not_found_emitted = true;
                rec.mark_terminal();
            }
            rec.stream_open = false;
            rec.user_stt_session_open = false;
            rec.vad_triggered = false;
            Self::clear_stt_listen_timer(&mut inner);
            Self::clear_utterance_finalize_timer(&mut inner);
            inner.barge_awaiting_stt_partial = false;
            (
                stream_was_open,
                session_open,
                vad_speaking,
                utterance_id,
                key,
                held,
            )
        };
        if stream_was_open {
            voice_debug("emit stt_stream_end (C1: no STT partial)");
            self.emit(
                SpeechEvent::stt_stream_end_with_reason("listen_timeout")
                    .with_utterance_id(Some(utterance_id.clone())),
            );
        }
        if session_open {
            voice_debug("emit user_stt_not_found + user_stt_end (C1)");
            self.emit(SpeechEvent::user_stt_not_found_with_reason(
                "no_partial",
                Some(utterance_id.clone()),
            ));
            self.emit(SpeechEvent::user_stt_end().with_utterance_id(Some(utterance_id)));
        }
        if !vad_speaking {
            let emit_end = {
                let mut inner = self.inner.lock().await;
                match inner.record_mut(key) {
                    Some(rec) if !rec.speaking_end_emitted => {
                        rec.speaking_end_emitted = true;
                        true
                    }
                    _ => false,
                }
            };
            if emit_end {
                self.emit_user_speaking_end_after_lid_gate(key).await;
            }
        }
        if !held {
            let mut inner = self.inner.lock().await;
            if let Some(actor) = self.actor() {
                actor.send(SttCommand::EndUtterance { key });
            }
            // While VAD still hears speech the record stays live: its `user_speaking_end` comes
            // later through the gate close and must carry the id. New speech replaces it.
            if !vad_speaking && inner.live_key() == Some(key) {
                if let Some(mut rec) = inner.live.take() {
                    rec.phase = UttPhase::Closing;
                    rec.close_reason = "listen_timeout";
                    inner.closing.push_back(rec);
                }
            }
        }
        Ok(())
    }

    /// Resets the turn-level flags a close ends (the record-level flags live on the record).
    fn reset_turn_close_flags(inner: &mut AgentInner) {
        inner.stt_finalize_pending = false;
        inner.stt_endpoint_closing_started = false;
        inner.stt_gate_hold_ms = 0;
        Self::clear_stt_listen_timer(inner);
        Self::clear_utterance_finalize_timer(inner);
    }

    /// Ends the live utterance towards the STT: endpoint tail audio, replay snapshot, then
    /// `EndUtterance`. Only sends commands; the vendor's answer is applied by the result pump
    /// (`FinalizeDone`). Called with the agent state locked.
    ///
    /// While an STT hold is active nothing reaches the STT: the tail goes into the held audio
    /// and the caller closes the record inline ([`CloseStarted::Held`]).
    fn begin_utterance_close(
        &self,
        inner: &mut AgentInner,
        reason: &'static str,
        with_tail: bool,
    ) -> CloseStarted {
        const CHUNK_MS: u32 = 100;
        let tail_ms = stt_endpoint_tail_ms(&inner.config.vad);
        let chunks = tail_ms.div_ceil(CHUNK_MS);
        let chunk = crate::pcm::silence_mono_s16le_bytes(CHUNK_MS);
        if inner.stt_hold.is_some() {
            if with_tail {
                voice_debug(format!("STT endpoint tail: {tail_ms} ms silence"));
                let live_id = inner
                    .live
                    .as_ref()
                    .filter(|r| !r.terminal)
                    .map(|r| r.id.clone());
                if let Some(hold) = inner.stt_hold.as_mut() {
                    if hold.replaces_utterance_id.is_none() {
                        hold.replaces_utterance_id = live_id;
                    }
                    for _ in 0..chunks {
                        hold.push(chunk.as_ref());
                    }
                }
            }
            voice_debug("STT finalize_utterance deferred (host STT hold)");
            if let Some(hold) = inner.stt_hold.as_mut() {
                hold.note_finalize();
            }
            return CloseStarted::Held;
        }
        Self::reset_turn_close_flags(inner);
        let Some(mut rec) = inner.live.take() else {
            return CloseStarted::NoRecord;
        };
        if with_tail && inner.stt_enabled {
            voice_debug(format!("STT endpoint tail: {tail_ms} ms silence"));
            for _ in 0..chunks {
                inner.utterance_replay_buffer.push(chunk.as_ref());
                self.send_stt_audio(chunk.clone());
            }
        }
        voice_debug("STT finalize_utterance: vendor finalize requested");
        Self::commit_utterance_replay_snapshot(inner, rec.id.clone());
        rec.phase = UttPhase::Closing;
        rec.close_reason = reason;
        let key = rec.key;
        inner.closing.push_back(rec);
        if let Some(actor) = self.actor() {
            actor.send(SttCommand::EndUtterance { key });
        }
        // A queued STT config is swapped in by the actor once this finalize is done.
        drop(self.send_pending_swap(inner));
        CloseStarted::Closing
    }

    /// Applies a close that has no STT answer to wait for (host STT hold, or no stream to end)
    /// to the live record.
    async fn finish_live_inline(&self, wait_lid: bool) {
        let key = self.inner.lock().await.live_key();
        if let Some(key) = key {
            self.finish_record_close(key, true, wait_lid).await;
        }
    }

    async fn force_close_utterance(&self, close_reason: &'static str) -> SpeechResult<()> {
        voice_debug("force_close_utterance (C2 timeout or stall)");
        let started = {
            let mut inner = self.inner.lock().await;
            let needs_finalize = inner.live_flag(|r| r.stream_open && !r.final_emitted);
            if needs_finalize {
                inner.stt_endpoint_closing_started = true;
                Some(self.begin_utterance_close(&mut inner, close_reason, true))
            } else {
                if let Some(live) = inner.live.as_mut() {
                    live.close_reason = close_reason;
                }
                None
            }
        };
        match started {
            // The fallback final / not_found is emitted when the vendor's answer (or its
            // timeout) arrives, by the result pump.
            Some(CloseStarted::Closing) | Some(CloseStarted::NoRecord) => {}
            Some(CloseStarted::Held) | None => self.finish_live_inline(true).await,
        }
        Ok(())
    }

    /// Feeds an STT final into the turn latency tracker (finalize ms is recorded once per turn).
    async fn note_stt_final_for_turn_latency(&self) {
        let mut guard = self.inner.lock().await;
        if let Some(finalize_ms) = guard.turn_latency.on_stt_final(Instant::now()) {
            otel::record_voice_stt_finalize_ms(
                finalize_ms,
                Self::project_id_label(&guard),
                Self::active_stt_vendor_label(&guard),
                Self::active_tts_vendor_label(&guard),
            );
        }
    }

    /// Emit the final of record `key`. A replay record, or a pending replay context, makes it a
    /// replay final. An empty text ends the utterance in `user_stt_not_found` (once).
    async fn emit_utterance_final(&self, key: u64, text: String) {
        if text.trim().is_empty() {
            // Never emit an empty final: the utterance ends in `user_stt_not_found` (once).
            let not_found_utterance_id = {
                let mut inner = self.inner.lock().await;
                let Some(rec) = inner.record_mut(key) else {
                    return;
                };
                rec.replay_ctx.take();
                if rec.not_found_emitted {
                    None
                } else {
                    rec.not_found_emitted = true;
                    rec.mark_terminal();
                    Some(rec.id.clone())
                }
            };
            if let Some(utterance_id) = not_found_utterance_id {
                self.emit(SpeechEvent::user_stt_not_found_with_reason(
                    "empty_final",
                    Some(utterance_id),
                ));
            }
            return;
        }
        self.note_stt_final_for_turn_latency().await;
        // Speaker gate: the verdict of this utterance, waiting at most `verifyTimeoutMs` for a
        // check that was sent and has not settled (then `unknown`).
        let speaker = match self.speaker_gate.as_ref() {
            Some(gate) => Some(gate.await_outcome(key).await),
            None => None,
        };
        let drop_mismatch = self.inner.lock().await.config.speaker_gate.on_mismatch
            == crate::config::SpeakerMismatchAction::Drop;
        let event = {
            let mut inner = self.inner.lock().await;
            let Some(rec) = inner.record_mut(key) else {
                return;
            };
            let replay_ctx = rec
                .replay_ctx
                .take()
                .filter(|ctx| ctx.set_at.elapsed() <= REPLAY_CONTEXT_MAX_AGE);
            let event = match (&rec.kind, replay_ctx) {
                (UttKind::Replay { replaces }, _) => SpeechEvent::user_speech_final_held_replay(
                    text,
                    Some(rec.id.clone()),
                    replaces.clone(),
                ),
                (
                    UttKind::Live,
                    Some(ReplayFinalContext {
                        replaces_utterance_id: Some(id),
                        ..
                    }),
                ) => SpeechEvent::user_speech_final_replay(text, Some(next_utterance_id()), id),
                (
                    UttKind::Live,
                    Some(ReplayFinalContext {
                        replaces_utterance_id: None,
                        ..
                    }),
                ) => {
                    // Held release with no original final.
                    SpeechEvent::user_speech_final_held_replay(
                        text,
                        Some(next_utterance_id()),
                        None,
                    )
                }
                (UttKind::Live, None) => SpeechEvent::user_speech_final(text, Some(rec.id.clone())),
            };
            rec.final_emitted = true;
            rec.mark_terminal();
            match speaker {
                Some(Outcome::Other) if drop_mismatch => {
                    SpeechEvent::user_speech_ignored(Some(rec.id.clone()))
                }
                Some(outcome) => event.with_speaker(outcome.as_str()),
                None => event,
            }
        };
        self.emit(event);
        if let (Some(gate), Some(outcome)) = (self.speaker_gate.as_ref(), speaker) {
            if outcome != Outcome::Other {
                gate.enroll_utterance(key);
            }
        }
        let mut inner = self.inner.lock().await;
        if let Some(rec) = inner.record_mut(key) {
            rec.fire_terminal_waiter(true);
        }
    }

    fn arm_stt_hold_if_idle(inner: &mut AgentInner) {
        if !inner.config.vad.gate_stt || inner.stt_gate_hold_ms > 0 {
            return;
        }
        let still_speaking = inner.vad.as_ref().map(|v| v.is_speaking()).unwrap_or(false);
        if still_speaking {
            return;
        }
        // Previous utterance is complete and nothing new started: arming would leave stt_finalize_pending set until the next SpeechStart (blocks update_stt_config / replay_last_utterance).
        let completed = inner
            .live
            .as_ref()
            .is_none_or(|r| r.final_emitted && !r.stream_open && !r.vad_triggered);
        if completed && !inner.stt_finalize_pending {
            return;
        }
        inner.stt_gate_hold_ms = inner.config.vad.stt_gate_hold_ms;
        inner.stt_finalize_pending = true;
        voice_debug(format!(
            "STT gate hold: {} ms after TTS playback ended",
            inner.stt_gate_hold_ms
        ));
    }

    /// Applies the end of record `key` (the vendor answered its `EndUtterance`, timed out, or
    /// no answer is awaited) and guarantees the utterance ends with a terminal event: the
    /// last-partial fallback final, or `user_stt_not_found` (`no_transcript`). Emits nothing extra
    /// when the vendor final already arrived (stream flags cleared, speaking_end emitted).
    ///
    /// `inline_live` is set for the live record: it also resets the turn-level timers and flags
    /// that belong to it. `wait_lid` is false when the caller already waited for the LID gate.
    async fn finish_record_close(&self, key: u64, inline_live: bool, wait_lid: bool) {
        let (
            emit_stream_end,
            emit_stt_end,
            need_forced_final,
            forced_text,
            not_found,
            close_reason,
            utterance_id,
            forced_reason,
        ) = {
            let mut inner = self.inner.lock().await;
            let hold_active = inner.stt_hold.is_some();
            let Some(rec) = inner.record_mut(key) else {
                return;
            };
            if matches!(rec.kind, UttKind::Replay { .. }) {
                return;
            }
            let stream = rec.stream_open;
            let session = rec.user_stt_session_open;
            rec.stream_open = false;
            rec.user_stt_session_open = false;
            rec.vad_triggered = false;
            let forced_text = rec.last_partial_text.clone().unwrap_or_default();
            let has_text = !forced_text.trim().is_empty();
            let forced_reason = matches!(rec.close_reason, "finalize_timeout" | "pcm_stall");
            let no_final = !rec.final_emitted && !hold_active;
            let need_forced = no_final && rec.partials_emitted && has_text;
            // No vendor final and no partial text: the utterance ends in `user_stt_not_found`.
            let not_found = no_final
                && !has_text
                && (stream || session || forced_reason)
                && !rec.not_found_emitted;
            if not_found {
                rec.not_found_emitted = true;
                rec.mark_terminal();
            }
            let out = (
                stream,
                session,
                need_forced,
                forced_text,
                not_found,
                rec.close_reason,
                rec.id.clone(),
                forced_reason,
            );
            if inline_live {
                Self::clear_stt_listen_timer(&mut inner);
                Self::clear_utterance_finalize_timer(&mut inner);
            }
            inner.barge_awaiting_stt_partial = false;
            out
        };
        if emit_stream_end {
            self.emit(
                SpeechEvent::stt_stream_end_with_reason(close_reason)
                    .with_utterance_id(Some(utterance_id.clone())),
            );
        }
        if not_found {
            voice_debug("emit user_stt_not_found (utterance close, no transcript)");
            self.emit(SpeechEvent::user_stt_not_found_with_reason(
                "no_transcript",
                Some(utterance_id.clone()),
            ));
        }
        if emit_stt_end {
            self.emit(SpeechEvent::user_stt_end().with_utterance_id(Some(utterance_id)));
        }
        if need_forced_final {
            let emit_speaking_end = {
                let mut inner = self.inner.lock().await;
                let emit_end = match inner.record_mut(key) {
                    Some(rec) => {
                        let emit_end = !rec.speaking_end_emitted;
                        if emit_end {
                            rec.speaking_end_emitted = true;
                        }
                        rec.final_emitted = true;
                        emit_end
                    }
                    None => false,
                };
                if inline_live {
                    inner.stt_finalize_pending = false;
                    inner.stt_endpoint_closing_started = false;
                }
                emit_end
            };
            if emit_speaking_end {
                voice_debug("emit user_speaking_end (finalize without vendor final)");
                self.emit_speaking_end(key, wait_lid).await;
            }
            voice_debug(format!(
                "emit user_speech_final (last partial fallback): {}",
                if forced_text.len() > 80 {
                    format!("{}…", &forced_text[..80])
                } else {
                    forced_text.clone()
                }
            ));
            self.emit_utterance_final(key, forced_text).await;
        } else {
            let emit_speaking_end_at_finalize = {
                let mut inner = self.inner.lock().await;
                let emit_end = match inner.record_mut(key) {
                    Some(rec) => {
                        // A forced (C2) close ends the utterance even without a transcript.
                        if forced_reason && !rec.final_emitted {
                            rec.final_emitted = true;
                        }
                        if rec.speaking_end_emitted {
                            false
                        } else {
                            rec.speaking_end_emitted = true;
                            true
                        }
                    }
                    None => false,
                };
                if emit_end && inline_live {
                    inner.stt_finalize_pending = false;
                    inner.stt_endpoint_closing_started = false;
                }
                emit_end
            };
            if emit_speaking_end_at_finalize {
                voice_debug("emit user_speaking_end (STT finalize without vendor final)");
                self.emit_speaking_end(key, wait_lid).await;
            }
        }
    }

    /// Finish STT for an utterance waiting on gate hold / finalize (e.g. before new `SpeechStart`).
    async fn complete_pending_utterance_if_any(&self) -> SpeechResult<()> {
        let started = {
            let mut inner = self.inner.lock().await;
            if !inner.config.vad.gate_stt || inner.config.stt.is_none() {
                return Ok(());
            }
            let needed = inner.stt_finalize_pending && !inner.live_flag(|r| r.final_emitted);
            if !needed {
                return Ok(());
            }
            voice_debug("STT: completing previous utterance (pending finalize before new speech)");
            finalize_trace("complete_pending", &inner);
            let closing_started = inner.stt_endpoint_closing_started;
            self.begin_utterance_close(&mut inner, "end_of_utterance", !closing_started)
        };
        if matches!(started, CloseStarted::Held) {
            self.finish_live_inline(true).await;
        }
        Ok(())
    }

    /// New speech starts a new utterance: turn-level state is reset and a fresh record (new key
    /// and id) replaces the live one.
    fn start_new_live_record(inner: &mut AgentInner) {
        inner.stt_gate_hold_ms = 0;
        inner.stt_finalize_pending = false;
        inner.stt_endpoint_closing_started = false;
        Self::clear_stt_listen_timer(inner);
        Self::clear_utterance_finalize_timer(inner);
        let rec = inner.new_record();
        if let Some(previous) = inner.live.replace(rec) {
            inner.retain_for_lid(previous);
        }
    }

    async fn start_lid_buffering(&self) {
        let enabled = {
            let inner = self.inner.lock().await;
            language_id_enabled(&inner.config.language_id)
        };
        if !enabled {
            return;
        }
        let mut inner = self.inner.lock().await;
        if let Some(live) = inner.live.as_mut() {
            live.lid.pcm_buffer.clear();
            live.lid.voiced_ms = 0;
            live.lid.pending_silence_ms = 0;
            live.lid.buffering = true;
        }
    }

    async fn append_lid_pcm_if_buffering(
        &self,
        mono_bytes: &Bytes,
        frame_active: bool,
        vad_speaking: bool,
        duration_ms: u32,
    ) {
        let identify_key = {
            let mut inner = self.inner.lock().await;
            if !inner.live_flag(|r| r.lid.buffering)
                || !language_id_enabled(&inner.config.language_id)
            {
                return;
            }
            // Bound the buffer to 2x the clip LID will use, keeping the OLDEST audio (the
            // clip is the head of the buffer); a long VAD-open stretch must not grow it.
            let max_clip_ms = crate::config::resolved_lid_max_clip_ms(
                inner.config.language_id.as_ref().expect("enabled"),
            );
            let continuous = language_id_continuous(&inner.config.language_id);
            let early = language_id_early(&inner.config.language_id);
            let min_ms = resolved_language_id_min_speech_ms(
                inner.config.language_id.as_ref().expect("enabled"),
            );
            let cap =
                (crate::pcm::STT_PCM_SAMPLE_RATE as usize * max_clip_ms as usize * 2 / 1000) * 2;
            let live = inner
                .live
                .as_mut()
                .expect("buffering implies a live record");
            let room = cap.saturating_sub(live.lid.pcm_buffer.len());
            let take = room.min(mono_bytes.len());
            live.lid.pcm_buffer.extend_from_slice(&mono_bytes[..take]);
            if frame_active {
                live.lid.voiced_ms = live
                    .lid
                    .voiced_ms
                    .saturating_add(live.lid.pending_silence_ms)
                    .saturating_add(duration_ms);
                live.lid.pending_silence_ms = 0;
            } else if vad_speaking {
                // Gap inside the utterance; counts only if speech resumes.
                live.lid.pending_silence_ms =
                    live.lid.pending_silence_ms.saturating_add(duration_ms);
            }
            // else: utterance tail / not speaking, never counted.
            // Default (end of utterance): buffer only until user_speaking_end.
            if !continuous && !early {
                return;
            }
            // Early: exactly one identify per utterance; nothing more once it started/deferred.
            if early && live.lid.identify_started {
                return;
            }
            (live.lid.voiced_ms >= min_ms).then_some(live.key)
        };
        if let Some(key) = identify_key {
            self.spawn_identify_language(key, false).await;
        }
    }

    /// Await in-flight LID (if any) then emit `user_speaking_end`. `user_language` is emitted by
    /// the identify task before this when successful.
    async fn emit_user_speaking_end_after_lid_gate(&self, key: u64) {
        // Buffer may still be growing during STT gate hold / endpoint tail after VAD SpeechEnd.
        self.prepare_lid_before_close(key).await;
        self.emit_user_speaking_end_for(key).await;
    }

    /// `user_speaking_end` of record `key`, waiting for the LID gate first when `wait_lid`.
    async fn emit_speaking_end(&self, key: u64, wait_lid: bool) {
        if wait_lid {
            self.emit_user_speaking_end_after_lid_gate(key).await;
        } else {
            self.emit_user_speaking_end_for(key).await;
        }
    }

    /// Starts the identify of the close window (if eligible) and waits for the LID gate, bounded.
    async fn prepare_lid_before_close(&self, key: u64) {
        self.try_spawn_default_mode_lid(key, false, "utterance close")
            .await;
        self.await_lid_gate_before_utterance_close(key).await;
    }

    async fn emit_user_speaking_end_for(&self, key: u64) {
        let utterance_id = {
            let mut inner = self.inner.lock().await;
            let enabled = language_id_enabled(&inner.config.language_id);
            let Some(rec) = inner.record_mut(key) else {
                return;
            };
            // No identify was started (default or continuous): tell the host no `user_language`
            // will follow for this utterance. Emitted before `user_speaking_end` / the final.
            if enabled && !rec.lid.identify_started && !rec.lid.identify_in_flight {
                let reason = if rec.lid.pcm_buffer.is_empty() {
                    "no_audio"
                } else {
                    "too_short"
                };
                self.emit_language_id_skipped_once(rec, reason);
            }
            rec.id.clone()
        };
        self.emit(SpeechEvent::user_speaking_end(Some(utterance_id)));
        let mut inner = self.inner.lock().await;
        if let Some(rec) = inner.record_mut(key) {
            rec.lid.buffering = false;
        }
    }

    /// Emit `language_id_skipped` once per utterance. Carries buffered speech ms, no transcript.
    fn emit_language_id_skipped_once(&self, rec: &mut SttUtterance, reason: &str) {
        let speech_ms = rec.lid.voiced_ms;
        self.emit_language_id_skipped_with_ms(rec, reason, speech_ms);
    }

    fn emit_language_id_skipped_with_ms(
        &self,
        rec: &mut SttUtterance,
        reason: &str,
        speech_ms: u32,
    ) {
        if rec.lid.skip_emitted {
            return;
        }
        rec.lid.skip_emitted = true;
        voice_debug(format!(
            "emit language_id_skipped: reason={reason} speech_ms={speech_ms}"
        ));
        self.emit(SpeechEvent::language_id_skipped(
            reason,
            speech_ms,
            Some(rec.id.clone()),
        ));
    }

    /// Start default-mode LID for record `key` when eligible. `context` is for debug only.
    async fn try_spawn_default_mode_lid(&self, key: u64, force: bool, context: &str) -> bool {
        let eligible = {
            let inner = self.inner.lock().await;
            language_id_enabled(&inner.config.language_id)
                && !language_id_continuous(&inner.config.language_id)
                && inner.record_ref(key).is_some_and(|r| {
                    !r.lid.identify_in_flight && (force || !r.lid.identify_started)
                })
        };
        if !eligible {
            return false;
        }
        if !force && self.tts_active_for_lid_defer().await {
            let mut inner = self.inner.lock().await;
            if crate::config::lid_tts_exclusion_enabled(
                &inner.config.language_id,
                &inner.config.tts,
            ) && language_id_enabled(&inner.config.language_id)
            {
                if let Some(rec) = inner.record_mut(key) {
                    rec.lid.identify_deferred = true;
                    rec.lid.identify_started = true;
                    voice_debug(format!("LID at {context} deferred (TTS active)"));
                    self.emit_language_id_skipped_once(rec, "deferred_tts");
                }
                return false;
            }
        }
        let started = self.spawn_identify_language(key, force).await;
        if started {
            voice_debug(format!(
                "LID at {context} starting identify in STT close window"
            ));
        }
        started
    }

    /// Waits until record `key` (or, with `None`, every record) has no identify running.
    async fn wait_lid_identify_complete(
        inner: &Arc<Mutex<AgentInner>>,
        notify: &Arc<Notify>,
        key: Option<u64>,
    ) {
        loop {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let guard = inner.lock().await;
            let in_flight = match key {
                Some(key) => guard
                    .record_ref(key)
                    .is_some_and(|r| r.lid.identify_in_flight),
                None => guard.any_lid_in_flight(),
            };
            drop(guard);
            if !in_flight {
                return;
            }
            notified.await;
        }
    }

    /// Bounded wait for the LID gate of record `key` (`None`: every record).
    async fn await_lid_gate_bounded(&self, key: Option<u64>) {
        let gate_ms = {
            let inner = self.inner.lock().await;
            if !crate::config::lid_tts_exclusion_enabled(
                &inner.config.language_id,
                &inner.config.tts,
            ) {
                return;
            }
            let in_flight = match key {
                Some(key) => inner
                    .record_ref(key)
                    .is_some_and(|r| r.lid.identify_in_flight),
                None => inner.any_lid_in_flight(),
            };
            if !in_flight {
                return;
            }
            inner
                .config
                .language_id
                .as_ref()
                .map(crate::config::resolved_lid_gate_max_wait_ms)
                .unwrap_or(3000)
        };
        let notify = Arc::clone(&self.lid_completion_notify);
        let inner = Arc::clone(&self.inner);
        let bound_hits = Arc::clone(&self.lid_gate_bound_hits);
        let wait = Self::wait_lid_identify_complete(&inner, &notify, key);
        if tokio::time::timeout(std::time::Duration::from_millis(gate_ms as u64), wait)
            .await
            .is_err()
        {
            bound_hits.fetch_add(1, Ordering::SeqCst);
            voice_debug(format!(
                "LID gate bound hit after {} ms — emitting utterance close while identify continues",
                gate_ms
            ));
        }
    }

    async fn await_lid_gate_before_utterance_close(&self, key: u64) {
        self.await_lid_gate_bounded(Some(key)).await;
    }

    async fn await_in_flight_lid_before_tts(
        weak_self: &Weak<VoiceAgent>,
        inner: &Arc<Mutex<AgentInner>>,
        notify: &Arc<Notify>,
    ) {
        let (in_flight, exclusion) = {
            let guard = inner.lock().await;
            (
                guard.any_lid_in_flight(),
                crate::config::lid_tts_exclusion_enabled(
                    &guard.config.language_id,
                    &guard.config.tts,
                ),
            )
        };
        if !in_flight || !exclusion {
            return;
        }
        if let Some(agent) = weak_self.upgrade() {
            agent.await_lid_gate_bounded(None).await;
            return;
        }
        Self::wait_lid_identify_complete(inner, notify, None).await;
    }

    async fn maybe_start_lid_at_speech_end(&self) {
        let key = self.inner.lock().await.live_key();
        if let Some(key) = key {
            self.try_spawn_default_mode_lid(key, false, "SpeechEnd")
                .await;
        }
    }

    async fn tts_active_for_lid_defer(&self) -> bool {
        if self.tts_synthesis_busy.load(Ordering::SeqCst) {
            return true;
        }
        if !self.tts_synthesis_queue.lock().await.is_empty() {
            return true;
        }
        self.inner.lock().await.agent_speaking
    }

    /// Starts a background identify for record `key` when its buffered speech is long enough
    /// (or `force`). Does not await inference — inbound PCM and STT/TTS must not block on LID.
    /// The outcome is written back to the record by key, whether it is still live or already
    /// closing. Returns `true` when a job was spawned.
    async fn spawn_identify_language(&self, key: u64, force: bool) -> bool {
        let provider = {
            let guard = self.language_id.lock().await;
            guard.as_ref().map(Arc::clone)
        };
        let Some(provider) = provider else {
            return false;
        };

        if self.tts_active_for_lid_defer().await {
            let mut inner = self.inner.lock().await;
            if crate::config::lid_tts_exclusion_enabled(
                &inner.config.language_id,
                &inner.config.tts,
            ) && language_id_enabled(&inner.config.language_id)
            {
                if let Some(rec) = inner.record_mut(key) {
                    rec.lid.identify_deferred = true;
                    rec.lid.identify_started = true;
                    voice_debug("LID identify deferred (TTS active)");
                    self.emit_language_id_skipped_once(rec, "deferred_tts");
                }
                return false;
            }
        }

        let job = {
            let mut inner = self.inner.lock().await;
            if !language_id_enabled(&inner.config.language_id) {
                return false;
            }
            let continuous = language_id_continuous(&inner.config.language_id);
            let allowlist_cfg = inner.config.language_id.clone().expect("enabled");
            let min_ms = resolved_language_id_min_speech_ms(&allowlist_cfg);
            let max_clip_ms = crate::config::resolved_lid_max_clip_ms(&allowlist_cfg);
            let Some(rec) = inner.record_mut(key) else {
                return false;
            };
            if rec.lid.identify_in_flight {
                rec.lid.identify_started = true;
                return false;
            }
            if rec.lid.identify_started && !force && !continuous {
                return false;
            }
            let buffer_len = rec.lid.pcm_buffer.len();
            let buffered_ms = crate::pcm::duration_ms_from_mono_s16le(
                buffer_len,
                crate::pcm::STT_PCM_SAMPLE_RATE,
            );
            let duration_ms = rec.lid.voiced_ms;
            if buffer_len == 0 || (!force && duration_ms < min_ms) {
                if !force && buffer_len > 0 && duration_ms < min_ms {
                    voice_debug(format!(
                        "LID identify skipped: {duration_ms} ms speech ({buffered_ms} ms buffered) < min_speech_ms {min_ms}"
                    ));
                }
                return false;
            }
            rec.lid.identify_deferred = false;
            rec.lid.identify_in_flight = true;
            rec.lid.identify_started = true;
            if !continuous {
                rec.lid.buffering = false;
            }
            let sample_rate = crate::pcm::STT_PCM_SAMPLE_RATE;
            let max_bytes = (sample_rate as u64)
                .saturating_mul(max_clip_ms as u64)
                .saturating_mul(2)
                / 1000;
            let buffer = std::mem::take(&mut rec.lid.pcm_buffer);
            let pcm = if buffer.len() > max_bytes as usize {
                Bytes::copy_from_slice(&buffer[..max_bytes as usize])
            } else {
                Bytes::from(buffer)
            };
            rec.lid.pcm_buffer.clear();
            rec.lid.voiced_ms = 0;
            rec.lid.pending_silence_ms = 0;
            Some((pcm, allowlist_cfg, duration_ms))
        };
        let Some((pcm, allowlist_cfg, speech_ms)) = job else {
            return false;
        };
        let Some(agent) = self.weak_self.upgrade() else {
            let mut inner = self.inner.lock().await;
            if let Some(rec) = inner.record_mut(key) {
                rec.lid.identify_in_flight = false;
            }
            return false;
        };
        let sample_rate = crate::pcm::STT_PCM_SAMPLE_RATE;
        let lid_notify = Arc::clone(&agent.lid_completion_notify);
        tokio::spawn(async move {
            let result = provider.identify(pcm, sample_rate).await;
            let Some(agent) = agent.weak_self.upgrade() else {
                return;
            };
            let mut inner = agent.inner.lock().await;
            if inner.record_ref(key).is_none() {
                // The record is gone (session reset or STT disabled): nothing to write to.
                voice_debug("LID result for a previous utterance ignored");
                drop(inner);
                lid_notify.notify_waiters();
                return;
            }
            // The first outcome per utterance is always reported: `user_language`, or
            // `language_id_skipped` (`undetermined`) when no usable code came back.
            let mut undetermined = false;
            match result {
                Ok(Some(lang_result)) => {
                    let code = lang_result.language.trim().to_ascii_lowercase();
                    let language_emitted = inner
                        .record_ref(key)
                        .is_some_and(|r| r.lid.language_emitted);
                    if code.is_empty() {
                        voice_debug("LID identify returned empty language code");
                        undetermined = true;
                    } else if !language_id_allowlist_accepts(&allowlist_cfg, &code) {
                        voice_debug(format!("LID result {code} not in allowlist — ignored"));
                        undetermined = true;
                    } else if inner.lid_last_emitted.as_deref() == Some(code.as_str())
                        && language_emitted
                    {
                        voice_debug(format!("LID result {code} unchanged — not re-emitted"));
                    } else {
                        inner.lid_last_emitted = Some(code.clone());
                        let rec = inner.record_mut(key).expect("checked");
                        rec.lid.language_emitted = true;
                        voice_debug(format!("emit user_language: {code}"));
                        let utterance_id = Some(rec.id.clone());
                        agent.emit(SpeechEvent::user_language(code, utterance_id));
                    }
                }
                Ok(None) => {
                    undetermined = true;
                }
                Err(err) => {
                    voice_debug(format!("LID identify failed: {err}"));
                    undetermined = true;
                }
            }
            let rec = inner.record_mut(key).expect("checked");
            if undetermined && !rec.lid.language_emitted {
                agent.emit_language_id_skipped_with_ms(rec, "undetermined", speech_ms);
            }
            rec.lid.identify_in_flight = false;
            rec.lid.identify_deferred = false;
            inner.lid_retained.retain(|r| r.key != key);
            drop(inner);
            lid_notify.notify_waiters();
        });
        true
    }

    async fn maybe_spawn_deferred_lid_after_tts(
        weak_self: &Weak<VoiceAgent>,
        _tts_synthesis_queue: &Arc<Mutex<VecDeque<TtsSynthesisJob>>>,
        inner: &Arc<Mutex<AgentInner>>,
    ) {
        let keys = {
            let guard = inner.lock().await;
            if language_id_enabled(&guard.config.language_id) {
                guard.deferred_lid_keys()
            } else {
                Vec::new()
            }
        };
        if keys.is_empty() {
            return;
        }
        // After drain, `synthesis_busy` may still be true while the worker waits in
        // `wait_job_playback_idle` — use playback/queue state only, not synthesis_busy.
        if let Some(agent) = weak_self.upgrade() {
            if agent.tts_active_for_lid_defer().await {
                let mut guard = inner.lock().await;
                for key in keys {
                    if let Some(rec) = guard.record_mut(key) {
                        rec.lid.identify_deferred = true;
                    }
                }
                return;
            }
            voice_debug("LID identify running after TTS drain");
            for key in keys {
                agent.spawn_identify_language(key, true).await;
            }
        }
    }

    /// Emits `user_speaking_start` for record `key` once. The replay ring and LID buffering start
    /// with the live utterance.
    async fn emit_user_speaking_start_if_needed(&self, key: u64) {
        let utterance_id = {
            let mut inner = self.inner.lock().await;
            if !inner.stt_enabled {
                return;
            }
            let is_live = inner.live_key() == Some(key);
            let Some(rec) = inner.record_mut(key) else {
                return;
            };
            if rec.speaking_start_emitted {
                return;
            }
            rec.speaking_start_emitted = true;
            let id = rec.id.clone();
            if is_live {
                if inner.replay_begun_at_pre_roll {
                    // Ring already holds the pre-roll/onset audio flushed at SpeechStart.
                    inner.replay_begun_at_pre_roll = false;
                } else {
                    inner.utterance_replay_buffer.begin_utterance();
                }
            }
            id
        };
        voice_debug("emit user_speaking_start (before STT transcript)");
        self.emit(SpeechEvent::user_speaking_start(utterance_id));
        let is_live = self.inner.lock().await.live_key() == Some(key);
        if is_live {
            self.start_lid_buffering().await;
        }
    }

    /// Processes one inbound WebRTC PCM frame (typically 20 ms stereo 48 kHz).
    ///
    /// Runs VAD transitions, barge-in and STT gate/hold/finalize. STT work is handed to the
    /// agent's STT actor as commands; this call never waits for the STT vendor. Results
    /// (partials, finals) are emitted by the agent's result pump, so they may follow the call that
    /// fed the audio; [`wait_stt_idle`](Self::wait_stt_idle) waits for them. No-op when
    /// [`stop`](Self::stop) has run. Set `VOICE_DEBUG=1` for per-frame diagnostics.
    pub async fn process_inbound_pcm(&self, pcm: Bytes, duration_ms: u32) -> SpeechResult<()> {
        self.process_inbound_pcm_with_meta(pcm, duration_ms, false)
            .await
    }

    /// Like [`process_inbound_pcm`](Self::process_inbound_pcm), with a `concealed` flag: true when
    /// the frame was synthesized by loss concealment. Concealed frames do not consume the STT
    /// pre-roll's received-audio budget.
    pub async fn process_inbound_pcm_with_meta(
        &self,
        pcm: Bytes,
        duration_ms: u32,
        concealed: bool,
    ) -> SpeechResult<()> {
        let _in_flight = InboundInFlight::enter(self);
        // One whole call at a time with pumped STT results, ticker checks and host APIs.
        let _step = self.utterance_step.lock().await;
        let call = INBOUND_PCM_FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
        if voice_debug_enabled() && (call == 1 || call.is_multiple_of(50)) {
            voice_debug(format!(
                "process_inbound_pcm call={call} bytes={} duration_ms={duration_ms}",
                pcm.len()
            ));
        }

        let running = {
            let inner = self.inner.lock().await;
            inner.running
        };
        if !running {
            voice_debug(format!(
                "process_inbound_pcm call={call} skipped: agent not running"
            ));
            return Ok(());
        }

        {
            let mut inner = self.inner.lock().await;
            inner.last_inbound_pcm_at = Some(Instant::now());
            inner.utterance_media_ms = inner.utterance_media_ms.saturating_add(duration_ms as u64);
            if inner.stt_listen_deadline_ms > 0 && inner.stt_listen_started_at.is_some() {
                inner.stt_listen_media_ms = inner.stt_listen_media_ms.saturating_add(duration_ms);
            }
        }

        let pcm = {
            let mut inner = self.inner.lock().await;
            if let Some(denoiser) = inner.denoise.as_mut() {
                Bytes::from(denoiser.process_s16le_stereo(pcm.as_ref()))
            } else {
                pcm
            }
        };

        let mono = crate::pcm::stereo_48k_to_mono_16k(pcm.as_ref());
        let mono_bytes = i16_samples_to_bytes(&mono);

        let (
            transitions,
            gate_stt,
            speech_start,
            complete_previous_utterance,
            frame_active,
            vad_pending,
            vad_speaking,
        ) = {
            let mut inner = self.inner.lock().await;
            let gate_stt = inner.config.vad.gate_stt;
            if let Some(tap) = inner.inbound_tap.as_mut() {
                tap.push(&mono_bytes);
            }

            let (transitions, frame_active) = match inner.vad.as_mut() {
                Some(vad) => vad.process_webrtc_pcm(pcm.as_ref(), duration_ms)?,
                // No VAD: every frame counts as voiced (LID `minSpeechMs` keeps buffer-length behaviour).
                None => (Vec::new(), true),
            };
            if transitions.contains(&VadTransition::SpeechEnd) {
                if let Some(tap) = inner.inbound_tap.as_mut() {
                    tap.flush_segment();
                }
            }

            let vad_pending = inner
                .vad
                .as_ref()
                .map(VadEngine::is_pending_speech)
                .unwrap_or(false);
            let vad_speaking = inner.vad.as_ref().map(|v| v.is_speaking()).unwrap_or(false);

            let speech_start = transitions.contains(&VadTransition::SpeechStart);
            let mut complete_previous_utterance = false;

            if transitions.contains(&VadTransition::SpeechEnd) {
                // Gate hold (and deferred speaking_end) still runs for VAD-only agents
                // (`stt: None`). Runtime `set_stt_enabled(false)` is the suppress path.
                if gate_stt && inner.stt_enabled {
                    // Pre-roll ring is not cleared here: it only holds frames not sent to STT;
                    // capacity cap drops stale audio after SpeechEnd/hold without a separate clear.
                    let hold_ms = inner.config.vad.stt_gate_hold_ms;
                    inner.stt_gate_hold_ms = hold_ms;
                    let ctx = inner.otel.session_context.clone();
                    otel::record_gate_hold_start(&ctx, hold_ms);
                    inner.stt_finalize_pending = true;
                    if inner.live_flag(|r| r.partials_emitted && !r.final_emitted) {
                        inner.defer_utterance_finalize_until_hold = true;
                        inner.utterance_finalize_deadline_ms = 0;
                        inner.utterance_finalize_armed_at = None;
                    }
                    voice_debug(format!(
                        "STT gate hold: {} ms after speech end{}",
                        inner.stt_gate_hold_ms,
                        if inner.agent_speaking {
                            " (agent TTS playing — finalize after playback)"
                        } else {
                            ""
                        }
                    ));
                } else if inner.live_flag(|r| r.partials_emitted && !r.final_emitted) {
                    Self::arm_utterance_finalize_timer(&mut inner);
                }
            } else if inner.stt_gate_hold_ms > 0 && !speech_start {
                // Brief gap (counting): cancel hold. Long pause then new speech: finish prior phrase.
                // Use declared VAD speech only — `frame_active` can flicker on TTS tail / echo during hold.
                let user_voice_active = vad_speaking;
                if user_voice_active && !inner.stt_endpoint_closing_started {
                    let hold_total = inner.config.vad.stt_gate_hold_ms;
                    let hold_elapsed = hold_total.saturating_sub(inner.stt_gate_hold_ms);
                    let long_pause_before_resume =
                        gate_hold_long_pause_elapsed(hold_total, hold_elapsed);
                    if inner.stt_finalize_pending
                        && !inner.live_flag(|r| r.final_emitted)
                        && long_pause_before_resume
                    {
                        complete_previous_utterance = true;
                        voice_debug(
                            "STT: voice resumed after long gate hold (≥90% elapsed) — completing previous utterance",
                        );
                    } else {
                        inner.stt_gate_hold_ms = 0;
                        inner.stt_finalize_pending = false;
                        inner.defer_utterance_finalize_until_hold = false;
                        Self::disarm_utterance_finalize_timer(&mut inner);
                        voice_debug(
                            "STT gate hold cancelled: voice active again before hold expired",
                        );
                    }
                } else {
                    let before = inner.stt_gate_hold_ms;
                    inner.stt_gate_hold_ms = before.saturating_sub(duration_ms);
                    let after = inner.stt_gate_hold_ms;
                    if before > 0 && after == 0 {
                        let ctx = inner.otel.session_context.clone();
                        otel::record_gate_hold_end(&ctx);
                        voice_debug(
                            "STT gate hold expired — utterance may finalize on next inbound frame",
                        );
                        if inner.defer_utterance_finalize_until_hold
                            && inner.live_flag(|r| r.partials_emitted && !r.final_emitted)
                        {
                            inner.defer_utterance_finalize_until_hold = false;
                            Self::arm_utterance_finalize_timer(&mut inner);
                        }
                    } else if before > 0 && after > 0 && (before / 500) != (after / 500) {
                        voice_debug(format!("STT gate hold: {after} ms remaining"));
                    }
                }
            }

            (
                transitions,
                gate_stt,
                speech_start,
                complete_previous_utterance,
                frame_active,
                vad_pending,
                vad_speaking,
            )
        };

        // Append after the VAD decision so the LID buffer knows whether this frame was voiced.
        // `frame_active` is the per-frame VAD decision (excludes hangover). SpeechStart handling
        // below starts buffering after this point, so the onset frame is not appended (as before).
        self.append_lid_pcm_if_buffering(&mono_bytes, frame_active, vad_speaking, duration_ms)
            .await;

        // C1 / C2 timeout ticks (only when VAD enabled and STT stream lifecycle active).
        let backlog_ms = self.stt_backlog_ms();
        let open_pending = self.stt_open_pending();
        let (c1_expired, c2_expired) = {
            let mut inner = self.inner.lock().await;
            if !inner.config.vad.enabled || !Self::stt_pipeline_active(&inner) {
                (false, false)
            } else {
                let mut c1 = false;
                let mut c2 = false;
                if inner.live_flag(|r| r.stream_open && !r.partials_emitted)
                    && inner.stt_listen_started_at.is_some()
                    && inner.stt_hold.is_none()
                {
                    // C1 also waits while an earlier utterance is still waiting for its final
                    // (rt.stt-terminal-before-late-final): bounded, every closing entry is
                    // answered or timed out by the STT actor.
                    let hold_for_vendor = open_pending || !inner.closing.is_empty();
                    if Self::c1_hold_for_stt_open(&mut inner, hold_for_vendor) {
                        // Not expired: the STT stream open is still being retried.
                    } else if Self::c1_listen_expired(&inner, backlog_ms, std::time::Duration::ZERO)
                    {
                        c1 = true;
                    } else {
                        Self::maybe_log_c1_backlog_deferred(&mut inner, backlog_ms);
                    }
                }
                if inner.utterance_finalize_deadline_ms > 0
                    && !inner.defer_utterance_finalize_until_hold
                    && !Self::vad_is_speaking(&inner)
                    && inner.stt_gate_hold_ms == 0
                {
                    inner.utterance_finalize_deadline_ms = inner
                        .utterance_finalize_deadline_ms
                        .saturating_sub(duration_ms);
                    if inner.utterance_finalize_deadline_ms == 0 {
                        c2 = true;
                    }
                }
                (c1, c2)
            }
        };

        if c1_expired {
            finalize_trace("c1", &*self.inner.lock().await);
            self.close_stt_stream_not_found().await?;
        }
        if c2_expired {
            finalize_trace("media_c2", &*self.inner.lock().await);
            self.force_close_utterance("finalize_timeout").await?;
        }

        {
            let mut guard = self.inner.lock().await;
            let ctx = guard.otel.session_context.clone();
            for transition in &transitions {
                otel::record_vad_transition(&ctx, transition);
                let now = Instant::now();
                match transition {
                    VadTransition::SpeechStart => {
                        if guard.turn_latency.on_speech_start(now).is_some() {
                            otel::record_voice_turn_response_abandoned(Self::project_id_label(
                                &guard,
                            ));
                        }
                    }
                    VadTransition::SpeechEnd => guard.turn_latency.on_speech_end(now),
                }
            }
        }

        if complete_previous_utterance {
            let stt_active = {
                let inner = self.inner.lock().await;
                Self::stt_pipeline_active(&inner)
            };
            if stt_active {
                self.complete_pending_utterance_if_any().await?;
            }
        }

        let (stt_active, stt_enabled) = {
            let inner = self.inner.lock().await;
            (Self::stt_pipeline_active(&inner), inner.stt_enabled)
        };

        if speech_start && stt_active {
            // Onset frames held back during the gate hold belong to the NEW phrase. Take them
            // before the old stream is finalized so its final never contains them.
            let held_onset = {
                let mut inner = self.inner.lock().await;
                std::mem::take(&mut inner.hold_pending_pcm)
            };
            let long_pause_new_phrase = {
                let inner = self.inner.lock().await;
                if !inner.config.vad.gate_stt {
                    true
                } else {
                    let hold_total = inner.config.vad.stt_gate_hold_ms;
                    let hold_elapsed = hold_total.saturating_sub(inner.stt_gate_hold_ms);
                    inner.stt_endpoint_closing_started
                        || (hold_total > 0 && hold_elapsed > hold_total / 2)
                }
            };
            if long_pause_new_phrase {
                self.complete_pending_utterance_if_any().await?;
            }
            let mut pre_roll_after_start = None;
            let mut held_same_stream: Option<Bytes> = None;
            {
                let mut inner = self.inner.lock().await;
                if long_pause_new_phrase
                    || inner.live.is_none()
                    || inner.live_flag(|r| r.final_emitted)
                {
                    // A completed final always starts a new utterance — the brief-gap path is only
                    // for mid-phrase pauses (e.g. counting). Leaving `stt_final_emitted` set blocks
                    // C2 arming and `should_finalize_utterance` on turn 2+ (local multi-turn E2E).
                    // Results of the previous utterance that arrive later are matched to their own
                    // record by key; they never reach this one.
                    Self::start_new_live_record(&mut inner);
                } else {
                    // Brief gap (e.g. counting): same utterance — clear hold only.
                    inner.stt_gate_hold_ms = 0;
                    inner.stt_finalize_pending = false;
                    inner.stt_endpoint_closing_started = false;
                    inner.defer_utterance_finalize_until_hold = false;
                    Self::disarm_utterance_finalize_timer(&mut inner);
                }
            }
            {
                let mut inner = self.inner.lock().await;
                if gate_stt {
                    // Cold inbound (no frames before the talker) leaves a short ring at
                    // SpeechStart; streaming recognizers need left context before onset.
                    // Pad only when opening a new STT stream — not for brief-gap continuations.
                    let new_stream = !inner.live_flag(|r| r.stream_open);
                    let mut held_onset = held_onset;
                    if !held_onset.is_empty() {
                        voice_debug(format!(
                            "STT hold-back: {} ms onset → {}",
                            crate::pcm::duration_ms_from_mono_s16le(
                                held_onset.len(),
                                crate::pcm::STT_PCM_SAMPLE_RATE,
                            ),
                            if new_stream {
                                "new stream pre-roll"
                            } else {
                                "same stream"
                            },
                        ));
                        if new_stream {
                            if let Some(pre_roll) = inner.stt_pre_roll.as_mut() {
                                pre_roll.push(&held_onset);
                                held_onset = Vec::new();
                            }
                        }
                        if !held_onset.is_empty() {
                            held_same_stream = Some(Bytes::from(held_onset));
                        }
                    }
                    let mut burst_head: Option<(Bytes, bool)> = None;
                    if new_stream {
                        let capacity_ms = inner
                            .stt_pre_roll
                            .as_ref()
                            .map_or(0, SttPreRollBuffer::capacity_ms);
                        if let Some((head, contiguous)) =
                            inner.short_burst.take_for_new_stream(capacity_ms)
                        {
                            voice_debug(format!(
                                "STT short-burst kept: {} ms before pre-roll ({})",
                                crate::pcm::duration_ms_from_mono_s16le(
                                    head.len(),
                                    crate::pcm::STT_PCM_SAMPLE_RATE,
                                ),
                                if contiguous {
                                    "contiguous"
                                } else {
                                    "separated"
                                },
                            ));
                            burst_head = Some((head, contiguous));
                        }
                    } else {
                        inner.short_burst.clear();
                    }
                    if let Some(pre_roll) = inner.stt_pre_roll.as_mut() {
                        let buffered_len = pre_roll.len();
                        let mut buffered = if new_stream && burst_head.is_none() {
                            pre_roll.drain_padded_to_capacity()
                        } else {
                            pre_roll.drain()
                        };
                        if let Some((head, contiguous)) = burst_head {
                            let mut joined = Vec::with_capacity(head.len() + buffered.len() + 6400);
                            joined.extend_from_slice(&head);
                            if !contiguous {
                                let sep = (crate::pcm::STT_PCM_SAMPLE_RATE as usize
                                    * SHORT_BURST_SEPARATOR_MS as usize
                                    / 1000)
                                    * 2;
                                joined.resize(joined.len() + sep, 0);
                            }
                            joined.extend_from_slice(&buffered);
                            buffered = Bytes::from(joined);
                        }
                        if !buffered.is_empty() {
                            let total_ms = crate::pcm::duration_ms_from_mono_s16le(
                                buffered.len(),
                                crate::pcm::STT_PCM_SAMPLE_RATE,
                            );
                            let buffered_ms = crate::pcm::duration_ms_from_mono_s16le(
                                buffered_len,
                                crate::pcm::STT_PCM_SAMPLE_RATE,
                            );
                            let pad_ms = total_ms.saturating_sub(buffered_ms);
                            voice_debug(format!(
                                "STT pre-roll flush: {} bytes (~{} ms, lead-in pad {} ms)",
                                buffered.len(),
                                total_ms,
                                pad_ms,
                            ));
                            pre_roll_after_start = Some(buffered);
                        }
                    }
                }
            }
            if let Some(buffered) = pre_roll_after_start {
                if !buffered.is_empty() {
                    {
                        let mut inner = self.inner.lock().await;
                        if inner.stt_enabled && inner.stt_hold.is_none() {
                            inner.utterance_replay_buffer.begin_utterance();
                            inner.replay_begun_at_pre_roll = true;
                        }
                    }
                    self.push_stt_audio_bytes(buffered).await?;
                }
            }
            if let Some(held) = held_same_stream {
                self.push_stt_audio_frames(held).await?;
            }
        }

        if !speech_start {
            if let Some(gate) = self.speaker_gate.as_ref() {
                gate.push_frame(&mono_bytes);
            }
        }
        let mut speech_end_transition = false;
        for transition in &transitions {
            voice_debug(format!("VAD {transition:?}"));
            match transition {
                VadTransition::SpeechStart => {
                    // VAD-only agents (`stt: None`) do not start a record on SpeechStart via the
                    // STT long-pause path — start one here so each VAD turn has its own id and can
                    // buffer and identify at the next user_speaking_end.
                    {
                        let mut inner = self.inner.lock().await;
                        if !Self::stt_pipeline_active(&inner) {
                            let rec = inner.new_record();
                            inner.live = Some(rec);
                        }
                    }
                    if let Some(gate) = self.speaker_gate.as_ref() {
                        let key = self.inner.lock().await.ensure_live().key;
                        gate.begin(key, &mono_bytes);
                    }
                    self.on_vad_speech_start().await?;
                    // VAD speaking_start is independent of having an STT vendor (tests with
                    // stt: None still expect it). setSttEnabled(false) is gated inside emit.
                    let key = self.inner.lock().await.ensure_live().key;
                    self.emit_user_speaking_start_if_needed(key).await;
                }
                VadTransition::SpeechEnd => {
                    speech_end_transition = true;
                    if let Some(gate) = self.speaker_gate.as_ref() {
                        gate.end_utterance();
                    }
                    self.maybe_start_lid_at_speech_end().await;
                    // Runtime toggle only — VAD-only (`stt: None`) still emits speaking_end.
                    if !stt_enabled {
                        voice_debug("user_speaking_end suppressed (STT disabled)");
                    } else {
                        let (has_stt, defer_speaking_end, agent_speaking) = {
                            let inner = self.inner.lock().await;
                            (
                                Self::stt_pipeline_active(&inner),
                                inner.config.vad.gate_stt && !inner.agent_speaking,
                                inner.agent_speaking,
                            )
                        };
                        if agent_speaking {
                            voice_debug(
                                "user_speaking_end suppressed (VAD SpeechEnd during agent TTS)",
                            );
                        } else if has_stt {
                            voice_debug(
                            "user_speaking_end deferred until user_speech_final (STT utterance close)",
                        );
                        } else if defer_speaking_end {
                            voice_debug(
                            "user_speaking_end deferred until STT gate hold expires (gate_stt, no STT)",
                        );
                        } else {
                            let key = self.inner.lock().await.live_key();
                            if let Some(key) = key {
                                self.emit_user_speaking_end_after_lid_gate(key).await;
                            }
                        }
                    }
                }
            }
        }

        if !stt_enabled {
            return Ok(());
        }

        // After VAD transitions (SpeechStart opens STT stream via `on_vad_speech_start`).
        let (
            stt_audio_open,
            stt_poll_open,
            should_finalize_utterance,
            hold_back,
            held_excess,
            held_resolved_to_stream,
        ) = {
            let mut inner = self.inner.lock().await;
            let pending_gate = inner.config.vad.gate_stt_open_on_pending && vad_pending;
            let utterance_closing =
                inner.stt_finalize_pending && !inner.live_flag(|r| r.final_emitted);
            let vad_enabled = inner.config.vad.enabled;
            let stt_audio_open = if !vad_enabled {
                true
            } else if gate_stt {
                if !inner.live_flag(|r| r.stream_open) {
                    false
                } else {
                    vad_speaking || inner.stt_gate_hold_ms > 0 || pending_gate || utterance_closing
                }
            } else {
                true
            };
            let stt_poll_open = !gate_stt
                || stt_audio_open
                || utterance_closing
                || inner.live_flag(|r| r.stream_open)
                || inner.utterance_finalize_deadline_ms > 0;
            let should_finalize_utterance = gate_stt
                && utterance_closing
                && !inner.agent_speaking
                && !inner.stt_endpoint_closing_started
                && inner.stt_gate_hold_ms == 0
                && !vad_speaking;

            // Onset of the next phrase during a gate hold / pending close: hold it back so the
            // closing stream does not count it (it belongs to the stream that starts next).
            let hold_back = gate_stt
                && stt_active
                && inner.live_flag(|r| r.stream_open)
                && vad_pending
                && !vad_speaking
                && !speech_start
                && (inner.stt_gate_hold_ms > 0 || utterance_closing);
            let mut held_excess: Option<Bytes> = None;
            let mut held_resolved_to_stream: Option<Bytes> = None;
            if hold_back {
                inner
                    .hold_pending_pcm
                    .extend_from_slice(mono_bytes.as_ref());
                let cap_ms = inner.config.vad.stt_gate_hold_ms.saturating_add(
                    crate::stt_pre_roll::stt_pre_roll_capacity_ms(&inner.config.vad),
                );
                let cap_bytes =
                    (crate::pcm::STT_PCM_SAMPLE_RATE as usize * cap_ms as usize / 1000) * 2;
                if inner.hold_pending_pcm.len() > cap_bytes {
                    // Never drop a frame: the oldest excess goes to the (open, old) stream.
                    let excess_len = (inner.hold_pending_pcm.len() - cap_bytes) & !1;
                    let excess: Vec<u8> = inner.hold_pending_pcm.drain(..excess_len).collect();
                    held_excess = Some(Bytes::from(excess));
                }
            } else if !inner.hold_pending_pcm.is_empty() {
                // Hold ended without a SpeechStart (finalize, C1/C2, toggle, swap): resolve it.
                let held = std::mem::take(&mut inner.hold_pending_pcm);
                if inner.live_flag(|r| r.stream_open) {
                    held_resolved_to_stream = Some(Bytes::from(held));
                } else if let Some(pre_roll) = inner.stt_pre_roll.as_mut() {
                    pre_roll.push(&held);
                }
            }
            (
                stt_audio_open,
                stt_poll_open,
                should_finalize_utterance,
                hold_back,
                held_excess,
                held_resolved_to_stream,
            )
        };
        if let Some(held) = held_resolved_to_stream {
            voice_debug("STT hold-back: onset → same stream (hold ended without speech start)");
            self.push_stt_audio_frames(held).await?;
        }
        if let Some(excess) = held_excess {
            self.push_stt_audio_frames(excess).await?;
        }

        // When gate is closed: skip STT poll. Continuous pre-roll still buffers every frame
        // not pushed directly to STT (silence, soft onset, agent-TTS listen).
        let gate_closed_skip_stt = gate_stt && !stt_poll_open && !should_finalize_utterance;
        let push_to_stt_directly = !gate_stt || stt_audio_open;

        if voice_debug_enabled() && (call == 1 || call.is_multiple_of(50)) {
            voice_debug(format!(
                "inbound PCM frame={call} bytes={} duration_ms={duration_ms}",
                pcm.len()
            ));
        }

        if hold_back {
            // Frame already appended to `hold_pending_pcm` above.
        } else if push_to_stt_directly {
            self.push_stt_audio_bytes(mono_bytes).await?;
        } else if gate_stt && stt_active {
            let mut inner = self.inner.lock().await;
            if let Some(pre_roll) = inner.stt_pre_roll.as_mut() {
                pre_roll.push_frame(&mono_bytes, concealed);
            }
            inner.short_burst.on_frame(
                &mono_bytes,
                frame_active && vad_pending && !vad_speaking,
                duration_ms,
            );
        }

        // STT results are not polled here: the STT actor polls the vendor and the result pump
        // emits what it returns.
        if gate_closed_skip_stt {
            if call == 1 || call.is_multiple_of(50) {
                voice_debug(format!(
                    "process_inbound_pcm call={call} skipped: gate_stt closed (STT poll deferred; pre-roll buffered)"
                ));
            }
            return Ok(());
        }

        if speech_end_transition {
            let mut inner = self.inner.lock().await;
            if !inner.agent_speaking {
                inner.barge_awaiting_stt_partial = false;
            }
        }

        if should_finalize_utterance {
            voice_debug(
                "STT should_finalize_utterance=true (gate hold done, agent idle, VAD not speaking)",
            );
            let started = {
                let mut inner = self.inner.lock().await;
                finalize_trace("gate_finalize", &inner);
                inner.stt_endpoint_closing_started = true;
                let tail_ms = stt_endpoint_tail_ms(&inner.config.vad);
                voice_debug(format!(
                    "STT utterance close: endpoint tail {tail_ms} ms then finalize (speaking_end with final)"
                ));
                self.begin_utterance_close(&mut inner, "end_of_utterance", true)
            };
            if matches!(started, CloseStarted::Held) {
                self.finish_live_inline(true).await;
            }
        }

        Ok(())
    }

    /// Push held-back onset audio to the open stream in 20 ms chunks, the granularity the same
    /// frames would have had if they had been pushed live.
    async fn push_stt_audio_frames(&self, mono_bytes: Bytes) -> SpeechResult<()> {
        const FRAME_BYTES: usize = 640; // 20 ms mono s16le 16 kHz
        let mut offset = 0;
        while offset < mono_bytes.len() {
            let end = (offset + FRAME_BYTES).min(mono_bytes.len());
            self.push_stt_audio_bytes(mono_bytes.slice(offset..end))
                .await?;
            offset = end;
        }
        Ok(())
    }

    /// Hands user PCM to the STT actor (or the host STT hold). Never waits for the vendor.
    async fn push_stt_audio_bytes(&self, mono_bytes: Bytes) -> SpeechResult<()> {
        {
            let mut inner = self.inner.lock().await;
            if !inner.stt_enabled {
                return Ok(());
            }
            if inner.stt_hold.is_some() {
                // Host hold: keep the PCM for the swapped STT; never feed the old model.
                let utterance_id = inner
                    .live
                    .as_ref()
                    .filter(|r| !r.terminal)
                    .map(|r| r.id.clone());
                let hold = inner.stt_hold.as_mut().expect("checked");
                if hold.replaces_utterance_id.is_none() {
                    hold.replaces_utterance_id = utterance_id;
                }
                hold.push(mono_bytes.as_ref());
                return Ok(());
            }
            inner.utterance_replay_buffer.push(mono_bytes.as_ref());
            if !inner.config.vad.enabled || !inner.config.vad.gate_stt {
                // Vendor-endpointed flows have no SpeechStart per utterance: the first audio
                // opens the record that the vendor's results will be applied to.
                inner.ensure_live();
            }
        }
        self.send_stt_audio(mono_bytes);
        Ok(())
    }

    // ---- STT results (applied by the result pump) ----------------------------------------

    /// The record a transcript is applied to, or why it is late.
    ///
    /// A transcript names its utterance by `key`. A transcript without a key belongs to audio the
    /// actor had not yet seen the end of: the oldest utterance still waiting for its
    /// `FinalizeDone` (its endpoint tail), else the live one.
    fn resolve_transcript_target(
        inner: &mut AgentInner,
        key: Option<u64>,
    ) -> Result<u64, (String, &'static str)> {
        match key {
            Some(key) => match inner.stt_record_ref(key) {
                Some(rec) if !rec.terminal => Ok(key),
                Some(rec) => Err((rec.id.clone(), rec.close_reason)),
                None => Err((format!("key {key}"), "closed")),
            },
            None => match inner.closing.front() {
                Some(front) if front.terminal => Err((front.id.clone(), front.close_reason)),
                Some(front) => Ok(front.key),
                None => Ok(inner.ensure_live().key),
            },
        }
    }

    /// The Live record whose terminal event `result` will emit, if any.
    async fn terminal_key_of(&self, result: &SttResult) -> Option<u64> {
        match result {
            SttResult::Transcript {
                key,
                transcript: SttTranscript::Final(_),
                ..
            } => {
                let mut inner = self.inner.lock().await;
                let key = Self::resolve_transcript_target(&mut inner, *key).ok()?;
                inner
                    .stt_record_ref(key)
                    .filter(|r| r.kind == UttKind::Live)
                    .map(|r| r.key)
            }
            SttResult::FinalizeDone { key, .. } => {
                let inner = self.inner.lock().await;
                inner
                    .closing
                    .iter()
                    .find(|r| r.key == *key && r.kind == UttKind::Live)
                    .map(|r| r.key)
            }
            _ => None,
        }
    }

    /// Applies one result of the STT actor, in the order the actor produced the results.
    async fn handle_stt_result(&self, result: SttResult) {
        // A result that ends an utterance first waits for that utterance's LID gate, without
        // the step lock: the inbound path never waits for LID because of a slow vendor.
        if let Some(key) = self.terminal_key_of(&result).await {
            self.prepare_lid_before_close(key).await;
        }
        let _step = self.utterance_step.lock().await;
        match result {
            SttResult::Transcript {
                key,
                gen,
                transcript,
            } => self.apply_transcript_result(key, gen, transcript).await,
            SttResult::FinalizeDone { key, timed_out, .. } => {
                self.apply_finalize_done(key, timed_out).await
            }
            SttResult::VendorError { message, .. } => {
                if !self.inner.lock().await.stt_enabled {
                    return;
                }
                eprintln!("[voice] STT vendor error: {message}");
                self.emit(SpeechEvent::error(format!("stt: {message}")));
            }
            SttResult::SwapApplied { config, result } => match result {
                Ok(()) => {
                    self.inner.lock().await.config.stt = Some(config.clone());
                    self.emit(SpeechEvent::stt_config_updated(&config));
                }
                Err(error) => eprintln!("[speech] stt config swap failed: {error}"),
            },
            SttResult::BarrierReached { done } => {
                let _ = done.send(());
            }
        }
    }

    async fn apply_transcript_result(&self, key: Option<u64>, gen: u64, transcript: SttTranscript) {
        let is_final = matches!(transcript, SttTranscript::Final(_));
        let kind = if is_final { "final" } else { "partial" };
        let target = {
            let mut inner = self.inner.lock().await;
            // STT disabled: results are ignored until it is enabled again.
            if !inner.stt_enabled {
                return;
            }
            let vendor = Self::active_stt_vendor_label(&inner);
            if inner
                .stt_hold
                .as_ref()
                .is_some_and(|hold| hold.gen_at_begin == gen)
            {
                // Old-model text must not surface while the hold is active.
                let log_key = key.or_else(|| inner.live_key()).unwrap_or(0);
                if inner.held_ignore_logged_key != Some(log_key) {
                    inner.held_ignore_logged_key = Some(log_key);
                    eprintln!("[voice] old-model STT result ignored during STT hold");
                }
                otel::record_voice_stt_late_result("held_old_model", vendor);
                return;
            }
            match Self::resolve_transcript_target(&mut inner, key) {
                Ok(target) => target,
                Err((utterance, closed_by)) => {
                    eprintln!(
                        "[voice] late STT result for closed utterance {utterance} ignored (kind={kind}, closed_by={closed_by})"
                    );
                    otel::record_voice_stt_late_result(kind, vendor);
                    return;
                }
            }
        };
        match transcript {
            SttTranscript::Partial(text) => self.apply_partial(target, text).await,
            SttTranscript::Final(text) => self.apply_final(target, text).await,
        }
    }

    async fn apply_partial(&self, key: u64, text: String) {
        voice_debug(format!("STT partial: {text}"));
        let utterance_id = {
            let mut inner = self.inner.lock().await;
            let is_live = inner.live_key() == Some(key);
            let Some(rec) = inner.record_mut(key) else {
                return;
            };
            if matches!(rec.kind, UttKind::Replay { .. }) {
                // A replay only produces its final; the original partials were emitted already.
                return;
            }
            rec.partials_emitted = true;
            rec.last_partial_text = Some(text.clone());
            let utterance_id = rec.id.clone();
            if is_live {
                Self::clear_stt_listen_timer(&mut inner);
                Self::refresh_utterance_finalize_after_partial(&mut inner);
            }
            utterance_id
        };
        self.emit_user_speaking_start_if_needed(key).await;
        // Partial must precede barge_in in the event stream (semantic roundtrip E2E).
        self.emit(SpeechEvent::user_speech_partial(
            text.clone(),
            Some(utterance_id),
        ));
        let _ = self.try_stt_gated_barge_in(key, &text).await;
    }

    async fn apply_final(&self, key: u64, text: String) {
        let (is_replay, is_live, vad_speaking) = {
            let inner = self.inner.lock().await;
            let Some(rec) = inner.record_ref(key) else {
                return;
            };
            (
                matches!(rec.kind, UttKind::Replay { .. }),
                inner.live_key() == Some(key),
                Self::vad_is_speaking(&inner),
            )
        };
        if is_replay {
            if text.trim().is_empty() {
                return;
            }
            voice_debug("emit user_speech_final (replay)");
            self.emit_utterance_final(key, text).await;
            return;
        }
        if text.trim().is_empty() && is_live && vad_speaking {
            voice_debug("ignore empty leftover user_speech_final while VAD still speaking");
            return;
        }
        voice_debug(format!("STT final: {text}"));
        finalize_trace("vendor_final", &*self.inner.lock().await);
        self.emit_user_speaking_start_if_needed(key).await;
        let _ = self.try_stt_gated_barge_in(key, &text).await;
        let (emit_speaking_end, close_stream, utterance_id) = {
            let mut inner = self.inner.lock().await;
            let Some(rec) = inner.record_mut(key) else {
                return;
            };
            rec.final_emitted = true;
            rec.vad_triggered = false;
            let emit_end = !rec.speaking_end_emitted;
            if emit_end {
                rec.speaking_end_emitted = true;
            }
            let close_stream = rec.stream_open || rec.user_stt_session_open;
            rec.stream_open = false;
            rec.user_stt_session_open = false;
            let utterance_id = rec.id.clone();
            if is_live {
                inner.stt_finalize_pending = false;
                inner.stt_endpoint_closing_started = false;
                Self::clear_stt_listen_timer(&mut inner);
                Self::clear_utterance_finalize_timer(&mut inner);
            }
            (emit_end, close_stream, utterance_id)
        };
        if close_stream {
            self.emit(
                SpeechEvent::stt_stream_end_with_reason("vendor_final")
                    .with_utterance_id(Some(utterance_id.clone())),
            );
            self.emit(SpeechEvent::user_stt_end().with_utterance_id(Some(utterance_id)));
        }
        if emit_speaking_end {
            voice_debug("emit user_speaking_end (paired with STT final)");
            self.emit_speaking_end(key, false).await;
        }
        voice_debug(format!(
            "emit user_speech_final: {}",
            if text.len() > 80 {
                format!("{}…", &text[..80])
            } else {
                text.clone()
            }
        ));
        self.emit_utterance_final(key, text).await;
    }

    /// The vendor answered the `EndUtterance` of `key` (or its wait timed out): apply the close.
    async fn apply_finalize_done(&self, key: u64, timed_out: bool) {
        let outcome = {
            let inner = self.inner.lock().await;
            // The live record (C1 with VAD still speaking) is closed later by the gate.
            let Some(rec) = inner.closing.iter().find(|r| r.key == key) else {
                return;
            };
            let has_text = rec
                .last_partial_text
                .as_deref()
                .is_some_and(|t| !t.trim().is_empty());
            if rec.final_emitted || rec.terminal || matches!(rec.kind, UttKind::Replay { .. }) {
                None
            } else if has_text {
                Some(("fallback final", rec.id.clone()))
            } else {
                Some(("not_found", rec.id.clone()))
            }
        };
        if timed_out {
            if let Some((what, id)) = &outcome {
                eprintln!("[voice] STT final timed out for {id}; closed with {what}");
            }
        }
        self.finish_record_close(key, false, false).await;
        let mut inner = self.inner.lock().await;
        if let Some(pos) = inner.closing.iter().position(|r| r.key == key) {
            if let Some(mut rec) = inner.closing.remove(pos) {
                let final_emitted = rec.final_emitted;
                rec.fire_terminal_waiter(final_emitted);
                inner.retain_for_lid(rec);
            }
        }
    }

    pub(crate) fn emit(&self, event: SpeechEvent) {
        let mode = self.events_mode();
        if matches!(
            mode,
            EventDeliveryMode::Callback | EventDeliveryMode::Stream | EventDeliveryMode::Both
        ) {
            self.event_bus.emit(event);
        }
    }

    async fn run_tts_drain(
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        weak_self: &Weak<VoiceAgent>,
        _tts_synthesis_busy: &Arc<AtomicBool>,
        tts_synthesis_queue: &Arc<Mutex<VecDeque<TtsSynthesisJob>>>,
    ) -> SpeechResult<()> {
        let (writer, tts_out_tap) = {
            let guard = inner.lock().await;
            (
                guard.pcm_writer.clone().ok_or(SpeechError::NotAttached)?,
                guard.tts_out_tap.clone(),
            )
        };

        let drain_generation = tts_buffer.current_generation().await;
        let mut pace = TtsDrainPaceState::new();
        // Resume mid-utterance drain passes without re-emitting agent_speaking_start.
        let mut agent_start_emitted = {
            let guard = inner.lock().await;
            guard.agent_speaking
        };
        let mut played_any = false;
        // Carry incomplete frames across progressive chunks / drain wakeups.
        // Do not pad mid-utterance — that inserts silence and drops STT words.
        let mut frame_carry = tts_buffer.take_frame_carry().await;

        // Popped chunks count as in flight until this pass is done writing them.
        let drain_hold = tts_buffer.drain_hold();

        loop {
            let Some(chunk) = drain_hold.pop_chunk().await else {
                // Progressive synth may still be producing — wait for the next wake
                // instead of ending the utterance early.
                if tts_buffer.is_producing().await
                    && tts_buffer.current_generation().await == drain_generation
                {
                    tts_buffer.store_frame_carry(frame_carry).await;
                    voice_debug("TTS drain paused (awaiting more progressive chunks)");
                    return Ok(());
                }
                break;
            };

            frame_carry.extend_from_slice(&chunk.pcm);
            let frames = take_complete_stereo_frames(&mut frame_carry);
            if frames.is_empty() {
                continue;
            }

            match Self::write_paced_tts_frames(
                &writer,
                tts_buffer,
                inner,
                event_bus,
                weak_self,
                drain_generation,
                &mut agent_start_emitted,
                &mut played_any,
                &mut pace,
                frames,
                None,
                tts_out_tap.as_ref(),
            )
            .await?
            {
                TtsDrainWrite::Continued => {}
                TtsDrainWrite::Stopped => return Ok(()),
            }
        }

        // End of utterance — pad any trailing partial frame once (same as buffered path).
        if !frame_carry.is_empty()
            && !tts_buffer.is_producing().await
            && tts_buffer.current_generation().await == drain_generation
        {
            let (frame, duration_ms) = pad_stereo_frame_20ms(&frame_carry);
            // Only the real PCM counts for the reply ledger, not the padding.
            let real_bytes = frame_carry.len();
            frame_carry.clear();
            match Self::write_paced_tts_frames(
                &writer,
                tts_buffer,
                inner,
                event_bus,
                weak_self,
                drain_generation,
                &mut agent_start_emitted,
                &mut played_any,
                &mut pace,
                vec![(frame, duration_ms)],
                Some(real_bytes),
                tts_out_tap.as_ref(),
            )
            .await?
            {
                TtsDrainWrite::Continued => {}
                TtsDrainWrite::Stopped => return Ok(()),
            }
        }
        tts_buffer.store_frame_carry(frame_carry).await;
        // Everything this pass held has been written; agent_speaking (if any) carries idle state.
        drop(drain_hold);
        // A reply marked complete after its last frame was written ends here.
        if tts_buffer.current_generation().await == drain_generation {
            Self::emit_reply_transitions(event_bus, tts_buffer.consume_played(0).await);
        }

        if played_any && agent_start_emitted && !tts_buffer.is_producing().await {
            if Self::end_agent_speaking_inner(inner, true).await {
                voice_debug("agent_speaking=false (TTS drained)");
                event_bus.emit(SpeechEvent::agent_speaking_end());
            } else {
                // A barge-in/flush path already ended speaking and owns the `agent_speaking_end`.
                voice_debug("TTS drained after speaking already ended (barge-in)");
            }
            // End of the utterance file before post-utterance silence, so silence is not in it.
            if let Some(tap) = tts_out_tap.as_ref() {
                tap.lock().unwrap_or_else(|e| e.into_inner()).finish(false);
            }
            Self::maybe_spawn_deferred_lid_after_tts(weak_self, tts_synthesis_queue, inner).await;

            let silence_ms = {
                let guard = inner.lock().await;
                if !guard.running {
                    return Ok(());
                }
                resolved_post_utterance_silence_ms(&guard.config)
            };
            if silence_ms > 0 {
                Self::stream_post_utterance_silence(
                    &writer,
                    tts_buffer,
                    inner,
                    weak_self,
                    drain_generation,
                    silence_ms,
                    &mut pace,
                    tts_synthesis_queue,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Write paced 20 ms frames. Writer failures propagate; barge/stop returns [`TtsDrainWrite::Stopped`].
    #[allow(clippy::too_many_arguments)] // shared handles of the TTS worker, kept as plain params (hot path)
    async fn write_paced_tts_frames(
        writer: &PcmWriter,
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        event_bus: &SpeechEventBus,
        weak_self: &Weak<VoiceAgent>,
        drain_generation: u64,
        agent_start_emitted: &mut bool,
        played_any: &mut bool,
        pace: &mut TtsDrainPaceState,
        frames: Vec<(Bytes, u32)>,
        last_frame_real_bytes: Option<usize>,
        tts_out_tap: Option<&Arc<std::sync::Mutex<TtsOutTap>>>,
    ) -> SpeechResult<TtsDrainWrite> {
        let frame_count = frames.len();
        for (index, (frame, duration_ms)) in frames.into_iter().enumerate() {
            if weak_self.strong_count() == 0 {
                // Dropped without stop(): no owner and no listener remain, so end silently.
                voice_debug("TTS drain stopped (agent dropped)");
                return Ok(TtsDrainWrite::Stopped);
            }
            *played_any = true;
            if !*agent_start_emitted {
                let (first_audio_ms, first_chunk_ms, sentence_case_adjusted) = {
                    let mut guard = inner.lock().await;
                    let timing = guard.pending_first_audio.take();
                    let first_audio_ms = timing.map(|t| Self::elapsed_ms_u32(t.requested_at));
                    if let Some(ms) = first_audio_ms {
                        otel::record_voice_tts_first_audio_ms(
                            f64::from(ms),
                            Self::active_tts_vendor_label(&guard),
                            Self::project_id_label(&guard),
                        );
                    }
                    if let Some(reply) = guard.turn_latency.on_first_reply_audio(Instant::now()) {
                        let project_id = Self::project_id_label(&guard);
                        let stt_vendor = Self::active_stt_vendor_label(&guard);
                        let tts_vendor = Self::active_tts_vendor_label(&guard);
                        otel::record_voice_final_to_audio_ms(
                            reply.final_to_audio_ms,
                            project_id,
                            stt_vendor,
                            tts_vendor,
                        );
                        if let Some(turn_ms) = reply.turn_response_ms {
                            otel::record_voice_turn_response_ms(
                                turn_ms, project_id, stt_vendor, tts_vendor,
                            );
                        }
                    }
                    guard.agent_speaking = true;
                    guard.agent_speaking_since = Some(Instant::now());
                    guard.stt_barge_fired_this_agent_playback = false;
                    guard.barge_awaiting_stt_partial = false;
                    // Drop any user-turn pre-roll so agent echo does not reach STT on barge.
                    guard.stt_pre_roll.as_mut().map(SttPreRollBuffer::clear);
                    guard.short_burst.clear();
                    guard.hold_pending_pcm.clear();
                    (
                        first_audio_ms,
                        timing.and_then(|t| t.first_chunk_ms),
                        timing.and_then(|t| t.sentence_case_adjusted),
                    )
                };
                event_bus.emit(SpeechEvent::agent_speaking_start_with_timing(
                    first_audio_ms,
                    first_chunk_ms,
                    sentence_case_adjusted,
                ));
                voice_debug("agent_speaking_start (first outbound PCM frame)");
                *agent_start_emitted = true;
            }

            if tts_buffer.current_generation().await != drain_generation {
                voice_debug("TTS drain stopped (barge-in flush)");
                if Self::end_agent_speaking_inner(inner, false).await {
                    event_bus.emit(SpeechEvent::agent_speaking_end());
                }
                if let Some(tap) = tts_out_tap {
                    tap.lock().unwrap_or_else(|e| e.into_inner()).finish(true);
                }
                return Ok(TtsDrainWrite::Stopped);
            }
            let ledger_bytes = match last_frame_real_bytes {
                Some(real) if index + 1 == frame_count => real,
                _ => frame.len(),
            };
            if let Some(tap) = tts_out_tap {
                tap.lock().unwrap_or_else(|e| e.into_inner()).push(&frame);
            }
            writer(frame, duration_ms)?;
            Self::emit_reply_transitions(
                event_bus,
                tts_buffer.consume_played(ledger_bytes as u64).await,
            );
            if !Self::pace_tts_drain_frame_while_running(
                tts_buffer,
                inner,
                weak_self,
                drain_generation,
                duration_ms,
                pace,
            )
            .await
            {
                if weak_self.strong_count() == 0 {
                    voice_debug("TTS drain stopped during frame pacing (agent dropped)");
                    return Ok(TtsDrainWrite::Stopped);
                }
                voice_debug("TTS drain stopped during frame pacing (barge-in flush / stop)");
                if Self::end_agent_speaking_inner(inner, false).await {
                    event_bus.emit(SpeechEvent::agent_speaking_end());
                }
                if let Some(tap) = tts_out_tap {
                    tap.lock().unwrap_or_else(|e| e.into_inner()).finish(true);
                }
                return Ok(TtsDrainWrite::Stopped);
            }
        }
        Ok(TtsDrainWrite::Continued)
    }

    #[allow(clippy::too_many_arguments)] // shared handles of the TTS worker, kept as plain params (hot path)
    async fn stream_post_utterance_silence(
        writer: &PcmWriter,
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        weak_self: &Weak<VoiceAgent>,
        drain_generation: u64,
        silence_ms: u32,
        pace: &mut TtsDrainPaceState,
        tts_synthesis_queue: &Arc<Mutex<VecDeque<TtsSynthesisJob>>>,
    ) -> SpeechResult<()> {
        let frame_count = silence_ms.div_ceil(20);
        let silent = Bytes::from(vec![0_u8; STEREO_FRAME_20MS_BYTES]);
        if Self::next_tts_work_pending(tts_buffer, tts_synthesis_queue).await {
            voice_debug("post-TTS silence skipped (next utterance queued)");
            return Ok(());
        }
        voice_debug(format!(
            "post-TTS outbound silence: {silence_ms} ms ({frame_count} frames)"
        ));
        for _ in 0..frame_count {
            if weak_self.strong_count() == 0 {
                voice_debug("post-TTS silence stopped (agent dropped)");
                return Ok(());
            }
            if !inner.lock().await.running {
                voice_debug("post-TTS silence stopped (agent stop)");
                return Ok(());
            }
            if tts_buffer.current_generation().await != drain_generation {
                voice_debug("post-TTS silence stopped (barge-in flush)");
                return Ok(());
            }
            // The next utterance never waits for the tail: stop before writing another silent
            // frame. The next drain pass paces from its own anchor, one frame after the last
            // write, so there is no catch-up burst.
            if Self::next_tts_work_pending(tts_buffer, tts_synthesis_queue).await {
                voice_debug("post-TTS silence stopped (next utterance queued)");
                return Ok(());
            }
            writer(silent.clone(), 20)?;
            if !Self::pace_tts_drain_frame_while_running(
                tts_buffer,
                inner,
                weak_self,
                drain_generation,
                20,
                pace,
            )
            .await
            {
                voice_debug("post-TTS silence stopped during pacing (barge-in flush / stop)");
                return Ok(());
            }
        }
        Ok(())
    }

    /// True when another utterance is queued, being synthesized, or has audio buffered.
    /// Cheap: one buffer lock and one queue lock, none held across a write. The synthesis
    /// busy flag is not used: the finished utterance's own job still holds it for up to one
    /// poll interval after `agent_speaking_end`.
    async fn next_tts_work_pending(
        tts_buffer: &TtsBuffer,
        tts_synthesis_queue: &Arc<Mutex<VecDeque<TtsSynthesisJob>>>,
    ) -> bool {
        tts_buffer.is_speaking().await || !tts_synthesis_queue.lock().await.is_empty()
    }

    async fn end_agent_speaking_inner(
        inner: &Arc<Mutex<AgentInner>>,
        arm_stt_hold_after_playback: bool,
    ) -> bool {
        let mut guard = inner.lock().await;
        let was_speaking = guard.agent_speaking;
        Self::clear_pending_first_audio(&mut guard);
        guard.agent_speaking = false;
        guard.agent_speaking_since = None;
        guard.barge_awaiting_stt_partial = false;
        guard.stt_barge_fired_this_agent_playback = false;
        if arm_stt_hold_after_playback {
            Self::arm_stt_hold_if_idle(&mut guard);
        }
        was_speaking
    }

    /// Real-time pacing between PCM frames; returns false when flushed (barge-in) or stopped.
    async fn pace_tts_drain_frame_while_running(
        tts_buffer: &TtsBuffer,
        inner: &Arc<Mutex<AgentInner>>,
        weak_self: &Weak<VoiceAgent>,
        drain_generation: u64,
        duration_ms: u32,
        pace: &mut TtsDrainPaceState,
    ) -> bool {
        let mut remaining = duration_ms;
        while remaining > 0 {
            if weak_self.strong_count() == 0 || !inner.lock().await.running {
                return false;
            }
            if tts_buffer.current_generation().await != drain_generation {
                return false;
            }
            let slice_ms = remaining.min(20);
            if !pace.sleep_until_next_slice(slice_ms).await {
                return false;
            }
            if weak_self.strong_count() == 0
                || tts_buffer.current_generation().await != drain_generation
            {
                return false;
            }
            remaining = remaining.saturating_sub(slice_ms);
        }
        true
    }
}

/// Drift-free TTS drain clock: `deadline = anchor + total_paced_ms` after each 20 ms slice.
struct TtsDrainPaceState {
    anchor: Option<tokio::time::Instant>,
    total_paced_ms: u64,
}

impl TtsDrainPaceState {
    fn new() -> Self {
        Self {
            anchor: None,
            total_paced_ms: 0,
        }
    }

    fn deadline(&self) -> Option<tokio::time::Instant> {
        self.anchor
            .map(|anchor| anchor + std::time::Duration::from_millis(self.total_paced_ms))
    }

    async fn sleep_until_next_slice(&mut self, slice_ms: u32) -> bool {
        if self.anchor.is_none() {
            let now = tokio::time::Instant::now();
            self.anchor = Some(
                now.checked_sub(std::time::Duration::from_millis(self.total_paced_ms))
                    .unwrap_or(now),
            );
        }
        self.total_paced_ms += u64::from(slice_ms);
        let deadline = self.deadline().expect("anchor set");
        tokio::time::sleep_until(deadline).await;
        true
    }
}

const STEREO_FRAME_20MS_BYTES: usize = 3840;

enum TtsDrainWrite {
    Continued,
    Stopped,
}

/// Pad to a full 20 ms stereo frame so Opus always receives 960 samples/channel.
fn pad_stereo_frame_20ms(frame: &[u8]) -> (Bytes, u32) {
    if frame.len() == STEREO_FRAME_20MS_BYTES {
        return (Bytes::copy_from_slice(frame), 20);
    }
    let mut padded = vec![0_u8; STEREO_FRAME_20MS_BYTES];
    let copy_len = frame.len().min(STEREO_FRAME_20MS_BYTES);
    padded[..copy_len].copy_from_slice(&frame[..copy_len]);
    (Bytes::from(padded), 20)
}

/// Drain complete 20 ms frames from `carry`; leave a short remainder unpadded.
fn take_complete_stereo_frames(carry: &mut Vec<u8>) -> Vec<(Bytes, u32)> {
    let complete = (carry.len() / STEREO_FRAME_20MS_BYTES) * STEREO_FRAME_20MS_BYTES;
    if complete == 0 {
        return Vec::new();
    }
    let data: Vec<u8> = carry.drain(..complete).collect();
    data.chunks(STEREO_FRAME_20MS_BYTES)
        .map(|frame| (Bytes::copy_from_slice(frame), 20_u32))
        .collect()
}

/// Legacy helper: split and pad every partial chunk (unit tests only).
#[cfg(test)]
fn split_stereo_pcm_frames(pcm: &Bytes, _total_duration_ms: u32) -> Vec<(Bytes, u32)> {
    if pcm.is_empty() {
        return Vec::new();
    }
    let mut carry = pcm.to_vec();
    let mut frames = take_complete_stereo_frames(&mut carry);
    if !carry.is_empty() {
        frames.push(pad_stereo_frame_20ms(&carry));
    }
    frames
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
    use crate::pipeline::VendorFactory;

    struct NoopFactory;

    impl VendorFactory for NoopFactory {
        fn create_stt(&self, _config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
            Err(SpeechError::Config("noop".into()))
        }

        fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
            Err(SpeechError::Config("noop".into()))
        }
    }

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }

    #[test]
    fn split_stereo_pcm_frames_pads_tail_to_opus_20ms() {
        let pcm = Bytes::from(vec![0_u8; STEREO_FRAME_20MS_BYTES + 2560]);
        let frames = split_stereo_pcm_frames(&pcm, 45);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].0.len(), STEREO_FRAME_20MS_BYTES);
        assert_eq!(frames[0].1, 20);
        assert_eq!(frames[1].0.len(), STEREO_FRAME_20MS_BYTES);
        assert_eq!(frames[1].1, 20);
    }

    #[test]
    fn take_complete_stereo_frames_does_not_pad_partial_midstream() {
        let mut carry = vec![1_u8; 1000];
        assert!(take_complete_stereo_frames(&mut carry).is_empty());
        assert_eq!(carry.len(), 1000);

        carry.extend(std::iter::repeat_n(2_u8, STEREO_FRAME_20MS_BYTES));
        let frames = take_complete_stereo_frames(&mut carry);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0.len(), STEREO_FRAME_20MS_BYTES);
        // Remainder kept for the next progressive chunk (no silence pad).
        assert_eq!(
            carry.len(),
            1000 + STEREO_FRAME_20MS_BYTES - STEREO_FRAME_20MS_BYTES
        );
        // 1000 bytes from the first partial remain after taking one full frame from the join.
        // carry was 1000+3840; drained 3840 → 1000 left.
        assert_eq!(carry.len(), 1000);
    }

    #[test]
    fn tts_drain_pace_state_deadline_invariant_after_100_slices() {
        let mut pace = TtsDrainPaceState::new();
        pace.anchor = Some(tokio::time::Instant::now());
        pace.total_paced_ms = 100 * 20;
        let anchor = pace.anchor.expect("pace anchor");
        let deadline = pace.deadline().expect("pace deadline");
        assert_eq!(
            deadline - anchor,
            std::time::Duration::from_millis(2000),
            "deadline invariant: anchor + n×20 ms per paced slice"
        );
    }

    #[tokio::test]
    async fn attach_requires_pcm_hooks() {
        let mut registry = VendorRegistry::new();
        registry.register_stt(SttVendor::Mock, Arc::new(NoopFactory));
        registry.register_tts(TtsVendor::Mock, Arc::new(NoopFactory));

        let config = VoiceAgentConfig {
            stt: None,
            tts: None,
            ..Default::default()
        };
        let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
        let reader: PcmReader = Arc::new(|| Ok(None));
        let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
        agent.attach(reader, writer).await.unwrap();
    }
}
