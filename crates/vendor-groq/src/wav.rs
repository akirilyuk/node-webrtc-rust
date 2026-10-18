//! Incremental WAV reader for the Groq TTS body.
//!
//! Groq documents `response_format` as `flac, mp3, mulaw, ogg, wav` (API reference, "Create
//! speech"): "Supported formats are `flac, mp3, mulaw, ogg, wav`." There is no raw PCM format, so
//! the body is a WAV file. This reader takes the body as it arrives, walks the RIFF chunks until
//! `data` and returns the 16-bit PCM payload bytes. The `RIFF` and `data` sizes are ignored (a
//! streamed WAV may carry a placeholder size); the payload ends where the body ends.

/// Why a WAV header was refused. The message names the cause and is shown to the caller.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WavError(pub String);

#[derive(Default)]
enum State {
    /// Waiting for `RIFF <size> WAVE` (12 bytes).
    #[default]
    Riff,
    /// Waiting for a chunk header (`id`, `size`; 8 bytes).
    ChunkHeader,
    /// Waiting for the whole `fmt ` chunk body.
    Fmt { len: usize },
    /// Skipping the body (and pad byte) of a chunk that is not `fmt ` or `data`.
    Skip { left: usize },
    /// Everything that follows is PCM payload.
    Data,
}

pub(crate) struct WavStream {
    state: State,
    head: Vec<u8>,
    rate: Option<u32>,
}

impl WavStream {
    pub(crate) fn new() -> Self {
        Self {
            state: State::Riff,
            head: Vec::new(),
            rate: None,
        }
    }

    /// Sample rate from the `fmt ` chunk, once it was read.
    pub(crate) fn rate(&self) -> Option<u32> {
        self.rate
    }

    /// True once the `data` chunk started.
    pub(crate) fn in_data(&self) -> bool {
        matches!(self.state, State::Data)
    }

