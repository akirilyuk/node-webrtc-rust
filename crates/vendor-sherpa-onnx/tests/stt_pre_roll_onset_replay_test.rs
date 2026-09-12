//! Replay captured RX PCM through VoiceAgent + Zipformer to assert soft-onset pre-roll.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, VadConfig, VoiceAgentConfig};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pcm::stereo_48k_to_mono_16k;
use node_webrtc_rust_speech::{VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;
use tokio::time::{sleep, timeout};

const FRAME_MS: u32 = 20;
const MONO_FRAME_SAMPLES: usize = 16_000 * FRAME_MS as usize / 1000; // 320 @ 16 kHz

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rx-echo-onset-s5.wav")
}

fn read_wav_mono_16k(path: &PathBuf) -> Vec<i16> {
    let data = std::fs::read(path).expect("read fixture wav");
    assert!(data.len() > 44, "wav too short");
    let pcm = &data[44..];
    pcm.chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

fn mono_16k_chunk_to_stereo_48k_frame(samples: &[i16]) -> Vec<u8> {
    assert_eq!(samples.len(), MONO_FRAME_SAMPLES);
    let mut pcm = Vec::with_capacity(MONO_FRAME_SAMPLES * 3 * 2 * 2);
    for &s in samples {
        let b = s.to_le_bytes();
        for _ in 0..3 {
            pcm.extend_from_slice(&b);
            pcm.extend_from_slice(&b);
        }
    }
    pcm
}

fn sherpa_vad_config() -> VadConfig {
    let mut vad = VadConfig::default();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 200;
    vad.speech_pad_ms = 500;
    vad.min_silence_duration_ms = 1300;
    vad.gate_stt = true;
    vad.gate_stt_open_on_pending = false;
    vad.stt_gate_hold_ms = 1000;
    vad
}

fn stt_config(model_path: String) -> SttConfig {
    SttConfig {
        provider: SttVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        language: Some("en".into()),
        api_key: None,
    }
}

#[tokio::test]
#[ignore = "requires SHERPA_STT_MODEL_PATH with valid Zipformer bundle"]
async fn replay_rx_echo_onset_fixture_starts_with_echo_one() {
    let model_path = std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH");
    let fixture = fixture_path();
    assert!(fixture.is_file(), "missing fixture {}", fixture.display());

    let mono = read_wav_mono_16k(&fixture);
    assert!(mono.len() >= MONO_FRAME_SAMPLES * 10, "fixture too short");

    // Round-trip sanity: upsample chunk → stereo 48 kHz frame → agent downmix.
    let chunk = &mono[0..MONO_FRAME_SAMPLES];
    let frame = mono_16k_chunk_to_stereo_48k_frame(chunk);
    let roundtrip = stereo_48k_to_mono_16k(&frame);
    assert_eq!(
        roundtrip.len(),
        MONO_FRAME_SAMPLES,
        "downmix length must match 20 ms @ 16 kHz"
    );
    for (a, b) in roundtrip.iter().zip(chunk.iter()) {
        assert!(
            (*a - *b).abs() <= 2,
            "downmix must match upsampled mono within 2 LSB"
        );
    }

    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::LocalSherpa, Arc::new(SherpaFactory));

    let config = VoiceAgentConfig {
        stt: Some(stt_config(model_path)),
        tts: None,
        vad: sherpa_vad_config(),
        ..Default::default()
    };

    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let mut rx = agent.subscribe_events();
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let silent = vec![0_u8; 3840];
    let mut finals = Vec::new();
    let mut partials = Vec::new();

    for chunk in mono.chunks(MONO_FRAME_SAMPLES) {
        if chunk.len() < MONO_FRAME_SAMPLES {
            break;
        }
        let frame = mono_16k_chunk_to_stereo_48k_frame(chunk);
        agent
            .process_inbound_pcm(Bytes::from(frame), FRAME_MS)
            .await
            .unwrap();
        while let Ok(event) = rx.try_recv() {
            match event.kind {
                SpeechEventKind::UserSpeechPartial => {
                    partials.push(event.text.unwrap_or_default());
                }
                SpeechEventKind::UserSpeechFinal => {
                    finals.push(event.text.unwrap_or_default());
                }
                _ => {}
            }
        }
    }

    // Trailing silence for VAD SpeechEnd + gate hold + STT finalize.
    let drain_result = timeout(Duration::from_secs(45), async {
        for _ in 0..200 {
            agent
                .process_inbound_pcm(Bytes::from(silent.clone()), FRAME_MS)
                .await
                .unwrap();
            while let Ok(event) = rx.try_recv() {
                match event.kind {
                    SpeechEventKind::UserSpeechPartial => {
                        partials.push(event.text.unwrap_or_default());
                    }
                    SpeechEventKind::UserSpeechFinal => {
                        finals.push(event.text.unwrap_or_default());
                    }
                    _ => {}
                }
            }
            if finals
                .iter()
                .any(|t| t.to_lowercase().starts_with("echo one"))
            {
                return;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(
        drain_result.is_ok(),
        "timed out waiting for user_speech_final"
    );

    let transcript = finals
        .iter()
        .find(|t| t.to_lowercase().contains("echo"))
        .cloned()
        .or_else(|| {
            partials
                .iter()
                .find(|t| t.to_lowercase().contains("echo"))
                .cloned()
        })
        .expect("expected speech transcript containing echo from replay")
        .to_lowercase();
    assert!(
        transcript.starts_with("echo one"),
        "final transcript must include soft onset pre-roll; got: {transcript} (finals: {finals:?}, partials: {partials:?})"
    );
}
