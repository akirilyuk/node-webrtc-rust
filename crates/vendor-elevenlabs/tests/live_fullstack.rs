//! Live ElevenLabs full-stack STT check: the real `VoiceAgent` and its STT actor with the production
//! VAD preset (`gate_stt: true`), not the direct provider calls of `live_test.rs`. Run with
//! `--features live`.
//!
//! The counting phrase is synthesized once with the vendor's own TTS and pushed as inbound 20 ms
//! frames. One spoken phrase must produce exactly one `user_speech_final` with at least 8 of the
//! 10 number words. Needs `ELEVENLABS_API_KEY` (CI maps the repo secret `ELEVENLABS_LIVE_KEY`); without it the test skips
//! unless `ELEVENLABS_LIVE_REQUIRED=1`. The key and transcripts are never printed.
#![cfg(feature = "live")]

use std::sync::Arc;
use std::time::Duration;

use node_webrtc_rust_speech::config::{
    SpeechTransport, SttConfig, SttVendor, TtsConfig, TtsVendor,
};
use node_webrtc_rust_speech::pipeline::{TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_elevenlabs::ElevenLabsFactory;
use node_webrtc_rust_vendor_live_support::audio::{COUNT_PHRASE, COUNT_WORDS};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::fullstack::{
    assert_fullstack_contract, run_fullstack, run_fullstack_idle_then_speak,
    run_fullstack_with_config,
};
use node_webrtc_rust_vendor_live_support::runner_config::runner_staging_config;
use node_webrtc_rust_vendor_live_support::vad_gate::silence_stereo_48k;
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "elevenlabs";
/// Longer than the ~15 s after which ElevenLabs closes an idle realtime socket.
/// Silence between the two halves of the phrase in the forced-split test: above the runner VAD's 1100 ms.
const FORCED_SPLIT_GAP_MS: usize = 1500;
const IDLE_BEFORE_SPEECH_MS: usize = 20_000;

fn tts(key: &str) -> Box<dyn TtsProvider> {
    ElevenLabsFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Elevenlabs,
            model: voice_catalog::default_tts_model(ID).map(str::to_string),
            model_path: None,
            voice: None,
            api_key: Some(key.to_string()),
            endpoint: None,
            sentence_case: Default::default(),
            transport: Some(SpeechTransport::parse("auto").expect("transport")),
        })
        .unwrap_or_else(|err| panic!("tts config: {err}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elevenlabs_stt_fullstack_one_final_per_utterance() {
    let Some(mut env) = live_env(ID, &["ELEVENLABS_API_KEY"]) else {
        return;
    };
    let key = env.remove("ELEVENLABS_API_KEY").expect("key");
    let speech: Vec<u8> = tts(&key)
        .synthesize_progressive(COUNT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();

    let stt = SttConfig {
        provider: SttVendor::Elevenlabs,
        model: voice_catalog::default_stt_model(ID).map(str::to_string),
        model_path: None,
        language: None,
        api_key: Some(key),
        endpoint: None,
        transport: None,
    };
    let sample = run_fullstack(
        Arc::new(ElevenLabsFactory) as Arc<dyn VendorFactory>,
        stt,
        &speech,
        &COUNT_WORDS,
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} full-stack run failed: {err}"));
    assert_fullstack_contract(ID, &sample, 8, COUNT_WORDS.len());
}

/// The same phrase through the config the runner builds on staging (`runner_staging_config`:
/// language ID early, rnnoise, the runner VAD and barge-in values). ElevenLabs answered such a
/// session with `user_stt_not_found` / `no_partial` after the 4 s listen window (2026-10-19).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elevenlabs_stt_fullstack_runner_config_one_final_per_utterance() {
    let Some(mut env) = live_env(ID, &["ELEVENLABS_API_KEY"]) else {
        return;
    };
    let key = env.remove("ELEVENLABS_API_KEY").expect("key");
    let speech: Vec<u8> = tts(&key)
        .synthesize_progressive(COUNT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    let stt = SttConfig {
        provider: SttVendor::Elevenlabs,
        model: voice_catalog::default_stt_model(ID).map(str::to_string),
        model_path: None,
        language: Some("en".into()),
        api_key: Some(key),
        endpoint: None,
        transport: None,
    };
    let sample = run_fullstack_with_config(
        Arc::new(ElevenLabsFactory) as Arc<dyn VendorFactory>,
        runner_staging_config(stt),
        &speech,
        &COUNT_WORDS,
        Duration::from_secs(4),
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} runner-config full-stack run failed: {err}"));
    assert_fullstack_contract(&format!("{ID}_runner"), &sample, 8, COUNT_WORDS.len());
}

/// The runner config again, with the session idle for 20 s before the caller speaks. ElevenLabs
/// closes a realtime socket that gets no audio for about 15 s (close code 1000, measured
/// 2026-10-19), and the gate sends nothing between utterances. Staging runs ended with
/// `user_stt_not_found` / `no_partial` after 4 s because the audio went to the closed socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elevenlabs_stt_fullstack_runner_config_after_idle_one_final_per_utterance() {
    let Some(mut env) = live_env(ID, &["ELEVENLABS_API_KEY"]) else {
        return;
    };
    let key = env.remove("ELEVENLABS_API_KEY").expect("key");
    let speech: Vec<u8> = tts(&key)
        .synthesize_progressive(COUNT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    let stt = SttConfig {
        provider: SttVendor::Elevenlabs,
        model: voice_catalog::default_stt_model(ID).map(str::to_string),
        model_path: None,
        language: Some("en".into()),
        api_key: Some(key),
        endpoint: None,
        transport: None,
    };
    let sample = run_fullstack_idle_then_speak(
        Arc::new(ElevenLabsFactory) as Arc<dyn VendorFactory>,
        runner_staging_config(stt),
        &speech,
        &COUNT_WORDS,
        Duration::from_secs(4),
        IDLE_BEFORE_SPEECH_MS,
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} idle full-stack run failed: {err}"));
    assert_fullstack_contract(&format!("{ID}_idle"), &sample, 8, COUNT_WORDS.len());
}

/// The counting phrase with 1.5 s of silence between "five." and "six.": the runner VAD (minimum
/// silence 1100 ms) always ends the first utterance there, so the second utterance's audio is
/// pushed while the first one's commit may still be unanswered. Each utterance must get its own
/// final, and no word may be lost across the two.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elevenlabs_stt_fullstack_runner_config_forced_split_keeps_every_word() {
    let Some(mut env) = live_env(ID, &["ELEVENLABS_API_KEY"]) else {
        return;
    };
    let key = env.remove("ELEVENLABS_API_KEY").expect("key");
    let provider = tts(&key);
    let mut speech: Vec<u8> = Vec::new();
    for (n, part) in [
        "one. two. three. four. five.",
        "six. seven. eight. nine. ten.",
    ]
    .iter()
    .enumerate()
    {
        if n > 0 {
            speech.extend(silence_stereo_48k(FORCED_SPLIT_GAP_MS));
        }
        speech.extend(
            provider
                .synthesize_progressive(part, None)
                .await
                .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
                .into_iter()
                .flat_map(|chunk| chunk.pcm),
        );
    }
    let stt = SttConfig {
        provider: SttVendor::Elevenlabs,
        model: voice_catalog::default_stt_model(ID).map(str::to_string),
        model_path: None,
        language: Some("en".into()),
        api_key: Some(key),
        endpoint: None,
        transport: None,
    };
    let sample = run_fullstack_with_config(
        Arc::new(ElevenLabsFactory) as Arc<dyn VendorFactory>,
        runner_staging_config(stt),
        &speech,
        &COUNT_WORDS,
        Duration::from_secs(4),
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} forced-split full-stack run failed: {err}"));
    assert!(
        sample.speech_started >= 2,
        "the {FORCED_SPLIT_GAP_MS} ms gap must split the phrase into at least two utterances ({sample:?})"
    );
    assert_fullstack_contract(&format!("{ID}_split"), &sample, 8, COUNT_WORDS.len());
}

/// The commit of utterance 1 is sent and the audio of utterance 2 follows at once, before the
/// server answers the commit (the Scribe docs do not say which segment such audio belongs to). The
/// provider is driven directly: utterance 1 ("one" to "five") must come back as its own final
/// with none of utterance 2's words ("six" to "ten"), and the other way round. Counts only; no
/// transcript is printed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elevenlabs_stt_commit_answer_after_next_audio_keeps_each_utterance() {
    use bytes::Bytes;
    use node_webrtc_rust_speech::pcm::stereo_48k_to_mono_16k;
    use node_webrtc_rust_speech::pipeline::{SttOutput, SttTranscript};
    use node_webrtc_rust_vendor_live_support::audio::matched_words;
    use node_webrtc_rust_vendor_live_support::stt::open_stt;
    use tokio::time::{sleep, sleep_until, Instant};

    let Some(mut env) = live_env(ID, &["ELEVENLABS_API_KEY"]) else {
        return;
    };
    let key = env.remove("ELEVENLABS_API_KEY").expect("key");
    let provider = tts(&key);
    let first_words = &COUNT_WORDS[..5];
    let second_words = &COUNT_WORDS[5..];
    let mut halves: Vec<Vec<Bytes>> = Vec::new();
    for part in [
        "one. two. three. four. five.",
        "six. seven. eight. nine. ten.",
    ] {
        let stereo: Vec<u8> = provider
            .synthesize_progressive(part, None)
            .await
            .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
            .into_iter()
            .flat_map(|chunk| chunk.pcm)
            .collect();
        // 20 ms of 48 kHz stereo s16le is 3840 bytes; 16 kHz mono is 640.
        halves.push(
            stereo
                .chunks(3840)
                .map(|frame| {
                    Bytes::from(node_webrtc_rust_speech::pcm::i16_samples_to_bytes(
                        &stereo_48k_to_mono_16k(frame),
                    ))
                })
                .collect(),
        );
    }
    let mut stt = ElevenLabsFactory
        .create_stt(&SttConfig {
            provider: SttVendor::Elevenlabs,
            model: voice_catalog::default_stt_model(ID).map(str::to_string),
            model_path: None,
            language: Some("en".into()),
            api_key: Some(key),
            endpoint: None,
            transport: None,
        })
        .unwrap_or_else(|err| panic!("stt config: {err}"));
    open_stt(stt.as_mut())
        .await
        .unwrap_or_else(|err| panic!("{ID} open failed: {err}"));

    let mut finals: Vec<String> = Vec::new();
    let mut dones = 0usize;
    let mut anchor = Instant::now();
    let mut n: u32 = 0;
    for (index, half) in halves.iter().enumerate() {
        for frame in half {
            sleep_until(anchor + Duration::from_millis(20) * n).await;
            n += 1;
            stt.push_audio(frame.clone()).await.expect("push");
            while let Some(output) = stt.poll_output().await.expect("poll") {
                match output {
                    SttOutput::Transcript(SttTranscript::Final(t)) => finals.push(t),
                    SttOutput::FinalizeDone => dones += 1,
                    SttOutput::Transcript(_) => {}
                }
            }
        }
        // The commit goes out and the next utterance's audio follows with no gap.
        stt.request_finalize().await.expect("finalize");
        if index == 0 {
            anchor = Instant::now() - Duration::from_millis(20) * n;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while dones < 2 && Instant::now() < deadline {
        match stt.poll_output().await.expect("poll") {
            Some(SttOutput::Transcript(SttTranscript::Final(t))) => finals.push(t),
            Some(SttOutput::FinalizeDone) => dones += 1,
            Some(_) => {}
            None => sleep(Duration::from_millis(10)).await,
        }
    }
    stt.stop().await.expect("stop");

    let per_final: Vec<(usize, usize)> = finals
        .iter()
        .map(|t| {
            (
                matched_words(t, first_words),
                matched_words(t, second_words),
            )
        })
        .collect();
    println!("{ID}_overlap_finals {} dones {dones}", finals.len());
    println!("{ID}_overlap_first_second_matched_per_final {per_final:?}");
    assert_eq!(dones, 2, "{ID}: both commits must be answered");
    assert_eq!(
        finals.len(),
        2,
        "{ID}: one final per utterance ({per_final:?})"
    );
    assert!(
        per_final[0].0 >= 4 && per_final[0].1 == 0,
        "{ID}: final 1 must hold its own words and none of utterance 2's ({per_final:?})"
    );
    assert!(
        per_final[1].1 >= 4 && per_final[1].0 == 0,
        "{ID}: final 2 must hold its own words and none of utterance 1's ({per_final:?})"
    );
}