    /// Feeds body bytes; returns the PCM payload bytes contained in them (possibly none).
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Result<Vec<u8>, WavError> {
        let mut pcm = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            match self.state {
                State::Data => {
                    pcm.extend_from_slice(rest);
                    rest = &[];
                }
                State::Skip { left } => {
                    let n = left.min(rest.len());
                    rest = &rest[n..];
                    self.state = if left == n {
                        State::ChunkHeader
                    } else {
                        State::Skip { left: left - n }
                    };
                }
                State::Riff => {
                    if !self.take(&mut rest, 12) {
                        break;
                    }
                    if &self.head[0..4] != b"RIFF" || &self.head[8..12] != b"WAVE" {
                        return Err(WavError("Groq TTS response is not a WAV file".into()));
                    }
                    self.head.clear();
                    self.state = State::ChunkHeader;
                }
                State::ChunkHeader => {
                    if !self.take(&mut rest, 8) {
                        break;
                    }
                    let size =
                        u32::from_le_bytes(self.head[4..8].try_into().expect("4 bytes")) as usize;
                    let id = [self.head[0], self.head[1], self.head[2], self.head[3]];
                    self.head.clear();
                    self.state = match &id {
                        b"fmt " => State::Fmt { len: size },
                        b"data" => {
                            if self.rate.is_none() {
                                return Err(WavError(
                                    "Groq TTS WAV has a data chunk before its fmt chunk".into(),
                                ));
                            }
                            State::Data
                        }
                        // Chunks are padded to an even size.
                        _ => State::Skip {
                            left: size + (size & 1),
                        },
                    };
                }
                State::Fmt { len } => {
                    if !self.take(&mut rest, len) {
                        break;
                    }
                    if len < 16 {
                        return Err(WavError(format!("Groq TTS WAV fmt chunk is {len} bytes")));
                    }
                    let tag = u16::from_le_bytes([self.head[0], self.head[1]]);
                    let channels = u16::from_le_bytes([self.head[2], self.head[3]]);
                    let rate = u32::from_le_bytes(self.head[4..8].try_into().expect("4 bytes"));
                    let bits = u16::from_le_bytes([self.head[14], self.head[15]]);
                    self.head.clear();
                    if tag != 1 || bits != 16 {
                        return Err(WavError(format!(
                            "Groq TTS WAV is not 16-bit PCM (format tag {tag}, {bits} bits)"
                        )));
                    }
                    if channels != 1 {
                        return Err(WavError(format!(
                            "Groq TTS WAV has {channels} channels; only mono is supported"
                        )));
                    }
                    if rate == 0 {
                        return Err(WavError("Groq TTS WAV has sample rate 0".into()));
                    }
                    self.rate = Some(rate);
                    // The pad byte of an odd-sized fmt chunk is skipped like any other chunk.
                    self.state = if len & 1 == 1 {
                        State::Skip { left: 1 }
                    } else {
                        State::ChunkHeader
                    };
                }
            }
        }
        Ok(pcm)
    }

    /// Moves bytes from `rest` into `head` until it holds `want` bytes. True when it does.
    fn take(&mut self, rest: &mut &[u8], want: usize) -> bool {
        let need = want - self.head.len();
        let n = need.min(rest.len());
        self.head.extend_from_slice(&rest[..n]);
        *rest = &rest[n..];
        self.head.len() == want
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, channels: u16, bits: u16, tag: u16, extra: &[u8], pcm: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(extra);
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(pcm);
        out
    }

    fn feed_in_pieces(input: &[u8], piece: usize) -> (Result<Vec<u8>, WavError>, WavStream) {
        let mut stream = WavStream::new();
        let mut pcm = Vec::new();
        for part in input.chunks(piece) {
            match stream.feed(part) {
                Ok(bytes) => pcm.extend_from_slice(&bytes),
                Err(err) => return (Err(err), stream),
            }
        }
        (Ok(pcm), stream)
    }

    #[test]
    fn payload_is_the_same_for_every_piece_size() {
        let pcm: Vec<u8> = (0..=255u8).cycle().take(1001).collect();
        let input = wav(24_000, 1, 16, 1, &[], &pcm);
        for piece in [1, 2, 3, 7, 12, 44, 45, 500, input.len()] {
            let (got, stream) = feed_in_pieces(&input, piece);
            assert_eq!(got.unwrap(), pcm, "piece size {piece}");
            assert_eq!(stream.rate(), Some(24_000));
            assert!(stream.in_data());
        }
    }

    #[test]
    fn other_chunks_before_data_are_skipped_including_the_pad_byte() {
        let mut extra = Vec::new();
        extra.extend_from_slice(b"LIST");
        extra.extend_from_slice(&3u32.to_le_bytes());
        extra.extend_from_slice(b"abc\0"); // 3 bytes + pad
        let pcm = vec![7u8; 10];
        let input = wav(48_000, 1, 16, 1, &[], &pcm);
        // The LIST chunk goes before `fmt `.
        let mut with_list = input[..12].to_vec();
        with_list.extend_from_slice(&extra);
        with_list.extend_from_slice(&input[12..]);
        for piece in [1, 5, with_list.len()] {
            let (got, _) = feed_in_pieces(&with_list, piece);
            assert_eq!(got.unwrap(), pcm, "piece size {piece}");
        }
    }

    #[test]
    fn a_header_alone_yields_no_payload_and_no_data_state() {
        let input = wav(24_000, 1, 16, 1, &[], &[]);
        let (got, stream) = feed_in_pieces(&input[..40], 40);
        assert_eq!(got.unwrap(), Vec::<u8>::new());
        assert!(!stream.in_data());
    }

    #[test]
    fn not_a_wav_file_is_named() {
        let (got, _) = feed_in_pieces(b"{\"error\":\"nope\"} and more bytes", 64);
        assert_eq!(got.unwrap_err().0, "Groq TTS response is not a WAV file");
    }

    #[test]
    fn eight_bit_and_float_and_stereo_are_rejected_with_a_named_error() {
        let (got, _) = feed_in_pieces(&wav(24_000, 1, 8, 1, &[], &[1]), 100);
        assert!(got.unwrap_err().0.contains("not 16-bit PCM"));
        let (got, _) = feed_in_pieces(&wav(24_000, 1, 32, 3, &[], &[1]), 100);
        assert!(got.unwrap_err().0.contains("format tag 3, 32 bits"));
        let (got, _) = feed_in_pieces(&wav(24_000, 2, 16, 1, &[], &[1]), 100);
        assert!(got.unwrap_err().0.contains("2 channels"));
    }
}
