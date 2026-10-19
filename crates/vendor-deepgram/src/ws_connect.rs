//! Bounded WebSocket connect shared by the TTS and STT legs (`infra.unbounded-external-wait`).

use std::time::Duration;

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::tts_matrix::WS_CONNECT_TIMEOUT;

pub(crate) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Opens `url` with the `Authorization: Token` header. `leg` (`tts` / `stt`) names the error:
/// `deepgram <leg> websocket connect timed out after 10 s`.
pub(crate) async fn connect(url: String, api_key: &str, leg: &str) -> SpeechResult<Socket> {
    connect_with_bound(url, api_key, leg, WS_CONNECT_TIMEOUT).await
}

pub(crate) async fn connect_with_bound(
    url: String,
    api_key: &str,
    leg: &str,
    bound: Duration,
) -> SpeechResult<Socket> {
    let vendor_error = |message: String| SpeechError::Vendor {
        vendor: "deepgram".into(),
        message,
    };
    let mut request = url
        .into_client_request()
        .map_err(|err| vendor_error(err.to_string()))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Token {api_key}")
            .parse()
            .map_err(|err| vendor_error(format!("invalid auth header: {err}")))?,
    );
    let (ws, _) = tokio::time::timeout(bound, connect_async(request))
        .await
        .map_err(|_| {
            vendor_error(format!(
                "deepgram {leg} websocket connect timed out after {} s",
                bound.as_secs()
            ))
        })?
        .map_err(|err| vendor_error(err.to_string()))?;
    Ok(ws)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listener that accepts the TCP connection and never answers the upgrade.
    async fn silent_listener() -> (String, tokio::net::TcpListener) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/v1/x", listener.local_addr().unwrap());
        (url, listener)
    }

    #[tokio::test(start_paused = true)]
    async fn tts_connect_that_never_handshakes_times_out_at_the_bound() {
        let (url, listener) = silent_listener().await;
        let _hold = tokio::spawn(async move {
            let _conn = listener.accept().await;
            std::future::pending::<()>().await;
        });
        let started = tokio::time::Instant::now();
        let err = connect(url, "k", "tts").await.expect_err("must fail");
        match err {
            SpeechError::Vendor { message, .. } => {
                assert_eq!(
                    message,
                    "deepgram tts websocket connect timed out after 10 s"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() <= Duration::from_secs(10) + Duration::from_millis(50));
    }

    #[tokio::test(start_paused = true)]
    async fn stt_connect_that_never_handshakes_times_out_at_the_bound() {
        let (url, listener) = silent_listener().await;
        let _hold = tokio::spawn(async move {
            let _conn = listener.accept().await;
            std::future::pending::<()>().await;
        });
        let err = connect(url, "k", "stt").await.expect_err("must fail");
        match err {
            SpeechError::Vendor { message, .. } => {
                assert_eq!(
                    message,
                    "deepgram stt websocket connect timed out after 10 s"
                );
            }
            other => panic!("{other:?}"),
        }
    }
}
