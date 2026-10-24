//! Per-remote-track inbound audio stage: reader task + reorder buffer + Opus concealment.
//!
//! One [`InboundStage`] is shared by every clone of a `RemoteTrack`. A dedicated task reads
//! raw RTP (webrtc-rs `TrackRemote::read` is not cancel-safe, so it is never wrapped in a
//! timeout) and forwards packets over an unbounded channel; `read_sample` waits on the
//! channel with a deadline from the reorder buffer.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_mixer::{Frame, OpusDecoder};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use webrtc::track::track_remote::TrackRemote;

use super::conceal::conceal_lost;
use super::impair::{Impairment, ImpairmentConfig};
use super::reorder::{InboundReorderBuffer, StageOut};
use super::stats::{InboundAudioStageStats, InboundAudioStatsSnapshot};
use crate::debug_call;
use crate::error::CoreError;
use crate::media::{PcmSample, RtpPacket};

/// Channel backlog above which one warning per episode is logged.
const BACKLOG_WARN: usize = 500;
/// Backlog below which a new episode may be reported.
const BACKLOG_RESET: usize = 100;

type RtpMsg = Result<RtpPacket, String>;

/// Reads one raw RTP packet from a webrtc-rs track.
pub(crate) async fn read_raw_rtp(track: &TrackRemote, track_id: &str) -> Result<RtpPacket, String> {
    debug_call!("core::media", "read_rtp", "id={}", track_id);
    let (packet, _) = track.read_rtp().await.map_err(|e| e.to_string())?;
    debug_call!(
        "core::media",
        "read_rtp:packet",
        "id={}, ssrc={}, seq={}, ts={}, pt={}, marker={}, payload_bytes={}",
        track_id,
        packet.header.ssrc,
        packet.header.sequence_number,
        packet.header.timestamp,
        packet.header.payload_type,
        packet.header.marker,
        packet.payload.len()
    );
    Ok(RtpPacket {
        payload: packet.payload,
        sequence_number: packet.header.sequence_number,
        timestamp: packet.header.timestamp,
        payload_type: packet.header.payload_type,
    })
}

struct StageState {
    rx: Option<UnboundedReceiver<RtpMsg>>,
    buffer: InboundReorderBuffer,
    decoder: OpusDecoder,
    pending: VecDeque<StageOut>,
    ended: Option<String>,
}

pub struct InboundStage {
    track: Arc<TrackRemote>,
    track_id: String,
    stats: Arc<InboundAudioStageStats>,
    enabled: AtomicBool,
    impairment: std::sync::Mutex<Option<ImpairmentConfig>>,
    started: AtomicBool,
    backlog: Arc<AtomicUsize>,
    state: tokio::sync::Mutex<StageState>,
}

fn reorder_enabled_from_env() -> bool {
    !matches!(
        std::env::var("NWR_INBOUND_REORDER")
            .ok()
            .as_deref()
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

fn impairment_from_env() -> Option<ImpairmentConfig> {
    let raw = std::env::var("NWR_TEST_RTP_IMPAIRMENT").ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    match ImpairmentConfig::parse(&raw) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            log::warn!("NWR_TEST_RTP_IMPAIRMENT ignored: {e}");
            None
        }
    }
}

impl InboundStage {
    pub fn new(track: Arc<TrackRemote>, track_id: String) -> Self {
        let stats = Arc::new(InboundAudioStageStats::default());
        Self {
            track,
            track_id,
            enabled: AtomicBool::new(reorder_enabled_from_env()),
            impairment: std::sync::Mutex::new(impairment_from_env()),
            started: AtomicBool::new(false),
            backlog: Arc::new(AtomicUsize::new(0)),
            state: tokio::sync::Mutex::new(StageState {
                rx: None,
                buffer: InboundReorderBuffer::new(Arc::clone(&stats)),
                decoder: OpusDecoder::new().expect("Opus decoder init"),
                pending: VecDeque::new(),
                ended: None,
            }),
            stats,
        }
    }

    pub fn stats(&self) -> Arc<InboundAudioStageStats> {
        Arc::clone(&self.stats)
    }

    pub fn snapshot(&self) -> InboundAudioStatsSnapshot {
        self.stats.snapshot()
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
    }

    pub fn set_impairment(&self, cfg: Option<ImpairmentConfig>) {
        *self.impairment.lock().unwrap_or_else(|e| e.into_inner()) = cfg;
    }

