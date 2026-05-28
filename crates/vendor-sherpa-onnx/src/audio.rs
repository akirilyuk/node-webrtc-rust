use bytes::Bytes;
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE};

/// Stereo 48 kHz s16le frame size for 20 ms (Opus-compatible).
pub const STEREO_FRAME_20MS_BYTES: usize = 3840;

/// Convert mono f32 PCM at `src_rate` Hz to stereo 48 kHz s16le for WebRTC outbound tracks.
pub fn f32_mono_to_stereo_48k_s16le(samples: &[f32], src_rate: u32) -> (Bytes, u32) {
    if samples.is_empty() || src_rate == 0 {
        return (Bytes::new(), 1);
    }

    let mono_i16: Vec<i16> = samples
        .iter()
        .map(|&sample| {
            let clamped = sample.clamp(-1.0, 1.0);
            (clamped * i16::MAX as f32) as i16
        })
        .collect();

    let resampled = if src_rate == WEBRTC_PCM_SAMPLE_RATE {
        mono_i16
    } else {
        resample_linear_i16(&mono_i16, src_rate, WEBRTC_PCM_SAMPLE_RATE)
    };

    let mono_bytes: Vec<u8> = resampled
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    let stereo = mono_s16le_to_stereo(&mono_bytes);
    align_stereo_pcm_to_20ms(stereo)
}

/// Pad trailing silence so stereo PCM length is a multiple of 20 ms @ 48 kHz.
pub fn align_stereo_pcm_to_20ms(stereo: Bytes) -> (Bytes, u32) {
    if stereo.is_empty() {
        return (stereo, 1);
    }

    let remainder = stereo.len() % STEREO_FRAME_20MS_BYTES;
    let aligned = if remainder == 0 {
        stereo
    } else {
        let mut padded = stereo.to_vec();
        padded.resize(stereo.len() + (STEREO_FRAME_20MS_BYTES - remainder), 0);
        Bytes::from(padded)
    };

    let duration_ms = stereo_duration_ms(&aligned);
    (aligned, duration_ms)
}

fn stereo_duration_ms(stereo: &Bytes) -> u32 {
    duration_ms_from_mono_s16le(stereo.len() / WEBRTC_PCM_CHANNELS, WEBRTC_PCM_SAMPLE_RATE)
}

const WEBRTC_PCM_CHANNELS: usize = 2;

fn resample_linear_i16(input: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    if input.is_empty() {
        return Vec::new();
    }
    if src_rate == dst_rate {
        return input.to_vec();
    }

    let output_len =
        ((input.len() as u64 * dst_rate as u64) / src_rate as u64).max(1) as usize;
    let mut output = Vec::with_capacity(output_len);

    for out_index in 0..output_len {
        let src_pos = out_index as f64 * src_rate as f64 / dst_rate as f64;
        let left = src_pos.floor() as usize;
        let right = (left + 1).min(input.len() - 1);
        let frac = (src_pos - left as f64) as f32;
        let sample = f32::from(input[left]) * (1.0 - frac) + f32::from(input[right]) * frac;
        output.push(sample as i16);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_increases_sample_count_for_upsampling() {
        let input = vec![0_i16, i16::MAX, 0, i16::MIN];
        let output = resample_linear_i16(&input, 22_050, 48_000);
        assert!(output.len() > input.len());
    }

    #[test]
    fn f32_to_stereo_produces_non_empty_pcm() {
        let samples = vec![0.0_f32, 0.5, -0.5, 0.25];
        let (pcm, duration_ms) = f32_mono_to_stereo_48k_s16le(&samples, 22_050);
        assert!(!pcm.is_empty());
        assert!(duration_ms >= 1);
        assert_eq!(pcm.len() % 4, 0);
        assert_eq!(pcm.len() % STEREO_FRAME_20MS_BYTES, 0);
    }

    #[test]
    fn align_stereo_pcm_pads_to_20ms_boundary() {
        let partial = Bytes::from(vec![0_u8; 1000]);
        let (aligned, duration_ms) = align_stereo_pcm_to_20ms(partial);
        assert_eq!(aligned.len() % STEREO_FRAME_20MS_BYTES, 0);
        assert!(aligned.len() > 1000);
        assert!(duration_ms >= 1);
    }
}
