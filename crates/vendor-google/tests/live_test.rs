//! Live Google speech checks against the real APIs. Run with `--features live`.
//!
//! Google has two credential paths, each with its own variable set and `*_LIVE_REQUIRED` flag:
//!
//! - API key (`GOOGLE_API_KEY`, flag `GOOGLE_LIVE_REQUIRED`): the REST paths, V1
//!   `speech:recognize` (`latest_long`) and Neural2 `text:synthesize`. The key is passed as
//!   `config.api_key`.
//! - Service account (`GOOGLE_APPLICATION_CREDENTIALS` and `GOOGLE_CLOUD_PROJECT`, flag
//!   `GOOGLE_SA_LIVE_REQUIRED`): the gRPC paths, V2 `StreamingRecognize` (`chirp_3`) and Chirp 3 HD
//!   `StreamingSynthesize`. These tests pass no API key; a gRPC path cannot use one, and each
//!   asserts `last_transport() == "grpc"`.
//!
//! CI maps the repo secrets `GOOGLE_LIVE_API_KEY`, `GOOGLE_LIVE_PROJECT_ID` and
//! `GOOGLE_LIVE_SERVICE_ACCOUNT_JSON`. Without the variables a test prints a note and returns,
//! unless its flag is `1`, which turns a missing variable into a panic naming it. Keys, the
//! service account file and transcripts are never printed.
//!
//! Run with `--test-threads=1`: the tests print timing lines (`google_*`) that CI copies into the
//! step summary.
#![cfg(feature = "live")]

use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider};
use node_webrtc_rust_vendor_google::{GoogleStt, GoogleTts};
use node_webrtc_rust_vendor_live_support::audio::{STT_PHRASE, STT_WORDS};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::stt::{assert_stt_contract, measure_stt};
use node_webrtc_rust_vendor_live_support::tts::{assert_tts_contract, measure_tts};
use node_webrtc_rust_vendor_live_support::vad_gate::silence_stereo_48k;
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "google";
/// Variable set and flag of the service account tests (`GOOGLE_SA_LIVE_REQUIRED`).
const SA_ID: &str = "google_sa";
const SA_VARS: [&str; 2] = ["GOOGLE_APPLICATION_CREDENTIALS", "GOOGLE_CLOUD_PROJECT"];
const TEXT: &str = "Hello from the live check. This reply should stream as it is generated.";
/// The voice of the streaming guide's example; the catalog lists no Chirp 3 HD voice.
const CHIRP3_VOICE: &str = "en-US-Chirp3-HD-Charon";

fn api_key() -> Option<String> {
    live_env(ID, &["GOOGLE_API_KEY"]).map(|mut env| env.remove("GOOGLE_API_KEY").expect("key"))
}

fn service_account() -> Option<()> {
    live_env(SA_ID, &SA_VARS).map(|_| ())
}

fn tts_config(voice: &str, api_key: Option<String>) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::Google,
        model: None,
        model_path: None,
        voice: Some(voice.to_string()),
        api_key,
        endpoint: None,
        sentence_case: Default::default(),
        transport: None,
    }
}

fn stt_config(model: &str, api_key: Option<String>) -> SttConfig {
    SttConfig {
        provider: SttVendor::Google,
        model: Some(model.to_string()),
        model_path: None,
        language: Some("en-US".into()),
        api_key,
        endpoint: None,
        transport: None,
    }
}

fn rest_voice() -> &'static str {
    voice_catalog::default_tts_voice(ID).expect("catalog default Google voice")
}

fn rest_model() -> &'static str {
    voice_catalog::default_stt_model(ID).expect("catalog default Google STT model")
}

async fn phrase_audio(provider: &dyn TtsProvider) -> Vec<u8> {
    let speech: Vec<u8> = provider
        .synthesize_progressive(STT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    // 1 s of silence before and 2 s after; the tail is longer than the VAD's 1300 ms hangover,
    // otherwise the speech segment never ends.
    [silence_stereo_48k(1000), speech, silence_stereo_48k(2000)].concat()
}

/// Neural2 over REST with an API key: the streaming contract (the JSON reply is cut into sink
/// chunks) and the transport really used.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn google_tts_live_rest_neural2() {
    let Some(key) = api_key() else { return };
    let provider = GoogleTts::new(&tts_config(rest_voice(), Some(key))).expect("tts config");
    let sample = measure_tts(&provider, TEXT)
        .await
        .unwrap_or_else(|err| panic!("{ID} rest synthesis failed: {err}"));
    assert_tts_contract(&format!("{ID}_rest"), &sample);
    let used = provider.last_transport().expect("last_transport set");
    println!("{ID}_transport transport=rest used={used}");
    assert!(used == "h2" || used == "http/1.1", "rest used {used}");
}

