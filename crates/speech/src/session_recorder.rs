//! Session stereo audio recorder: media-clock capture, wall-time turn serialization, WAV/Opus export.
//!
//! Channel layout (48 kHz s16le stereo):
//!   L = outbound (client mic / TTS)
//!   R = inbound (agent ready TTS + echo)
//!
//! Port of e2e `session-audio-recorder.ts` — vendor-agnostic; no filesystem I/O.

use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate};
use thiserror::Error;

/// Session recorder sample rate (WebRTC stereo PCM).
pub const SESSION_AUDIO_SAMPLE_RATE: u32 = 48_000;
/// Session recorder channel count (stereo export).
pub const SESSION_AUDIO_CHANNELS: u16 = 2;
/// Default Opus encode bitrate for session export (256 kbps stereo).
pub const SESSION_RECORDER_DEFAULT_OPUS_BITRATE_BPS: i32 = 256_000;
/// Max gap (ms) between non-silent chunks merged into one speech run.
pub const SPEECH_RUN_MERGE_GAP_MS: u32 = 250;
/// Peak threshold for mic-pump silence detection.
pub const DEFAULT_PEAK_THRESHOLD: i16 = 200;

const DEFAULT_MAX_DURATION_MS: u32 = 90_000;
const OPUS_FRAME_SAMPLES_PER_CHANNEL: usize = 960; // 20 ms @ 48 kHz

/// Export container format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAudioFormat {
    Wav,
    Opus,
}

/// Per-channel media-clock state for contiguous PCM placement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaClockState {
    /// End of last placed audio on this channel (ms from epoch).
    pub media_end_ms: u32,
    /// Wall time of last push on this channel.
    pub last_wall_ms: u64,
    /// Duration of last placed chunk (ms).
    pub last_duration_ms: u32,
}

/// One mono PCM chunk with media + wall placement metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct PcmChunk {
    /// ms on the channel media timeline (from shared recorder epoch).
    pub offset_ms: u32,
    /// Wall ms from recorder epoch when this chunk arrived.
    pub wall_offset_ms: u32,
    /// Mono s16 samples (downmixed from stereo WebRTC PCM when captured).
    pub mono: Vec<i16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionChannel {
    Outbound,
    Inbound,
}

struct SpeechRun {
    channel: SessionChannel,
    wall_start_ms: u32,
    wall_end_ms: u32,
    chunks: Vec<PcmChunk>,
}

/// Mixed stereo PCM + metadata from {@link SessionRecorder::build}.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAudioBuild {
    pub pcm_interleaved: Vec<u8>,
    pub duration_ms: u32,
    pub outbound_frames: u32,
    pub inbound_frames: u32,
}

/// Encoded session audio from {@link SessionRecorder::finalize}.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFinalizeResult {
    pub format: SessionAudioFormat,
    pub data: Vec<u8>,
    pub duration_ms: u32,
    pub outbound_frames: u32,
    pub inbound_frames: u32,
}

#[derive(Debug, Error)]
pub enum SessionRecorderError {
    #[error("session recorder is closed")]
    Closed,
    #[error("no PCM frames captured")]
    Empty,
    #[error("Opus encode: {0}")]
    OpusEncode(String),
    #[error("Ogg mux: {0}")]
    OggMux(String),
}

pub type SessionRecorderResult<T> = Result<T, SessionRecorderError>;

type NowFn = Box<dyn Fn() -> u64 + Send + Sync>;

/// Captures outbound/inbound PCM and builds stereo WAV or Opus-in-Ogg on finalize.
pub struct SessionRecorder {
    max_duration_ms: u32,
    started_at_ms: u64,
    first_push_at_ms: Option<u64>,
    outbound: Vec<PcmChunk>,
    inbound: Vec<PcmChunk>,
    outbound_clock: Option<MediaClockState>,
    inbound_clock: Option<MediaClockState>,
    outbound_frames: u32,
    inbound_frames: u32,
    closed: bool,
    now_ms: NowFn,
}

