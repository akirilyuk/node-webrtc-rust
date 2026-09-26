//! Committed tonic/prost stubs for `proto/speech/v1/speech.proto`.
//!
//! Regenerate with `cargo build -p node-webrtc-rust-speech-proto --features regen-proto`
//! and commit `src/generated/speech.v1.rs` together with the `.proto`.
//!
//! Features: `client` (default) and `server` gate the generated service modules so a
//! later server binary can depend on `features = ["server"]` without pulling client
//! channel code when default features are off.

pub mod v1 {
    include!("generated/speech.v1.rs");
}

#[cfg(test)]
mod tests {
    use prost::Message;

    #[test]
    fn generated_contract_mentions_service_and_relocate() {
        let generated = include_str!("generated/speech.v1.rs");
        assert!(
            generated.contains("pub mod speech_client"),
            "committed stubs missing speech_client"
        );
        assert!(
            generated.contains("pub mod speech_server"),
            "committed stubs missing speech_server"
        );
        assert!(
            generated.contains("\"speech.v1.Speech\""),
            "committed stubs missing gRPC service name"
        );
        assert!(
            generated.contains("struct SttRelocate"),
            "committed stubs missing SttRelocate"
        );

        let proto = include_str!("../proto/speech/v1/speech.proto");
        assert!(proto.contains("reserved 4"));
        assert!(proto.contains("message SttRelocate"));
        // The sketch comments mention the removed set_model draft; no live field or rpc uses it.
        assert!(
            proto.lines().all(|line| {
                let code = line.split("//").next().unwrap_or("");
                !code.contains("set_model")
            }),
            "set_model must stay removed from the wire contract"
        );
    }

    #[test]
    fn prepare_request_prost_round_trip() {
        let req = crate::v1::PrepareRequest {
            stt: vec![crate::v1::ModelRef {
                model_path: "/models/sherpa/stt/bundle".to_string(),
                catalog_id: String::new(),
            }],
            tts: vec![crate::v1::ModelRef {
                model_path: "/models/sherpa/tts/voice".to_string(),
                catalog_id: String::new(),
            }],
        };
        let encoded = req.encode_to_vec();
        let decoded = crate::v1::PrepareRequest::decode(encoded.as_slice()).expect("decode");
        assert_eq!(decoded, req);
        assert!(!encoded.is_empty());
    }
}
