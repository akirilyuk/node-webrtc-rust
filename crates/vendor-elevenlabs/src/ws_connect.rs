//! Bounded WebSocket connect shared by the TTS and STT legs (`infra.unbounded-external-wait`).

use std::time::Duration;

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::matrix::WS_CONNECT_TIMEOUT;

pub(crate) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Opens `url` with the `xi-api-key` header. `leg` (`tts` / `stt`) names the error:
/// `elevenlabs <leg> websocket connect timed out after 10 s`.
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
        vendor: "elevenlabs".into(),
        message,
    };
    let mut request = url
        .into_client_request()
        .map_err(|err| vendor_error(err.to_string()))?;
    request.headers_mut().insert(
        "xi-api-key",
        api_key
            .parse()
            .map_err(|_| vendor_error("invalid ELEVENLABS_API_KEY header value".into()))?,
    );
    let (ws, _) = tokio::time::timeout(bound, connect_async(request))
        .await
        .map_err(|_| {
            vendor_error(format!(
                "elevenlabs {leg} websocket connect timed out after {} s",
                bound.as_secs()
            ))
        })?
        .map_err(|err| vendor_error(err.to_string()))?;
    Ok(ws)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn never_handshakes() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/v1/x", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _conn = listener.accept().await;
            std::future::pending::<()>().await;
        });
        url
    }

    async fn assert_times_out(leg: &str) {
        let url = never_handshakes().await;
        match connect(url, "k", leg).await.expect_err("must fail") {
            SpeechError::Vendor { message, .. } => assert_eq!(
                message,
                format!("elevenlabs {leg} websocket connect timed out after 10 s")
            ),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn tts_connect_that_never_handshakes_times_out_at_the_bound() {
        assert_times_out("tts").await;
    }

    #[tokio::test(start_paused = true)]
    async fn stt_connect_that_never_handshakes_times_out_at_the_bound() {
        assert_times_out("stt").await;
    }
}