impl SessionRecorder {
    pub fn new(max_duration_ms: Option<u32>) -> Self {
        Self::with_now(max_duration_ms, Box::new(current_wall_ms))
    }

    pub(crate) fn with_now(max_duration_ms: Option<u32>, now_ms: NowFn) -> Self {
        Self {
            max_duration_ms: max_duration_ms.unwrap_or(DEFAULT_MAX_DURATION_MS),
            started_at_ms: 0,
            first_push_at_ms: None,
            outbound: Vec::new(),
            inbound: Vec::new(),
            outbound_clock: None,
            inbound_clock: None,
            outbound_frames: 0,
            inbound_frames: 0,
            closed: false,
            now_ms,
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn push_outbound(&mut self, pcm: &[u8]) -> SessionRecorderResult<()> {
        self.push_channel(SessionChannel::Outbound, pcm);
        Ok(())
    }

    pub fn push_inbound(&mut self, pcm: &[u8]) -> SessionRecorderResult<()> {
        self.push_channel(SessionChannel::Inbound, pcm);
        Ok(())
    }

    fn push_channel(&mut self, side: SessionChannel, pcm: &[u8]) {
        if self.closed {
            return;
        }
        let now_wall_ms = (self.now_ms)();
        if !self.accepting(now_wall_ms) {
            return;
        }
        let mono = pcm_to_mono_s16(pcm);
        if mono.is_empty() {
            return;
        }

        let epoch = self.ensure_epoch(now_wall_ms);
        let duration_ms = chunk_duration_ms_from_samples(mono.len(), SESSION_AUDIO_SAMPLE_RATE);
        let wall_offset_ms = (now_wall_ms.saturating_sub(epoch)) as u32;
        let prior = match side {
            SessionChannel::Outbound => self.outbound_clock,
            SessionChannel::Inbound => self.inbound_clock,
        };
        let resolved = resolve_media_clock_offset_ms(ResolveMediaClockParams {
            wall_offset_ms,
            duration_ms,
            now_wall_ms,
            state: prior,
        });

        let chunk = PcmChunk {
            offset_ms: resolved.offset_ms,
            wall_offset_ms,
            mono,
        };
        match side {
            SessionChannel::Outbound => {
                self.outbound_clock = Some(resolved.next);
                self.outbound.push(chunk);
                self.outbound_frames += 1;
            }
            SessionChannel::Inbound => {
                self.inbound_clock = Some(resolved.next);
                self.inbound.push(chunk);
                self.inbound_frames += 1;
            }
        }
    }

    fn ensure_epoch(&mut self, now_wall_ms: u64) -> u64 {
        if self.first_push_at_ms.is_none() {
            self.first_push_at_ms = Some(now_wall_ms);
            if self.started_at_ms == 0 {
                self.started_at_ms = now_wall_ms;
            }
        }
        self.first_push_at_ms.unwrap_or(now_wall_ms)
    }

    fn accepting(&self, now_wall_ms: u64) -> bool {
        if self.closed {
            return false;
        }
        match self.first_push_at_ms {
            None => true,
            Some(epoch) => now_wall_ms.saturating_sub(epoch) < u64::from(self.max_duration_ms),
        }
    }

    pub fn build(&self) -> SessionRecorderResult<SessionAudioBuild> {
        if self.outbound_frames == 0 && self.inbound_frames == 0 {
            return Err(SessionRecorderError::Empty);
        }
        let turned = serialize_speech_turns_by_wall_time(SerializeSpeechTurnsParams {
            outbound: &self.outbound,
            inbound: &self.inbound,
            sample_rate: SESSION_AUDIO_SAMPLE_RATE,
            peak_threshold: DEFAULT_PEAK_THRESHOLD,
        });
        let mixed = mix_stereo_timeline(MixStereoTimelineParams {
            outbound: turned.outbound,
            inbound: turned.inbound,
            sample_rate: SESSION_AUDIO_SAMPLE_RATE,
            max_duration_ms: self.max_duration_ms,
        });
        Ok(SessionAudioBuild {
            pcm_interleaved: mixed.pcm,
            duration_ms: mixed.duration_ms,
            outbound_frames: self.outbound_frames,
            inbound_frames: self.inbound_frames,
        })
    }

    pub fn finalize(
        &mut self,
        format: SessionAudioFormat,
    ) -> SessionRecorderResult<SessionFinalizeResult> {
        if self.closed {
            return Err(SessionRecorderError::Closed);
        }
        self.closed = true;
        let built = self.build()?;
        let data = match format {
            SessionAudioFormat::Wav => encode_pcm16le_wav(
                &built.pcm_interleaved,
                SESSION_AUDIO_SAMPLE_RATE,
                SESSION_AUDIO_CHANNELS,
            ),
            SessionAudioFormat::Opus => encode_opus_ogg(
                &built.pcm_interleaved,
                SESSION_AUDIO_SAMPLE_RATE,
                SESSION_AUDIO_CHANNELS,
                SESSION_RECORDER_DEFAULT_OPUS_BITRATE_BPS,
            )?,
        };
        Ok(SessionFinalizeResult {
            format,
            data,
            duration_ms: built.duration_ms,
            outbound_frames: built.outbound_frames,
            inbound_frames: built.inbound_frames,
        })
    }
}

pub struct ResolveMediaClockParams {
    pub wall_offset_ms: u32,
    pub duration_ms: u32,
    pub now_wall_ms: u64,
    pub state: Option<MediaClockState>,
}

pub struct ResolveMediaClockResult {
    pub offset_ms: u32,
    pub next: MediaClockState,
}

/// Compute media-timeline offset for the next PCM chunk on one channel.
pub fn resolve_media_clock_offset_ms(params: ResolveMediaClockParams) -> ResolveMediaClockResult {
    let duration_ms = params.duration_ms;

    if params.state.is_none() {
        let offset_ms = params.wall_offset_ms;
        return ResolveMediaClockResult {
            offset_ms,
            next: MediaClockState {
                media_end_ms: offset_ms.saturating_add(duration_ms),
                last_wall_ms: params.now_wall_ms,
                last_duration_ms: duration_ms,
            },
        };
    }

    let state = params.state.unwrap_or(MediaClockState {
        media_end_ms: 0,
        last_wall_ms: 0,
        last_duration_ms: 0,
    });
    let offset_ms = state.media_end_ms;
    ResolveMediaClockResult {
        offset_ms,
        next: MediaClockState {
            media_end_ms: offset_ms.saturating_add(duration_ms),
            last_wall_ms: params.now_wall_ms,
            last_duration_ms: duration_ms,
        },
    }
}

/// True when every sample is below `peak_threshold` (mic-pump silence).
pub fn is_nearly_silent_mono(mono: &[i16], peak_threshold: i16) -> bool {
    mono.iter()
        .all(|&sample| sample.abs() < peak_threshold)
}

/// Downmix interleaved stereo s16le → mono. Mono input passthrough.
pub fn pcm_to_mono_s16(pcm: &[u8]) -> Vec<i16> {
    if pcm.len() < 2 {
        return Vec::new();
    }
    let looks_stereo = pcm.len() % 4 == 0 && pcm.len() >= 4;
    if !looks_stereo {
        return pcm
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
    }
    let frames = pcm.len() / 4;
    let mut out = Vec::with_capacity(frames);
    for frame in 0..frames {
        let base = frame * 4;
        let l = i16::from_le_bytes([pcm[base], pcm[base + 1]]);
        let r = i16::from_le_bytes([pcm[base + 2], pcm[base + 3]]);
        out.push(((i32::from(l) + i32::from(r)) / 2) as i16);
    }
    out
}

pub struct SerializeSpeechTurnsParams<'a> {
    pub outbound: &'a [PcmChunk],
    pub inbound: &'a [PcmChunk],
    pub sample_rate: u32,
    pub peak_threshold: i16,
}

pub struct SerializeSpeechTurnsResult {
    pub outbound: Vec<PcmChunk>,
    pub inbound: Vec<PcmChunk>,
}

/// Group non-silent chunks on one channel into utterance runs.
pub(crate) fn collect_speech_runs(
    chunks: &[PcmChunk],
    channel: SessionChannel,
    sample_rate: u32,
    peak_threshold: i16,
    merge_gap_ms: u32,
) -> Vec<SpeechRun> {
    let mut runs: Vec<SpeechRun> = Vec::new();
    let mut last_media_end_ms = 0u32;
    let mut last_wall_ms = 0u32;

    for chunk in chunks {
        if is_nearly_silent_mono(&chunk.mono, peak_threshold) {
            continue;
        }
        let dur = chunk_duration_ms_from_samples(chunk.mono.len(), sample_rate);
        if dur == 0 {
            continue;
        }

        let media_gap = chunk.offset_ms.saturating_sub(last_media_end_ms);
        let wall_gap = chunk.wall_offset_ms.saturating_sub(last_wall_ms);
        if let Some(run) = runs.last_mut() {
            if media_gap <= merge_gap_ms && wall_gap <= merge_gap_ms {
                run.chunks.push(chunk.clone());
                run.wall_end_ms = chunk.wall_offset_ms.saturating_add(dur);
                last_media_end_ms = chunk.offset_ms.saturating_add(dur);
                last_wall_ms = chunk.wall_offset_ms;
                continue;
            }
        }

        runs.push(SpeechRun {
            channel,
            wall_start_ms: chunk.wall_offset_ms,
            wall_end_ms: chunk.wall_offset_ms.saturating_add(dur),
            chunks: vec![chunk.clone()],
        });
        last_media_end_ms = chunk.offset_ms.saturating_add(dur);
        last_wall_ms = chunk.wall_offset_ms;
    }
    runs
}

fn speech_run_media_duration_ms(run: &SpeechRun, sample_rate: u32) -> u32 {
    let first = &run.chunks[0];
    let last = run.chunks.last().expect("speech run has chunks");
    last.offset_ms
        .saturating_add(chunk_duration_ms_from_samples(last.mono.len(), sample_rate))
        .saturating_sub(first.offset_ms)
}

/// Rebuild L/R timelines: speech runs ordered by wall start, placed sequentially.
pub fn serialize_speech_turns_by_wall_time(
    params: SerializeSpeechTurnsParams<'_>,
) -> SerializeSpeechTurnsResult {
    let mut runs = collect_speech_runs(
        params.outbound,
        SessionChannel::Outbound,
        params.sample_rate,
        params.peak_threshold,
        SPEECH_RUN_MERGE_GAP_MS,
    );
    runs.extend(collect_speech_runs(
        params.inbound,
        SessionChannel::Inbound,
        params.sample_rate,
        params.peak_threshold,
        SPEECH_RUN_MERGE_GAP_MS,
    ));
    runs.sort_by(|a, b| {
        a.wall_start_ms
            .cmp(&b.wall_start_ms)
            .then_with(|| match (a.channel, b.channel) {
                (SessionChannel::Inbound, SessionChannel::Outbound) => std::cmp::Ordering::Less,
                (SessionChannel::Outbound, SessionChannel::Inbound) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            })
    });

    let mut outbound: Vec<PcmChunk> = Vec::new();
    let mut inbound: Vec<PcmChunk> = Vec::new();
    let mut media_pos = 0u32;
    let mut prev_wall_end_ms: Option<u32> = None;

    for run in runs {
        let media_dur = speech_run_media_duration_ms(&run, params.sample_rate);
        let run_wall_end_ms = run
            .wall_end_ms
            .max(run.wall_start_ms.saturating_add(media_dur));

        if let Some(prev) = prev_wall_end_ms {
            let wall_silence_ms = run.wall_start_ms.saturating_sub(prev);
            media_pos = media_pos.saturating_add(wall_silence_ms);
        }

        let base = run.chunks[0].offset_ms;
        let mut run_end = media_pos;
        for chunk in &run.chunks {
            let placed = PcmChunk {
                offset_ms: media_pos.saturating_add(chunk.offset_ms.saturating_sub(base)),
                wall_offset_ms: chunk.wall_offset_ms,
                mono: chunk.mono.clone(),
            };
            let placed_end = placed
                .offset_ms
                .saturating_add(chunk_duration_ms_from_samples(placed.mono.len(), params.sample_rate));
            run_end = run_end.max(placed_end);
            match run.channel {
                SessionChannel::Outbound => outbound.push(placed),
                SessionChannel::Inbound => inbound.push(placed),
            }
        }
        media_pos = run_end;
        prev_wall_end_ms = Some(run_wall_end_ms);
    }

    SerializeSpeechTurnsResult { outbound, inbound }
}

pub struct MixStereoTimelineParams {
    pub outbound: Vec<PcmChunk>,
    pub inbound: Vec<PcmChunk>,
    pub sample_rate: u32,
    pub max_duration_ms: u32,
}

pub struct MixStereoTimelineResult {
    pub pcm: Vec<u8>,
    pub duration_ms: u32,
}

/// Mix timed mono chunks into stereo (L=outbound, R=inbound), media-timeline aligned.
pub fn mix_stereo_timeline(params: MixStereoTimelineParams) -> MixStereoTimelineResult {
    let mut end_ms = 0u32;
    for chunk in &params.outbound {
        end_ms = end_ms.max(
            chunk
                .offset_ms
                .saturating_add(chunk_duration_ms_from_samples(
                    chunk.mono.len(),
                    params.sample_rate,
                )),
        );
    }
    for chunk in &params.inbound {
        end_ms = end_ms.max(
            chunk
                .offset_ms
                .saturating_add(chunk_duration_ms_from_samples(
                    chunk.mono.len(),
                    params.sample_rate,
                )),
        );
    }
    let duration_ms = params.max_duration_ms.min(end_ms);
    let total_frames = ((f64::from(duration_ms) / 1000.0 * f64::from(params.sample_rate)).ceil()
        as usize)
        .max(1);
    let mut interleaved = vec![0_i16; total_frames * 2];

    let paint = |chunks: &[PcmChunk], channel: usize, buf: &mut [i16]| {
        for chunk in chunks {
            let start = ((f64::from(chunk.offset_ms) / 1000.0 * f64::from(params.sample_rate))
                .floor() as usize)
                .min(total_frames);
            for (i, &sample) in chunk.mono.iter().enumerate() {
                let frame = start.saturating_add(i);
                if frame >= total_frames {
                    break;
                }
                buf[frame * 2 + channel] = sample;
            }
        }
    };

    paint(&params.outbound, 0, &mut interleaved);
    paint(&params.inbound, 1, &mut interleaved);

    let pcm = interleaved
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();

    MixStereoTimelineResult { pcm, duration_ms }
}

/// Encode interleaved s16le PCM as a minimal RIFF/WAVE file.
pub fn encode_pcm16le_wav(pcm_interleaved: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_size = pcm_interleaved.len() as u32;
    let block_align = u32::from(channels) * 2;
    let byte_rate = sample_rate * block_align;
    let riff_size = 36 + data_size;
    let mut wav = Vec::with_capacity(44 + pcm_interleaved.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&(block_align as u16).to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm_interleaved);
    wav
}

/// Encode interleaved s16le PCM into Opus packets muxed in an Ogg container (RFC 7845).
pub fn encode_opus_ogg(
    pcm_interleaved: &[u8],
    sample_rate: u32,
    channels: u16,
    bitrate_bps: i32,
) -> SessionRecorderResult<Vec<u8>> {
    if sample_rate != SESSION_AUDIO_SAMPLE_RATE {
        return Err(SessionRecorderError::OpusEncode(format!(
            "unsupported sample rate {sample_rate}; expected {SESSION_AUDIO_SAMPLE_RATE}"
        )));
    }
    if channels != SESSION_AUDIO_CHANNELS {
        return Err(SessionRecorderError::OpusEncode(format!(
            "unsupported channel count {channels}; expected {SESSION_AUDIO_CHANNELS}"
        )));
    }

    let samples: Vec<i16> = pcm_interleaved
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    let samples_per_frame = OPUS_FRAME_SAMPLES_PER_CHANNEL * usize::from(channels);
    if samples_per_frame == 0 {
        return Err(SessionRecorderError::OpusEncode(
            "invalid frame size".into(),
        ));
    }

    let mut encoder = Encoder::new(SampleRate::Hz48000, Channels::Stereo, Application::Audio)
        .map_err(|e| SessionRecorderError::OpusEncode(e.to_string()))?;
    encoder
        .set_bitrate(Bitrate::BitsPerSecond(bitrate_bps))
        .map_err(|e| SessionRecorderError::OpusEncode(e.to_string()))?;

    let mut opus_packets: Vec<Vec<u8>> = Vec::new();
    let mut opus_buf = vec![0_u8; 4_000];
    let mut frame_start = 0usize;
    while frame_start < samples.len() {
        let frame_end = (frame_start + samples_per_frame).min(samples.len());
        let mut frame = samples[frame_start..frame_end].to_vec();
        if frame.len() < samples_per_frame {
            frame.resize(samples_per_frame, 0);
        }
        let len = encoder
            .encode(&frame, &mut opus_buf)
            .map_err(|e| SessionRecorderError::OpusEncode(e.to_string()))?;
        if len == 0 {
            return Err(SessionRecorderError::OpusEncode(
                "encoder produced empty packet".into(),
            ));
        }
        opus_packets.push(opus_buf[..len].to_vec());
        frame_start += samples_per_frame;
    }

    mux_opus_packets_to_ogg(&opus_packets, channels)
}

fn mux_opus_packets_to_ogg(
    opus_packets: &[Vec<u8>],
    channels: u16,
) -> SessionRecorderResult<Vec<u8>> {
    use ogg::{PacketWriteEndInfo, PacketWriter};

    let mut output = Vec::new();
    let mut writer = PacketWriter::new(&mut output);
    let serial = 0x4F50_5553u32; // "OPUS"

    let head = build_opus_head(channels as u8, 0, SESSION_AUDIO_SAMPLE_RATE);
    let tags = build_opus_tags("node-webrtc-rust-session-recorder");

    writer
        .write_packet(head, serial, PacketWriteEndInfo::EndPage, 0)
        .map_err(|e| SessionRecorderError::OggMux(e.to_string()))?;
    writer
        .write_packet(tags, serial, PacketWriteEndInfo::EndPage, 0)
        .map_err(|e| SessionRecorderError::OggMux(e.to_string()))?;

    let mut granule = 0u64;
    for (idx, packet) in opus_packets.iter().enumerate() {
        granule = granule.saturating_add(OPUS_FRAME_SAMPLES_PER_CHANNEL as u64);
        let end_info = if idx + 1 == opus_packets.len() {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        writer
            .write_packet(packet, serial, end_info, granule)
            .map_err(|e| SessionRecorderError::OggMux(e.to_string()))?;
    }

    Ok(output)
}

fn build_opus_head(channels: u8, preskip: u16, sample_rate: u32) -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(channels);
    head.extend_from_slice(&preskip.to_le_bytes());
    head.extend_from_slice(&sample_rate.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    head
}

fn build_opus_tags(vendor: &str) -> Vec<u8> {
    let vendor_bytes = vendor.as_bytes();
    let mut tags = Vec::with_capacity(8 + 4 + vendor_bytes.len() + 4);
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor_bytes.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor_bytes);
    tags.extend_from_slice(&0u32.to_le_bytes());
    tags
}

fn chunk_duration_ms_from_samples(sample_count: usize, sample_rate: u32) -> u32 {
    if sample_rate == 0 {
        return 0;
    }
    ((sample_count as u64 * 1000) / sample_rate as u64) as u32
}

fn current_wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn stereo_tone_frame(amplitude: i16, frames: usize) -> Vec<u8> {
        let mut out = vec![0_u8; frames * 4];
        for frame in 0..frames {
            let base = frame * 4;
            out[base..base + 2].copy_from_slice(&amplitude.to_le_bytes());
            out[base + 2..base + 4].copy_from_slice(&amplitude.to_le_bytes());
        }
        out
    }