    fn spawn_reader(&self, st: &mut StageState) {
        let (tx, rx) = unbounded_channel::<RtpMsg>();
        st.rx = Some(rx);
        let track = Arc::clone(&self.track);
        let track_id = self.track_id.clone();
        let backlog = Arc::clone(&self.backlog);
        let cfg = self
            .impairment
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        tokio::spawn(async move {
            let mut impairment = cfg.map(|c| {
                log::warn!("inbound RTP impairment active on track {track_id}: {c:?}");
                Impairment::new(c)
            });
            let mut warned = false;
            let send = |msg: RtpMsg, warned: &mut bool| -> bool {
                let n = backlog.fetch_add(1, Ordering::SeqCst) + 1;
                if n > BACKLOG_WARN && !*warned {
                    *warned = true;
                    log::warn!(
                        "inbound audio backlog on track {track_id}: {n} packets queued (consumer is behind)"
                    );
                } else if n < BACKLOG_RESET {
                    *warned = false;
                }
                tx.send(msg).is_ok()
            };
            loop {
                match read_raw_rtp(&track, &track_id).await {
                    Ok(packet) => {
                        let packets = match impairment.as_mut() {
                            Some(imp) => imp.apply(packet),
                            None => vec![packet],
                        };
                        for p in packets {
                            if !send(Ok(p), &mut warned) {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(imp) = impairment.as_mut() {
                            for p in imp.flush() {
                                if !send(Ok(p), &mut warned) {
                                    return;
                                }
                            }
                        }
                        let _ = send(Err(e), &mut warned);
                        return;
                    }
                }
            }
        });
    }

    /// Reads the next PCM sample through the stage (or the raw path when disabled).
    pub async fn read_sample(&self) -> Result<PcmSample, CoreError> {
        let mut guard = self.state.lock().await;
        let st = &mut *guard;

        if !self.enabled.load(Ordering::SeqCst) {
            return self.read_sample_raw(st).await;
        }
        if !self.started.swap(true, Ordering::SeqCst) {
            self.spawn_reader(st);
        }

        loop {
            if let Some(out) = st.pending.pop_front() {
                let frames = match out {
                    StageOut::Packet(packet) => self.decode_packet(&mut st.decoder, &packet),
                    StageOut::Lost { count, next } => conceal_lost(
                        &mut st.decoder,
                        count,
                        next.as_ref().map(Bytes::as_ref),
                        &self.stats,
                    ),
                };
                if frames.is_empty() {
                    continue;
                }
                return Ok(frames_to_sample(&frames));
            }

            st.pending.extend(st.buffer.poll(Instant::now()));
            if !st.pending.is_empty() {
                continue;
            }

            if let Some(reason) = st.ended.clone() {
                st.pending.extend(st.buffer.flush());
                if !st.pending.is_empty() {
                    continue;
                }
                return Err(CoreError::Track(reason));
            }

            let deadline = st
                .buffer
                .next_deadline()
                .map(tokio::time::Instant::from_std);
            let far = tokio::time::Instant::now() + Duration::from_secs(3600);
            let Some(rx) = st.rx.as_mut() else {
                st.ended = Some("track ended".to_owned());
                continue;
            };
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(Ok(p)) => {
                            self.backlog.fetch_sub(1, Ordering::SeqCst);
                            st.buffer.push(p, Instant::now());
                        }
                        Some(Err(e)) => {
                            self.backlog.fetch_sub(1, Ordering::SeqCst);
                            st.ended = Some(e);
                        }
                        None => st.ended = Some("track ended".to_owned()),
                    }
                }
                _ = tokio::time::sleep_until(deadline.unwrap_or(far)), if deadline.is_some() => {}
            }
        }
    }

    /// Old path: arrival order, decode, silence on error.
    async fn read_sample_raw(&self, st: &mut StageState) -> Result<PcmSample, CoreError> {
        loop {
            let packet = read_raw_rtp(&self.track, &self.track_id)
                .await
                .map_err(CoreError::Track)?;
            let frames = self.decode_packet_or_silence(&mut st.decoder, &packet);
            if frames.is_empty() {
                continue;
            }
            return Ok(frames_to_sample(&frames));
        }
    }

    fn decode_packet_or_silence(
        &self,
        decoder: &mut OpusDecoder,
        packet: &RtpPacket,
    ) -> Vec<Frame> {
        match decoder.try_decode_payload(&packet.payload) {
            Ok(frames) => self.log_decoded(packet, frames),
            Err(reason) => {
                self.log_decode_failed(packet, &reason.to_string());
                vec![node_webrtc_rust_mixer::silence_frame()]
            }
        }
    }

    fn decode_packet(&self, decoder: &mut OpusDecoder, packet: &RtpPacket) -> Vec<Frame> {
        match decoder.try_decode_payload(&packet.payload) {
            Ok(frames) => self.log_decoded(packet, frames),
            Err(reason) => {
                self.log_decode_failed(packet, &reason.to_string());
                InboundAudioStageStats::add(&self.stats.undecodable, 1);
                match decoder.conceal_plc(decoder.last_packet_samples()) {
                    Ok(frames) => frames,
                    Err(_) => vec![node_webrtc_rust_mixer::silence_frame()],
                }
            }
        }
    }

    fn log_decode_failed(&self, packet: &RtpPacket, reason: &str) {
        debug_call!(
            "core::media",
            "read_sample:decode_failed",
            "id={}, seq={}, payload_bytes={}, reason={}",
            self.track_id,
            packet.sequence_number,
            packet.payload.len(),
            reason
        );
    }

    fn log_decoded(&self, packet: &RtpPacket, frames: Vec<Frame>) -> Vec<Frame> {
        if crate::debug::is_debug_enabled() && !frames.is_empty() {
            let peak = frames
                .iter()
                .flat_map(|f| f.pcm.as_chunks::<2>().0.iter())
                .map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs())
                .max()
                .unwrap_or(0);
            debug_call!(
                "core::media",
                "read_sample:decoded",
                "id={}, seq={}, payload_bytes={}, frames={}, peak={}",
                self.track_id,
                packet.sequence_number,
                packet.payload.len(),
                frames.len(),
                peak
            );
        }
        frames
    }
}

fn frames_to_sample(frames: &[Frame]) -> PcmSample {
    let duration =
        Duration::from_millis(node_webrtc_rust_mixer::FRAME_MS as u64 * frames.len() as u64);
    let pcm = if frames.len() == 1 {
        frames[0].pcm.clone()
    } else {
        let mut all = Vec::with_capacity(frames.len() * node_webrtc_rust_mixer::FRAME_BYTES);
        for f in frames {
            all.extend_from_slice(&f.pcm);
        }
        Bytes::from(all)
    };
    PcmSample { pcm, duration }
}
