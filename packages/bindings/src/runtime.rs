//! Tokio runtime initialization for async NAPI methods.

use napi::bindgen_prelude::create_custom_tokio_runtime;
use tokio::runtime::Builder;

/// rustls 0.23 panics when both `ring` and `aws-lc-rs` crate features are
/// enabled unless a process-level [`rustls::crypto::CryptoProvider`] is
/// installed first.
///
/// Bindings link both backends: reqwest / `tokio-tungstenite` enable `ring`,
/// while the AWS SDK default HTTPS client and `google-cloud-auth` enable
/// `aws-lc-rs`. Without an explicit provider, Tokio workers panic during TLS
/// and WebRTC peers stay `connecting` (CI: `mix-graph-three-peer`).
fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[napi::module_init]
fn init() {
    install_rustls_crypto_provider();
    let rt = Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("failed to create Tokio runtime");
    create_custom_tokio_runtime(rt);
}
