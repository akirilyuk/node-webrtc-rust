#![cfg(feature = "live")]
//! Local mock OpenAI `/audio/speech` server. Guards that the buffered (full-body) path sends the
//! configured voice and model with the documented `response_format: pcm`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_openai::OpenAiFactory;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const BOUND: Duration = Duration::from_secs(10);

/// Read one HTTP request and return `(request_line, json_body)`.
async fn read_request(stream: &mut tokio::net::TcpStream) -> (String, serde_json::Value) {
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 4096];
    loop {
        let n = stream.read(&mut tmp).await.unwrap();
        assert!(n > 0, "connection closed before request completed");
        buf.extend_from_slice(&tmp[..n]);
        let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let content_length = head
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let body_start = head_end + 4;
        if buf.len() >= body_start + content_length {
            let body = serde_json::from_slice(&buf[body_start..body_start + content_length])
                .expect("request body is JSON");
            let line = head.lines().next().unwrap_or("").to_string();
            return (line, body);
        }
    }
}

#[tokio::test]
async fn full_body_sends_the_configured_voice() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Arc<Mutex<Option<(String, serde_json::Value)>>> = Arc::new(Mutex::new(None));
    {
        let seen = seen.clone();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            *seen.lock().unwrap() = Some(request);
            let pcm = vec![0_u8; 480];
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: audio/pcm\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                pcm.len()
            );
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(&pcm).await.unwrap();
            let _ = stream.shutdown().await;
        });
    }

    let config = TtsConfig {
        provider: TtsVendor::Openai,
        model: Some("gpt-4o-mini-tts".into()),
        model_path: None,
        voice: Some("marin".into()),
        api_key: Some("test-key".into()),
        endpoint: Some(format!("http://{addr}/v1")),
        sentence_case: Default::default(),
        transport: None,
    };
    let tts = OpenAiFactory.create_tts(&config).expect("openai tts");
    let chunks = tokio::time::timeout(BOUND, tts.synthesize("hello"))
        .await
        .expect("synthesize within bound")
        .expect("synthesize ok");
    assert!(!chunks.is_empty());

    let (line, body) = seen.lock().unwrap().take().expect("request seen");
    assert!(line.starts_with("POST /v1/audio/speech"), "{line}");
    assert_eq!(body["voice"], "marin");
    assert_eq!(body["model"], "gpt-4o-mini-tts");
    assert_eq!(body["response_format"], "pcm");
    assert_eq!(body["input"], "hello");
}
