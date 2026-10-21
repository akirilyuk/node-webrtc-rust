//! Incremental WAV decoder for the concatenated `speech.audio.delta` payloads.
//!
//! Mistral does not document the PCM sample format, so the request asks for `wav` and the
//! header states it: `fmt ` tag 1 with 16 bits (s16le) or tag 3 with 32 bits (f32le), mono,
//! any sample rate. The header can be split across any number of deltas. Header bytes and
//! other chunks are never emitted, only sample bytes.
//!
//! [`WavToStereo48k`] adds the 48 kHz stereo conversion with the speech crate's
//! `MonoToStereo48k` (the helper the Fish Audio vendor uses).

use bytes::Bytes;
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pcm::MonoToStereo48k;

use crate::error::vendor_error;

/// A `fmt ` chunk larger than this is not a plain PCM/float format chunk.
const MAX_FMT_CHUNK: usize = 1024;
/// `data` size values that mean "until the stream ends".
const STREAMED_SIZE: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SampleFormat {
    S16,
    F32,
}

impl SampleFormat {
    fn width(self) -> usize {
        match self {
            Self::S16 => 2,
            Self::F32 => 4,
        }
    }
}

#[derive(Debug)]
enum State {
    /// Waiting for `RIFF<size>WAVE`.
    Riff,
    /// Waiting for an 8-byte chunk header.
    ChunkHeader,
    /// Waiting for the whole `fmt ` payload (`size` bytes, plus a pad byte when odd).
    Fmt { size: usize },
    /// Skipping a chunk we do not use.
    Skip { left: u64 },
    /// Inside `data`. `left` is `None` for a streamed (0 or 0xFFFFFFFF) size.
    Data { left: Option<u64> },
    /// The declared `data` size was consumed; anything after it is ignored.
    Done,
}

pub(crate) struct WavDecoder {
    state: State,
    buf: Vec<u8>,
    format: Option<(SampleFormat, u32)>,
    /// Partial sample carried to the next push.
    carry: Vec<u8>,
}

impl WavDecoder {
    pub(crate) fn new() -> Self {
        Self {
            state: State::Riff,
            buf: Vec::new(),
            format: None,
            carry: Vec::new(),
        }
    }

    /// Sample rate from the `fmt ` chunk, once it has been parsed.
    pub(crate) fn sample_rate(&self) -> Option<u32> {
        self.format.map(|(_, rate)| rate)
    }

    /// True once the `data` chunk has started.
    pub(crate) fn header_done(&self) -> bool {
        matches!(self.state, State::Data { .. } | State::Done)
    }

