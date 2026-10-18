//! Latency bench against the real API (`vendor-bench.yml`, or locally). Prints one
//! `vendor_bench <json>` line per sample; never a key or a transcript.
//!
//! Transcribe ends its stream at every finalize and each utterance opens a new one, so a second
//! utterance on the same provider is not a warm connection: the bench measures `cold` only.
//!
//! `NWR_BENCH_RUNS` sets the runs (default 5). Without AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_REGION the bench skips, unless
//! `AWS_LIVE_REQUIRED=1`.
#![cfg(feature = "live")]

use node_webrtc_rust_speech::config::{
    SpeechTransport, SttConfig, SttVendor, TtsConfig, TtsVendor,
};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_aws::AwsFactory;
use node_webrtc_rust_vendor_live_support::audio::STT_PHRASE;
use node_webrtc_rust_vendor_live_support::bench::{bench_stt, bench_tts, runs_from_env};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::vad_gate::silence_stereo_48k;
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "aws";

fn tts(transport: &str) -> Box<dyn TtsProvider> {
    AwsFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Aws,
            model: voice_catalog::default_tts_model(ID).map(str::to_string),
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
            sentence_case: Default::default(),
            transport: Some(SpeechTransport::parse(transport).expect("transport")),
        })
        .unwrap_or_else(|err| panic!("tts config: {err}"))
}

fn stt() -> Box<dyn SttProvider> {
    AwsFactory
        .create_stt(&SttConfig {
            provider: SttVendor::Aws,
            model: voice_catalog::default_stt_model(ID).map(str::to_string),
            model_path: None,
            language: None,
            api_key: None,
            endpoint: None,
            transport: None,
        })
        .unwrap_or_else(|err| panic!("stt config: {err}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bench() {
    // Gate first: no provider (and no AWS SDK credential lookup) is built without the variables.
    if live_env(
        ID,
        &["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_REGION"],
    )
    .is_none()
    {
        return;
    }
    let runs = runs_from_env(5);
    // The catalog default TTS model/voice is `None` for vendors that pick by voice; label it.
    let tts_model = voice_catalog::default_tts_model(ID).unwrap_or("default");
    let stt_model = voice_catalog::default_stt_model(ID).unwrap_or("default");

    for transport in voice_catalog::tts_transports(ID) {
        bench_tts(ID, transport, tts_model, || tts(transport), runs)
            .await
            .unwrap_or_else(|err| panic!("{ID} {transport} tts bench failed: {err}"));
    }

    // The STT phrase is synthesized once with the vendor's own default TTS.
    let speech: Vec<u8> = tts("auto")
        .synthesize_progressive(STT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    let stereo = [silence_stereo_48k(1000), speech, silence_stereo_48k(2000)].concat();
    bench_stt(ID, "http", stt_model, || stt(), &stereo, runs)
        .await
        .unwrap_or_else(|err| panic!("{ID} stt bench failed: {err}"));
}