    fn recorder_with_walls(walls: Vec<u64>, max_duration_ms: u32) -> SessionRecorder {
        let idx = Arc::new(AtomicUsize::new(0));
        let walls = Arc::new(walls);
        SessionRecorder::with_now(Some(max_duration_ms), Box::new(move || {
            let i = idx.fetch_add(1, Ordering::SeqCst);
            walls.get(i).copied().unwrap_or_else(|| {
                *walls.last().expect("walls sequence must not be empty")
            })
        }))
    }

    fn wav_pcm(wav: &[u8]) -> &[u8] {
        assert!(wav.starts_with(b"RIFF"));
        &wav[44..]
    }

    fn channel_peak(pcm: &[u8], channel: usize, frame: usize) -> i16 {
        let offset = frame * 4 + channel * 2;
        i16::from_le_bytes([pcm[offset], pcm[offset + 1]])
    }

    fn max_abs_in_range(pcm: &[u8], channel: usize, start_frame: usize, end_frame: usize) -> i16 {
        let mut peak = 0_i16;
        for frame in start_frame..end_frame {
            peak = peak.max(channel_peak(pcm, channel, frame).abs());
        }
        peak
    }

    fn longest_zero_run_inside_turn(
        pcm: &[u8],
        channel: usize,
        start_frame: usize,
        end_frame: usize,
        threshold: i16,
    ) -> usize {
        let mut longest = 0usize;
        let mut current = 0usize;
        for frame in start_frame..end_frame {
            if channel_peak(pcm, channel, frame).abs() < threshold {
                current += 1;
                longest = longest.max(current);
            } else {
                current = 0;
            }
        }
        longest
    }