    /// Feeds decoded delta bytes and returns the s16le mono samples that are complete so far.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> SpeechResult<Vec<u8>> {
        self.buf.extend_from_slice(bytes);
        let mut samples = Vec::new();
        loop {
            match self.state {
                State::Riff => {
                    if self.buf.len() < 12 {
                        break;
                    }
                    if &self.buf[0..4] != b"RIFF" || &self.buf[8..12] != b"WAVE" {
                        return Err(vendor_error(
                            "mistral tts audio is not a RIFF/WAVE stream (expected response_format wav)",
                        ));
                    }
                    self.buf.drain(..12);
                    self.state = State::ChunkHeader;
                }
                State::ChunkHeader => {
                    if self.buf.len() < 8 {
                        break;
                    }
                    let id: [u8; 4] = self.buf[0..4].try_into().expect("4 bytes");
                    let size = u32::from_le_bytes(self.buf[4..8].try_into().expect("4 bytes"));
                    self.buf.drain(..8);
                    match &id {
                        b"fmt " => {
                            let size = size as usize;
                            if !(16..=MAX_FMT_CHUNK).contains(&size) {
                                return Err(vendor_error(format!(
                                    "mistral tts WAV fmt chunk has an unsupported size ({size})"
                                )));
                            }
                            self.state = State::Fmt { size };
                        }
                        b"data" => {
                            if self.format.is_none() {
                                return Err(vendor_error(
                                    "mistral tts WAV data chunk came before the fmt chunk",
                                ));
                            }
                            let left =
                                (size != 0 && size != STREAMED_SIZE).then_some(u64::from(size));
                            self.state = State::Data { left };
                        }
                        _ => {
                            // RIFF chunks are padded to an even size.
                            let left = u64::from(size) + u64::from(size % 2);
                            self.state = State::Skip { left };
                        }
                    }
                }
                State::Fmt { size } => {
                    let total = size + size % 2;
                    if self.buf.len() < total {
                        break;
                    }
                    let fmt = parse_fmt(&self.buf[..size])?;
                    self.buf.drain(..total);
                    self.format = Some(fmt);
                    self.state = State::ChunkHeader;
                }
                State::Skip { left } => {
                    let take = (self.buf.len() as u64).min(left);
                    self.buf.drain(..take as usize);
                    if take == left {
                        self.state = State::ChunkHeader;
                    } else {
                        self.state = State::Skip { left: left - take };
                        break;
                    }
                }
                State::Data { left } => {
                    let (format, _) = self.format.expect("fmt parsed before data");
                    let available = self.buf.len() as u64;
                    let take = left.map_or(available, |left| left.min(available)) as usize;
                    let data: Vec<u8> = self.buf.drain(..take).collect();
                    self.convert(format, &data, &mut samples);
                    match left {
                        Some(left) if take as u64 == left => {
                            self.buf.clear();
                            self.state = State::Done;
                        }
                        Some(left) => {
                            self.state = State::Data {
                                left: Some(left - take as u64),
                            };
                            break;
                        }
                        None => break,
                    }
                }
                State::Done => {
                    self.buf.clear();
                    break;
                }
            }
        }
        Ok(samples)
    }

    fn convert(&mut self, format: SampleFormat, data: &[u8], out: &mut Vec<u8>) {
        self.carry.extend_from_slice(data);
        let width = format.width();
        let whole = self.carry.len() / width * width;
        for sample in self.carry[..whole].chunks_exact(width) {
            match format {
                SampleFormat::S16 => out.extend_from_slice(sample),
                SampleFormat::F32 => {
                    let value = f32::from_le_bytes(sample.try_into().expect("4 bytes"));
                    let scaled = (value.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                    out.extend_from_slice(&scaled.to_le_bytes());
                }
            }
        }
        self.carry.drain(..whole);
    }
}

fn parse_fmt(fmt: &[u8]) -> SpeechResult<(SampleFormat, u32)> {
    let tag = u16::from_le_bytes([fmt[0], fmt[1]]);
    let channels = u16::from_le_bytes([fmt[2], fmt[3]]);
    let rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
    let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
    if channels != 1 {
        return Err(vendor_error(format!(
            "mistral tts WAV has {channels} channels; only mono is supported"
        )));
    }
    let format = match (tag, bits) {
        (1, 16) => SampleFormat::S16,
        (3, 32) => SampleFormat::F32,
        _ => {
            return Err(vendor_error(format!(
                "mistral tts WAV format tag {tag} with {bits} bits is not supported (supported: 1/16 integer PCM, 3/32 float)"
            )))
        }
    };
    if rate == 0 {
        return Err(vendor_error("mistral tts WAV has a zero sample rate"));
    }
    Ok((format, rate))
}

/// WAV bytes in, 48 kHz stereo s16le out, for one synthesis.
pub(crate) struct WavToStereo48k {
    decoder: WavDecoder,
    up: Option<MonoToStereo48k>,
}

impl WavToStereo48k {
    pub(crate) fn new() -> Self {
        Self {
            decoder: WavDecoder::new(),
            up: None,
        }
    }

    pub(crate) fn header_done(&self) -> bool {
        self.decoder.header_done()
    }