/// V1 `speech:recognize` (`latest_long`) with an API key: audio from the Neural2 voice, a final
/// only (`emits_partials() == false`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn google_stt_live_rest_latest_long_vad_gated_roundtrip() {
    let Some(key) = api_key() else { return };
    let tts = GoogleTts::new(&tts_config(rest_voice(), Some(key.clone()))).expect("tts config");
    let stereo = phrase_audio(&tts).await;

    let mut provider = GoogleStt::new(&stt_config(rest_model(), Some(key))).expect("stt config");
    let sample = measure_stt(&mut provider, &stereo, &STT_WORDS)
        .await
        .unwrap_or_else(|err| panic!("{ID} rest stt failed: {err}"));
    let emits_partials = provider.emits_partials();
    let used = provider.last_transport().expect("last_transport set");
    let _ = provider.stop().await;
    assert!(
        !emits_partials,
        "{ID}: REST latest_long must be finals-only"
    );
    assert!(used == "h2" || used == "http/1.1", "rest used {used}");
    assert_stt_contract(&format!("{ID}_rest"), emits_partials, &sample);
}

/// Chirp 3 HD over `StreamingSynthesize` with the service account (no API key): the streaming
/// contract, 48 kHz stereo, and the grpc transport.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn google_tts_live_streaming_chirp3_hd() {
    let Some(()) = service_account() else { return };
    let provider = GoogleTts::new(&tts_config(CHIRP3_VOICE, None)).expect("tts config");
    let sample = measure_tts(&provider, TEXT)
        .await
        .unwrap_or_else(|err| panic!("{ID} streaming synthesis failed: {err}"));
    assert_tts_contract(&format!("{ID}_grpc"), &sample);
    assert_eq!(provider.last_transport().as_deref(), Some("grpc"));
}

/// V2 `StreamingRecognize` (`chirp_3`, the `_` recognizer) with the service account: audio from
/// the Chirp 3 HD voice, interim results asserted (`emits_partials() == true`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn google_stt_live_v2_chirp_3_vad_gated_roundtrip() {
    let Some(()) = service_account() else { return };
    let tts = GoogleTts::new(&tts_config(CHIRP3_VOICE, None)).expect("tts config");
    let stereo = phrase_audio(&tts).await;

    let mut provider = GoogleStt::new(&stt_config("chirp_3", None)).expect("stt config");
    let sample = measure_stt(&mut provider, &stereo, &STT_WORDS)
        .await
        .unwrap_or_else(|err| panic!("{ID} v2 stt failed: {err}"));
    let emits_partials = provider.emits_partials();
    let used = provider.last_transport();
    let _ = provider.stop().await;
    assert!(emits_partials, "{ID}: V2 chirp_3 must emit partials");
    assert_eq!(used.as_deref(), Some("grpc"));
    assert_stt_contract(&format!("{ID}_v2"), emits_partials, &sample);
}

/// An API key reaches no gRPC path: V2 `StreamingRecognize` and `StreamingSynthesize` fail with a
/// named config error before any network call (no credentials needed).
#[tokio::test]
async fn google_api_key_on_a_grpc_path_is_a_named_config_error() {
    let key = "not-a-real-key-0123456789";

    let mut stt = GoogleStt::new(&SttConfig {
        endpoint: Some("projects/p/locations/us/recognizers/_".into()),
        ..stt_config("chirp_3", Some(key.into()))
    })
    .unwrap()
    .__without_service_account();
    let err = stt.start().await.expect_err("a key alone cannot open V2");
    let message = err.to_string();
    assert!(matches!(err, SpeechError::Config(_)), "{message}");
    assert!(
        message.contains(
            "google stt chirp_3 needs a service account (GOOGLE_APPLICATION_CREDENTIALS)"
        ),
        "{message}"
    );
    assert!(message.contains("API keys work only for"), "{message}");
    assert!(!message.contains(key), "{message}");

    let tts = GoogleTts::new(&tts_config(CHIRP3_VOICE, Some(key.into())))
        .unwrap()
        .__without_service_account();
    let err = tts
        .synthesize("hello")
        .await
        .expect_err("a key alone cannot stream");
    let message = err.to_string();
    assert!(matches!(err, SpeechError::Config(_)), "{message}");
    assert!(
        message.contains(&format!(
            "google tts {CHIRP3_VOICE} needs a service account (GOOGLE_APPLICATION_CREDENTIALS)"
        )),
        "{message}"
    );
    assert!(!message.contains(key), "{message}");
}

/// The catalog lists no transports for Google (the model or voice decides), so the REST and gRPC
/// tests above cover everything.
#[test]
fn google_live_tests_cover_every_catalog_transport() {
    assert!(voice_catalog::tts_transports(ID).is_empty());
    assert!(voice_catalog::stt_transports(ID).is_empty());
}
