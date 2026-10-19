//! Latency bench against the real APIs (`vendor-bench.yml`, or locally). Prints one
//! `vendor_bench <json>` line per sample; never a key or a transcript.
//!
//! Each credential path is gated on its own variable set (see `live_test.rs`):
//!
//! - API key (`GOOGLE_API_KEY`, `GOOGLE_LIVE_REQUIRED`): TTS Neural2 over REST (cold and warm on
//!   the pooled client) and STT `latest_long` over REST (cold, then warm: the second request on
//!   the pooled client).
//! - Service account (`GOOGLE_APPLICATION_CREDENTIALS` + `GOOGLE_CLOUD_PROJECT`,
//!   `GOOGLE_SA_LIVE_REQUIRED`): TTS Chirp 3 HD over `StreamingSynthesize` and STT `chirp_3` over
//!   V2 `StreamingRecognize`. V2 is measured cold only: one stream serves one utterance (the
//!   finalize half-closes it), and a second utterance on one stream is not a documented flow.
//!
//! `NWR_BENCH_RUNS` sets the runs (default 5).
//!
//! `NWR_BENCH_TRANSPORT` (`all` by default, or a comma list such as `http,websocket`) selects the
//! transports to measure.
#![cfg(feature = "live")]

use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider};
use node_webrtc_rust_vendor_google::{GoogleStt, GoogleTts};
use node_webrtc_rust_vendor_live_support::audio::STT_PHRASE;
use node_webrtc_rust_vendor_live_support::bench::{
    bench_stt, bench_stt_warm, bench_tts, runs_from_env, transports_from_env,
};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::vad_gate::{silence_stereo_48k, STT_CLIP_TAIL_MS};
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "google";
const SA_ID: &str = "google_sa";
const CHIRP3_VOICE: &str = "en-US-Chirp3-HD-Charon";

fn tts(voice: &str, key: Option<&str>) -> Box<dyn TtsProvider> {
    Box::new(
        GoogleTts::new(&TtsConfig {
            provider: TtsVendor::Google,
            model: None,
            model_path: None,
            voice: Some(voice.to_string()),
            api_key: key.map(str::to_string),
            endpoint: None,
            sentence_case: Default::default(),
            transport: None,
        })
        .unwrap_or_else(|err| panic!("tts config: {err}")),
    )
}

fn stt(model: &str, key: Option<&str>) -> Box<dyn SttProvider> {
    Box::new(
        GoogleStt::new(&SttConfig {
            provider: SttVendor::Google,
            model: Some(model.to_string()),
            model_path: None,
            language: Some("en-US".into()),
            api_key: key.map(str::to_string),
            endpoint: None,
            transport: None,
        })
        .unwrap_or_else(|err| panic!("stt config: {err}")),
    )
}

async fn phrase(provider: &dyn TtsProvider) -> Vec<u8> {
    let speech: Vec<u8> = provider
        .synthesize_progressive(STT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    [
        silence_stereo_48k(1000),
        speech,
        silence_stereo_48k(STT_CLIP_TAIL_MS),
    ]
    .concat()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bench() {
    let runs = runs_from_env(5);

    if let Some(mut env) = live_env(ID, &["GOOGLE_API_KEY"])
        .filter(|_| transports_from_env(&["rest"]).contains(&"rest"))
    {
        let key = env.remove("GOOGLE_API_KEY").expect("key");
        let voice = voice_catalog::default_tts_voice(ID).expect("catalog default voice");
        let model = voice_catalog::default_stt_model(ID).expect("catalog default model");
        bench_tts(ID, "rest", voice, || tts(voice, Some(&key)), runs)
            .await
            .unwrap_or_else(|err| panic!("{ID} rest tts bench failed: {err}"));
        let stereo = phrase(tts(voice, Some(&key)).as_ref()).await;
        bench_stt_warm(ID, "rest", model, || stt(model, Some(&key)), &stereo, runs)
            .await
            .unwrap_or_else(|err| panic!("{ID} rest stt bench failed: {err}"));
    }

    if live_env(
        SA_ID,
        &["GOOGLE_APPLICATION_CREDENTIALS", "GOOGLE_CLOUD_PROJECT"],
    )
    .is_some()
        && transports_from_env(&["grpc"]).contains(&"grpc")
    {
        bench_tts(ID, "grpc", CHIRP3_VOICE, || tts(CHIRP3_VOICE, None), runs)
            .await
            .unwrap_or_else(|err| panic!("{ID} grpc tts bench failed: {err}"));
        let stereo = phrase(tts(CHIRP3_VOICE, None).as_ref()).await;
        bench_stt(
            ID,
            "grpc",
            "chirp_3",
            || stt("chirp_3", None),
            &stereo,
            runs,
        )
        .await
        .unwrap_or_else(|err| panic!("{ID} grpc stt bench failed: {err}"));
    }
}
