//! ISO BMFF (`ftyp`/`moov`/`mdat`) probing for progressive AAC/MP4 playback.

/// Outcome of scanning available bytes for MP4/M4A layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp4Layout {
    /// Not enough data or not an ISO BMFF file.
    Unknown,
    /// `moov` appears before `mdat` — decoder can start while `mdat` streams.
    FastStart,
    /// `mdat` started before any `moov` — wait for index at end of file.
    MoovAtEnd,
    /// `moov` box is fully present in the buffer.
    MoovAvailable,
}

/// Scan buffered bytes for top-level ISO boxes and classify layout.
///
/// Walks box headers from offset 0. Stops when a box extends past `len` (incomplete).
pub fn probe_mp4_layout(data: &[u8]) -> Mp4Layout {
    if data.len() < 8 {
        return Mp4Layout::Unknown;
    }

    let mut offset = 0usize;
    let mut saw_ftyp = false;
    let mut saw_moov = false;
    let mut saw_mdat = false;
    let mut moov_complete = false;

    while offset + 8 <= data.len() {
        let box_size = read_u32_be(data, offset) as usize;
        let box_type = read_box_type(data, offset + 4);
        if box_size < 8 {
            break;
        }

        let box_end = offset + box_size;
        if box_end > data.len() {
            // Incomplete box — if moov header fits, we cannot know yet.
            if box_type == *b"moov" {
                return if saw_mdat && !saw_moov {
                    Mp4Layout::MoovAtEnd
                } else {
                    Mp4Layout::Unknown
                };
            }
            break;
        }

        match &box_type {
            b"ftyp" => saw_ftyp = true,
            b"moov" => {
                saw_moov = true;
                moov_complete = true;
            }
            b"mdat" => saw_mdat = true,
            _ => {}
        }

        if saw_mdat && !saw_moov {
            return Mp4Layout::MoovAtEnd;
        }

        offset = box_end;
    }

    if moov_complete {
        return Mp4Layout::MoovAvailable;
    }
    if saw_ftyp && saw_moov {
        return Mp4Layout::FastStart;
    }
    if saw_ftyp {
        return Mp4Layout::Unknown;
    }
    Mp4Layout::Unknown
}

/// Returns true when symphonia may open an MP4/M4A source from the current buffer.
pub fn mp4_ready_for_decode(data: &[u8], eof: bool) -> bool {
    match probe_mp4_layout(data) {
        Mp4Layout::FastStart | Mp4Layout::MoovAvailable => true,
        Mp4Layout::MoovAtEnd => eof && probe_mp4_layout(data) == Mp4Layout::MoovAvailable,
        Mp4Layout::Unknown => {
            if eof {
                probe_mp4_layout(data) == Mp4Layout::MoovAvailable
            } else {
                false
            }
        }
    }
}

fn read_u32_be(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_box_type(data: &[u8], offset: usize) -> [u8; 4] {
    [data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_on_short_buffer() {
        assert_eq!(probe_mp4_layout(&[0, 0, 0, 0]), Mp4Layout::Unknown);
    }
}
