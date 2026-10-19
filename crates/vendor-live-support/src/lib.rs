//! Shared support for the live vendor tests (`tests/live_test.rs`) and the latency bench
//! (`tests/live_bench.rs`) of the cloud speech vendors. Dev-dependency only, never published.
//!
//! Nothing here prints a key or a transcript.

pub mod audio;
pub mod bench;
pub mod env;
pub mod fullstack;
pub mod http_mock;
pub mod stt;
pub mod tts;
pub mod vad_gate;

/// 16 kHz mono s16le of [`audio::STT_PHRASE`], rendered by local Sherpa `vits-piper-en_US-amy-low`
/// (see the crate README for the generator command).
const FIXTURE_16K_MONO: &[u8] = include_bytes!("../fixtures/quick-brown-fox-16k-mono.pcm");

/// The STT fixture as the voice agent receives audio: 48 kHz stereo s16le, 1 s of silence, the
/// phrase, then 2 s of silence. The tail is longer than the VAD's 1300 ms hangover, otherwise
/// the speech segment never ends.
pub fn stt_fixture_stereo_48k() -> Vec<u8> {
    [
        vad_gate::silence_stereo_48k(1000),
        vad_gate::mono_16k_to_stereo_48k(FIXTURE_16K_MONO),
        vad_gate::silence_stereo_48k(2000),
    ]
    .concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_is_the_phrase_with_padding() {
        let stereo = stt_fixture_stereo_48k();
        let ms = stereo.len() / vad_gate::BYTES_PER_MS;
        // The phrase is roughly 2 to 4 s long.
        assert!((5000..=7500).contains(&ms), "fixture clip is {ms} ms");
        let gated = vad_gate::vad_gated_frames(&stereo);
        assert_eq!((gated.starts, gated.ends), (1, 1));
        assert!(gated.pushed_ms < gated.total_ms - 1000);
        let rms = audio::rms(FIXTURE_16K_MONO);
        assert!(rms > 200.0, "fixture rms {rms}");
    }
}
