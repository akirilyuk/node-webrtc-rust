//! `infra.tls-provider-unselected`: this binary installs no rustls CryptoProvider, like a plain Rust
//! consumer of the crate. Opening a `wss://` connection must return an error (the peer here is not a
//! TLS server), not panic in `rustls crypto/mod.rs`. Keep this the only test in the file: the
//! provider is process-global.

use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_assemblyai::AssemblyAiFactory;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

#[tokio::test]
async fn wss_connect_without_an_installed_provider_errors_instead_of_panicking() {
    assert!(
        rustls::crypto::CryptoProvider::get_default().is_none(),
        "test precondition: no provider installed"
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("https://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            // Not a TLS server: the handshake fails after the client config was built.
            let _ = stream.write_all(b"not tls").await;
        }
    });
    let mut provider = AssemblyAiFactory
        .create_stt(&SttConfig {
            provider: SttVendor::Assemblyai,
            model: None,
            model_path: None,
            language: None,
            api_key: Some("k".into()),
            endpoint: Some(endpoint),
            transport: None,
        })
        .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), provider.start())
        .await
        .expect("connect finished");
    assert!(result.is_err(), "a non-TLS peer must fail the handshake");
}