    #[test]
    fn lr_channel_layout_places_energy_on_expected_channels() {
        let mut recorder = recorder_with_walls(vec![100, 200], 10_000);
        recorder.push_outbound(&stereo_tone_frame(5000, 480)).unwrap();
        recorder.push_inbound(&stereo_tone_frame(9000, 480)).unwrap();
        let built = recorder.build().unwrap();
        let pcm = &built.pcm_interleaved;
        // Outbound (L) placed first at media 0; inbound (R) after wall gap.
        assert!(max_abs_in_range(pcm, 0, 0, 480) >= 4000);
        assert!(max_abs_in_range(pcm, 1, 0, 480) < 500);
        let inbound_start = ((100.0 / 1000.0) * SESSION_AUDIO_SAMPLE_RATE as f64) as usize;
        assert!(max_abs_in_range(pcm, 1, inbound_start, inbound_start + 480) >= 8000);
        assert!(max_abs_in_range(pcm, 0, inbound_start, inbound_start + 480) < 500);
    }

    #[test]
    fn conversation_order_inbound_outbound_inbound_on_timeline() {
        let sample_rate = 1000u32;
        let inbound = vec![
            PcmChunk {
                offset_ms: 0,
                wall_offset_ms: 0,
                mono: vec![9000; 50],
            },
            PcmChunk {
                offset_ms: 50,
                wall_offset_ms: 800,
                mono: vec![8000; 40],
            },
        ];
        let outbound = vec![PcmChunk {
            offset_ms: 0,
            wall_offset_ms: 100,
            mono: vec![5000; 60],
        }];
        let turned = serialize_speech_turns_by_wall_time(SerializeSpeechTurnsParams {
            outbound: &outbound,
            inbound: &inbound,
            sample_rate,
            peak_threshold: DEFAULT_PEAK_THRESHOLD,
        });
        assert_eq!(turned.inbound[0].offset_ms, 0);
        assert_eq!(turned.inbound[0].mono[0], 9000);
        assert_eq!(turned.outbound[0].offset_ms, 100);
        assert_eq!(turned.outbound[0].mono[0], 5000);
        assert_eq!(turned.inbound[1].offset_ms, 800);
        assert_eq!(turned.inbound[1].mono[0], 8000);
    }

