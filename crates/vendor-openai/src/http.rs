//! One pooled `reqwest::Client` per provider instance.

use std::time::Duration;

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};

use crate::matrix::{CONNECT_TIMEOUT, HTTP_TOTAL_TIMEOUT, POOL_IDLE_TIMEOUT, TCP_KEEPALIVE};

/// Builds the client a provider keeps for its whole life. HTTP version: ALPN default (h2 when the
/// server offers it, else HTTP/1.1).
pub(crate) fn build_client(total: Option<Duration>) -> SpeechResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(total.unwrap_or(HTTP_TOTAL_TIMEOUT))
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .tcp_keepalive(TCP_KEEPALIVE)
        .build()
        .map_err(|err| SpeechError::Vendor {
            vendor: "openai".into(),
            message: format!("http client: {err}"),
        })
}

/// `h2` or `http/1.1` for `last_transport()`.
pub(crate) fn transport_label(version: reqwest::Version) -> String {
    if version == reqwest::Version::HTTP_2 {
        "h2".into()
    } else {
        "http/1.1".into()
    }
}

/// `last_transport()` value for requests made through the `async-openai` client, which does not
/// expose the response's HTTP version.
pub(crate) const SDK_TRANSPORT: &str = "sdk";
