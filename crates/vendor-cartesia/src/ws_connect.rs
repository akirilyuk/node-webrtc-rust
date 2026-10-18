//! Bounded WebSocket connect (`infra.unbounded-external-wait`).

use std::time::Duration;

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::matrix::CONNECT_TIMEOUT;

pub(crate) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn vendor_error(message: String) -> SpeechError {
    SpeechError::Vendor {
        vendor: "cartesia".into(),
        message,
    }
}

/// Opens `url` with the documented `X-API-Key` and `Cartesia-Version` headers. A handshake that
/// does not finish in 10 s is `cartesia tts websocket connect timed out after 10 s`.
pub(crate) async fn connect(url: String, api_key: &str, api_version: &str) -> SpeechResult<Socket> {
    connect_with_bound(url, api_key, api_version, CONNECT_TIMEOUT).await
}

pub(crate) async fn connect_with_bound(
    url: String,
    api_key: &str,
    api_version: &str,
    bound: Duration,
) -> SpeechResult<Socket> {
    let mut request = url
        .into_client_request()
        .map_err(|err| vendor_error(err.to_string()))?;
    request.headers_mut().insert(
        "X-API-Key",
        api_key
            .parse()
            .map_err(|_| vendor_error("invalid CARTESIA_API_KEY header value".into()))?,
    );
    request.headers_mut().insert(
        "Cartesia-Version",
        api_version
            .parse()
            .map_err(|_| vendor_error("invalid Cartesia-Version header value".into()))?,
    );
    let (ws, _) = tokio::time::timeout(bound, connect_async(request))
        .await
        .map_err(|_| {
            vendor_error(format!(
                "cartesia tts websocket connect timed out after {} s",
                bound.as_secs()
            ))
        })?
        .map_err(|err| vendor_error(err.to_string()))?;
    Ok(ws)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn connect_that_never_handshakes_times_out_at_the_bound() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/tts/websocket", listener.local_addr().unwrap());
        let _hold = tokio::spawn(async move {
            let _conn = listener.accept().await;
            std::future::pending::<()>().await;
        });
        let started = tokio::time::Instant::now();
        match connect(url, "k", "v").await {
            Err(SpeechError::Vendor { message, .. }) => {
                assert_eq!(
                    message,
                    "cartesia tts websocket connect timed out after 10 s"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() <= Duration::from_secs(10) + Duration::from_millis(50));
    }
}