    #[test]
    fn no_mid_utterance_clipping_inside_turn() {
        let sample_rate = 1000u32;
        let inbound: Vec<PcmChunk> = [0u32, 20, 40]
            .into_iter()
            .map(|offset_ms| PcmChunk {
                offset_ms,
                wall_offset_ms: 10 + offset_ms,
                mono: vec![9000; 20],
            })
            .collect();
        let turned = serialize_speech_turns_by_wall_time(SerializeSpeechTurnsParams {
            outbound: &[],
            inbound: &inbound,
            sample_rate,
            peak_threshold: DEFAULT_PEAK_THRESHOLD,
        });
        let mixed = mix_stereo_timeline(MixStereoTimelineParams {
            outbound: turned.outbound,
            inbound: turned.inbound,
            sample_rate,
            max_duration_ms: 10_000,
        });
        let pcm = mixed.pcm;
        let zero_run = longest_zero_run_inside_turn(&pcm, 1, 0, 60, 100);
        assert!(
            zero_run < 10,
            "unexpected mid-utterance silence run={zero_run}"
        );
    }

    #[test]
    fn burst_frames_append_contiguously_without_overwrite() {
        let mut recorder = recorder_with_walls(vec![1000, 1000], 5_000);
        recorder
            .push_outbound(&stereo_tone_frame(1000, 480))
            .unwrap();
        recorder
            .push_outbound(&stereo_tone_frame(8000, 480))
            .unwrap();
        let built = recorder.build().unwrap();
        let wav = encode_pcm16le_wav(
            &built.pcm_interleaved,
            SESSION_AUDIO_SAMPLE_RATE,
            SESSION_AUDIO_CHANNELS,
        );
        let pcm = wav_pcm(&wav);
        assert_eq!(channel_peak(pcm, 0, 0), 1000);
        assert_eq!(channel_peak(pcm, 0, 480), 8000);
        assert!(built.duration_ms >= 19);
    }

