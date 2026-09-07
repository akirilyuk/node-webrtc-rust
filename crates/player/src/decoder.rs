//! Symphonia-based decode pipeline producing stereo 48 kHz PCM bytes.

use std::io::{self, Cursor, Seek, SeekFrom};

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::{Hint, ProbeResult};

use crate::error::PlayerError;
use crate::resample::f32_interleaved_to_stereo_48k;

pub struct DecodeOutput {
    pub pcm: bytes::Bytes,
    pub duration_ms: Option<u64>,
}

pub struct DecoderSession {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    duration_ms: Option<u64>,
}

impl DecoderSession {
    pub fn open<M: MediaSource + 'static>(
        mut source: M,
        hint: Option<Hint>,
    ) -> Result<Self, PlayerError> {
        let _ = source.seek(SeekFrom::Start(0));
        let mss = MediaSourceStream::new(Box::new(source), Default::default());
        let hint = hint.unwrap_or_default();
        let probed: ProbeResult = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|e| PlayerError::DecodeFailed(format!("probe failed: {e}")))?;

        let mut format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| PlayerError::DecodeFailed("no audio track".into()))?;

        let track_id = track.id;
        let duration_ms = track
            .codec_params
            .n_frames
            .and_then(|frames| {
                track
                    .codec_params
                    .sample_rate
                    .map(|rate| (frames * 1000) / rate as u64)
            });

        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| PlayerError::DecodeFailed(format!("codec open failed: {e}")))?;

        Ok(Self {
            format,
            decoder,
            track_id,
            duration_ms,
        })
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    /// Decode until blocked waiting for more input or EOF.
    pub fn decode_available(&mut self) -> Result<DecodeOutput, PlayerError> {
        let mut pcm_out = Vec::new();
        let mut packets = 0u32;

        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(err))
                    if err.kind() == io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(SymphoniaError::ResetRequired) => {
                    return Err(PlayerError::DecodeFailed("decoder reset required".into()));
                }
                Err(err) => {
                    if pcm_out.is_empty() {
                        return Err(PlayerError::DecodeFailed(format!(
                            "read packet after {packets} packets: {err}"
                        )));
                    }
                    break;
                }
            };

            packets += 1;

            if packet.track_id() != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(audio_buf) => {
                    pcm_out.extend_from_slice(&audio_buffer_to_stereo_48k(&audio_buf));
                }
                Err(SymphoniaError::DecodeError(err)) => {
                    return Err(PlayerError::DecodeFailed(format!("decode: {err}")));
                }
                Err(SymphoniaError::IoError(err))
                    if err.kind() == io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(err) => {
                    return Err(PlayerError::DecodeFailed(format!("decode: {err}")));
                }
            }
        }

        Ok(DecodeOutput {
            pcm: bytes::Bytes::from(pcm_out),
            duration_ms: self.duration_ms,
        })
    }
}

fn audio_buffer_to_stereo_48k(buf: &AudioBufferRef<'_>) -> Vec<u8> {
    match buf {
        AudioBufferRef::F32(b) => {
            let spec = b.spec();
            let channels = spec.channels.count();
            let rate = spec.rate;
            let frames = b.frames();
            let mut interleaved = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    interleaved.push(b.chan(ch)[frame]);
                }
            }
            return f32_interleaved_to_stereo_48k(&interleaved, rate, channels).to_vec();
        }
        AudioBufferRef::S16(b) => {
            let spec = b.spec();
            let channels = spec.channels.count();
            let rate = spec.rate;
            let frames = b.frames();
            let mut interleaved = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    interleaved.push(b.chan(ch)[frame] as f32 / i16::MAX as f32);
                }
            }
            return f32_interleaved_to_stereo_48k(&interleaved, rate, channels).to_vec();
        }
        _ => {
            let f32_buf = buf.make_equivalent::<f32>();
            let spec = f32_buf.spec();
            let channels = spec.channels.count();
            let rate = spec.rate;
            let frames = f32_buf.frames();
            let mut interleaved = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    interleaved.push(f32_buf.chan(ch)[frame]);
                }
            }
            f32_interleaved_to_stereo_48k(&interleaved, rate, channels).to_vec()
        }
    }
}

pub fn hint_from_path(path: &str) -> Hint {
    let mut hint = Hint::new();
    if let Some(ext) = path.rsplit('.').next() {
        hint.with_extension(ext);
    }
    hint
}

pub fn hint_from_bytes(data: &[u8]) -> Hint {
    let mut hint = Hint::new();
    if data.starts_with(b"ID3") || looks_like_mp3_frame(data) {
        hint.with_extension("mp3");
    } else if data.starts_with(b"OggS") {
        hint.with_extension("ogg");
    } else if data.starts_with(b"fLaC") {
        hint.with_extension("flac");
    } else if data.len() >= 8 && data[4..8] == *b"ftyp" {
        hint.with_extension("m4a");
    } else if data.len() >= 2 && (data[0] == 0xFF && (data[1] & 0xF0) == 0xF0) {
        hint.with_extension("aac");
    } else if data.starts_with(b"RIFF") {
        hint.with_extension("wav");
    }
    hint
}

fn looks_like_mp3_frame(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xFF && (data[1] & 0xE0) == 0xE0
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use std::io::Cursor;

    #[test]
    fn mp3_decode_outputs_pcm() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.mp3");
        let data = std::fs::read(path).expect("read mp3");
        let hint = hint_from_bytes(&data);
        let mut session =
            DecoderSession::open(Cursor::new(data), Some(hint)).expect("open");
        let out = session.decode_available().expect("decode");
        assert!(!out.pcm.is_empty(), "pcm bytes {}", out.pcm.len());
    }

    #[test]
    fn wav_decode_outputs_pcm() {
        let sample_rate = 48_000u32;
        let num_samples = sample_rate / 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + num_samples * 4).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(num_samples * 4).to_le_bytes());
        wav.extend(vec![0u8; num_samples as usize * 4]);
        let hint = hint_from_bytes(&wav);
        let mut session =
            DecoderSession::open(Cursor::new(wav), Some(hint)).expect("open wav");
        let out = session.decode_available().expect("decode wav");
        assert!(!out.pcm.is_empty());
    }
}
