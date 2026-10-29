//! Real TitaNet embeddings: same speaker scores above the gate threshold, another speaker below.
//!
//! Gated on `SHERPA_SPEAKER_MODEL` (the `.onnx`) and `SHERPA_SPEAKER_WAV_DIR` (sherpa's
//! `fangjun-*` / `leijun-*` test wavs); skips with a printed reason when unset.

use std::path::Path;

use bytes::Bytes;
use node_webrtc_rust_speech::config::SpeakerGateConfig;
use node_webrtc_rust_speech::pipeline::SpeakerEmbedder;
use node_webrtc_rust_vendor_sherpa_onnx::SherpaSpeakerEmbedder;

/// Minimal RIFF PCM16 reader: mono 16 kHz s16le bytes.
fn read_wav_16k_mono(path: &Path) -> Vec<u8> {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(&data[0..4], b"RIFF");
    let mut pos = 12;
    let (mut channels, mut rate) = (1_u16, 16_000_u32);
    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let len = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if id == b"fmt " {
            channels = u16::from_le_bytes(data[body + 2..body + 4].try_into().unwrap());
            rate = u32::from_le_bytes(data[body + 4..body + 8].try_into().unwrap());
        } else if id == b"data" {
            assert_eq!((channels, rate), (1, 16_000), "{}", path.display());
            return data[body..(body + len).min(data.len())].to_vec();
        }
        pos = body + len + (len & 1);
    }
    panic!("no data chunk in {}", path.display());
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}

#[tokio::test]
async fn titanet_separates_speakers_and_short_clip_is_safe() {
    let (Ok(model), Ok(dir)) = (
        std::env::var("SHERPA_SPEAKER_MODEL"),
        std::env::var("SHERPA_SPEAKER_WAV_DIR"),
    ) else {
        eprintln!("skip: set SHERPA_SPEAKER_MODEL and SHERPA_SPEAKER_WAV_DIR");
        return;
    };
    let cfg = SpeakerGateConfig {
        enabled: true,
        model_path: Some(model),
        ..SpeakerGateConfig::default()
    };
    let threshold = cfg.threshold;
    let embedder = SherpaSpeakerEmbedder::new(&cfg).expect("load speaker model");
    let embed = |name: &str| {
        let pcm = read_wav_16k_mono(&Path::new(&dir).join(name));
        let embedder = &embedder;
        async move {
            embedder
                .embed(Bytes::from(pcm), 16_000)
                .await
                .expect("embed")
                .expect("clip long enough")
        }
    };
    let enrolled = embed("fangjun-sr-1.wav").await;
    let same = embed("fangjun-test-sr-1.wav").await;
    let other = embed("leijun-test-sr-1.wav").await;
    let same_score = cosine(&enrolled, &same);
    let other_score = cosine(&enrolled, &other);
    eprintln!("same={same_score:.3} other={other_score:.3} threshold={threshold}");
    assert!(
        same_score >= threshold,
        "same speaker {same_score} < {threshold}"
    );
    assert!(
        other_score < threshold,
        "other speaker {other_score} >= {threshold}"
    );

    // A 0.2 s clip: a short result or an embedding, never a panic or an error.
    let clip = read_wav_16k_mono(&Path::new(&dir).join("fangjun-sr-1.wav"));
    let short = Bytes::from(clip[..6400].to_vec());
    let r = embedder.embed(short, 16_000).await;
    assert!(r.is_ok(), "short clip must not error: {r:?}");
}