    #[test]
    fn resolve_media_clock_offset_ms_always_appends_after_first_chunk() {
        let first = resolve_media_clock_offset_ms(ResolveMediaClockParams {
            wall_offset_ms: 1000,
            duration_ms: 20,
            now_wall_ms: 10_000,
            state: None,
        });
        assert_eq!(first.offset_ms, 1000);
        assert_eq!(first.next.media_end_ms, 1020);

        let delayed = resolve_media_clock_offset_ms(ResolveMediaClockParams {
            wall_offset_ms: 1500,
            duration_ms: 20,
            now_wall_ms: 10_500,
            state: Some(first.next),
        });
        assert_eq!(delayed.offset_ms, 1020);
        assert_eq!(delayed.next.media_end_ms, 1040);
    }

    #[test]
    fn finalize_opus_produces_ogg_with_default_bitrate_constant() {
        assert_eq!(
            SESSION_RECORDER_DEFAULT_OPUS_BITRATE_BPS,
            256_000,
            "default Opus bitrate must be 256000 bps"
        );
        let mut recorder = recorder_with_walls(vec![0], 5_000);
        recorder
            .push_outbound(&stereo_tone_frame(4000, 480))
            .unwrap();
        let result = recorder
            .finalize(SessionAudioFormat::Opus)
            .expect("opus finalize");
        assert_eq!(result.format, SessionAudioFormat::Opus);
        assert!(result.data.starts_with(b"OggS"));
        assert!(result.data.windows(8).any(|w| w == b"OpusHead"));
    }

    #[test]
    fn pcm_to_mono_s16_averages_stereo_channels() {
        let mut pcm = vec![0_u8; 8];
        pcm[0..2].copy_from_slice(&1000_i16.to_le_bytes());
        pcm[2..4].copy_from_slice(&3000_i16.to_le_bytes());
        pcm[4..6].copy_from_slice(&(-1000_i16).to_le_bytes());
        pcm[6..8].copy_from_slice(&1000_i16.to_le_bytes());
        let mono = pcm_to_mono_s16(&pcm);
        assert_eq!(mono.len(), 2);
        assert_eq!(mono[0], 2000);
        assert_eq!(mono[1], 0);
    }
}