    /// Returns the 48 kHz stereo bytes for the samples that are complete so far.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> SpeechResult<Bytes> {
        let mono = self.decoder.push(bytes)?;
        if self.up.is_none() {
            if let Some(rate) = self.decoder.sample_rate() {
                self.up = Some(MonoToStereo48k::new(rate)?);
            }
        }
        match (&mut self.up, mono.is_empty()) {
            (Some(up), false) => Ok(Bytes::from(up.push(&mono))),
            _ => Ok(Bytes::new()),
        }
    }

    /// Flushes the interpolator tail once, at the end of an uncancelled synthesis.
    pub(crate) fn finish(&mut self) -> Bytes {
        self.up
            .as_mut()
            .map_or_else(Bytes::new, |up| Bytes::from(up.finish()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn header(tag: u16, bits: u16, channels: u16, rate: u32, data_size: u32) -> Vec<u8> {
        let mut h = Vec::new();
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        h.extend_from_slice(b"WAVE");
        h.extend_from_slice(b"fmt ");
        h.extend_from_slice(&16u32.to_le_bytes());
        h.extend_from_slice(&tag.to_le_bytes());
        h.extend_from_slice(&channels.to_le_bytes());
        h.extend_from_slice(&rate.to_le_bytes());
        let align = channels * bits / 8;
        h.extend_from_slice(&(rate * u32::from(align)).to_le_bytes());
        h.extend_from_slice(&align.to_le_bytes());
        h.extend_from_slice(&bits.to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&data_size.to_le_bytes());
        h
    }

    #[test]
    fn odd_trailing_byte_is_carried_across_pushes() {
        let mut wav = header(1, 16, 1, 24_000, 0);
        wav.extend_from_slice(&[1, 0, 2, 0, 3, 0]);
        let mut d = WavDecoder::new();
        // Split in the middle of the second and third samples.
        let mut out = d.push(&wav[..wav.len() - 3]).unwrap();
        out.extend(d.push(&wav[wav.len() - 3..wav.len() - 1]).unwrap());
        out.extend(d.push(&wav[wav.len() - 1..]).unwrap());
        assert_eq!(out, vec![1, 0, 2, 0, 3, 0]);
        assert_eq!(d.sample_rate(), Some(24_000));
    }

    #[test]
    fn f32_carry_needs_four_bytes() {
        let mut wav = header(3, 32, 1, 24_000, STREAMED_SIZE);
        wav.extend_from_slice(&1.0f32.to_le_bytes());
        wav.extend_from_slice(&(-2.0f32).to_le_bytes());
        let mut d = WavDecoder::new();
        let mut out = Vec::new();
        for b in wav.chunks(1) {
            out.extend(d.push(b).unwrap());
        }
        assert_eq!(
            out,
            [32767i16.to_le_bytes(), (-32767i16).to_le_bytes()].concat()
        );
    }

    #[test]
    fn unsupported_format_and_channels_are_named_errors() {
        for (tag, bits, channels, needle) in [
            (1u16, 8u16, 1u16, "format tag 1 with 8 bits"),
            (3, 64, 1, "format tag 3 with 64 bits"),
            (2, 16, 1, "format tag 2"),
            (1, 16, 2, "2 channels"),
        ] {
            let err = WavDecoder::new()
                .push(&header(tag, bits, channels, 24_000, 0))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("mistral tts WAV") && err.contains(needle),
                "{err}"
            );
        }
    }

    #[test]
    fn not_a_wav_is_a_named_error() {
        let err = WavDecoder::new()
            .push(b"ID3\x04\x00\x00\x00\x00\x00\x00\x00\x00")
            .unwrap_err()
            .to_string();
        assert!(err.contains("RIFF/WAVE"), "{err}");
    }

    #[test]
    fn declared_data_size_ends_the_samples() {
        let mut wav = header(1, 16, 1, 24_000, 4);
        wav.extend_from_slice(&[1, 0, 2, 0, 9, 9, 9, 9]);
        assert_eq!(WavDecoder::new().push(&wav).unwrap(), vec![1, 0, 2, 0]);
    }
}
