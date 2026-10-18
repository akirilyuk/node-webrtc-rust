//! Bounded WebSocket connect and send (`infra.unbounded-external-wait`).

use std::time::Duration;

use futures_util::{Sink, SinkExt};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::matrix::WS_BOUND;

pub(crate) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn vendor_error(message: String) -> SpeechError {
    SpeechError::Vendor {
        vendor: "assemblyai".into(),
        message,
    }
}

/// Opens `url` with the `Authorization` header. A handshake that does not finish in 10 s is
/// `assemblyai stt websocket connect timed out after 10 s`.
pub(crate) async fn connect(url: String, api_key: &str) -> SpeechResult<Socket> {
    connect_with_bound(url, api_key, WS_BOUND).await
}

pub(crate) async fn connect_with_bound(
    url: String,
    api_key: &str,
    bound: Duration,
) -> SpeechResult<Socket> {
    let mut request = url
        .into_client_request()
        .map_err(|err| vendor_error(err.to_string()))?;
    request.headers_mut().insert(
        "Authorization",
        api_key
            .parse()
            .map_err(|err| vendor_error(format!("invalid auth header: {err}")))?,
    );
    let (ws, _) = tokio::time::timeout(bound, connect_async(request))
        .await
        .map_err(|_| {
            vendor_error(format!(
                "assemblyai stt websocket connect timed out after {} s",
                bound.as_secs()
            ))
        })?
        .map_err(|err| vendor_error(err.to_string()))?;
    Ok(ws)
}

/// One send, bounded: `assemblyai stt websocket send timed out after 10 s`.
pub(crate) async fn send_bounded<S>(
    sink: &mut S,
    message: Message,
    bound: Duration,
) -> SpeechResult<()>
where
    S: Sink<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    match tokio::time::timeout(bound, sink.send(message)).await {
        Err(_) => Err(vendor_error(format!(
            "assemblyai stt websocket send timed out after {} s",
            bound.as_secs()
        ))),
        Ok(Err(err)) => Err(vendor_error(err.to_string())),
        Ok(Ok(())) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    #[tokio::test(start_paused = true)]
    async fn connect_that_never_handshakes_times_out_at_the_bound() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/v3/ws", listener.local_addr().unwrap());
        let _hold = tokio::spawn(async move {
            let _conn = listener.accept().await;
            std::future::pending::<()>().await;
        });
        let started = tokio::time::Instant::now();
        match connect(url, "k").await {
            Err(SpeechError::Vendor { message, .. }) => {
                assert_eq!(
                    message,
                    "assemblyai stt websocket connect timed out after 10 s"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() <= Duration::from_secs(10) + Duration::from_millis(50));
    }

    /// A sink that is never ready, like a socket whose peer stopped reading and whose buffers are full.
    struct StuckSink;

    impl Sink<Message> for StuckSink {
        type Error = std::convert::Infallible;
        fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Pending
        }
        fn start_send(self: Pin<&mut Self>, _: Message) -> Result<(), Self::Error> {
            Ok(())
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Pending
        }
        fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Pending
        }
    }

    #[tokio::test(start_paused = true)]
    async fn send_that_never_completes_times_out_at_the_bound() {
        let started = tokio::time::Instant::now();
        match send_bounded(&mut StuckSink, Message::Text("x".into()), WS_BOUND).await {
            Err(SpeechError::Vendor { message, .. }) => {
                assert_eq!(
                    message,
                    "assemblyai stt websocket send timed out after 10 s"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() <= Duration::from_secs(10) + Duration::from_millis(50));
    }
}
