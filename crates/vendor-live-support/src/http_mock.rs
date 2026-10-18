//! Minimal keep-alive HTTP/1.1 server for the "one pooled client reuses one connection" and
//! "a server that never answers is bounded" tests of the vendor crates. Plain HTTP only, so the
//! client under test reports `http/1.1`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// What the server does with every request.
#[derive(Clone)]
pub enum Reply {
    /// Answers each request with this status line code, content type and body.
    Fixed {
        status: u16,
        content_type: &'static str,
        body: Vec<u8>,
    },
    /// Reads the request and never answers.
    Hang,
}

impl Reply {
    pub fn ok(content_type: &'static str, body: Vec<u8>) -> Self {
        Self::Fixed {
            status: 200,
            content_type,
            body,
        }
    }
}

pub struct MockHttp {
    addr: SocketAddr,
    accepted: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
    heads: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl MockHttp {
    pub async fn start(reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock http");
        let addr = listener.local_addr().expect("mock http addr");
        let accepted = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let heads = Arc::new(Mutex::new(Vec::new()));
        let task = {
            let (accepted, requests, heads, closed) = (
                accepted.clone(),
                requests.clone(),
                heads.clone(),
                closed.clone(),
            );
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    accepted.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(serve(
                        stream,
                        reply.clone(),
                        requests.clone(),
                        heads.clone(),
                        closed.clone(),
                    ));
                }
            })
        };
        Self {
            addr,
            accepted,
            requests,
            closed,
            heads,
            task,
        }
    }

    /// `http://127.0.0.1:<port>` without a trailing slash.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn host_port(&self) -> String {
        self.addr.to_string()
    }

    /// TCP connections accepted so far.
    pub fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// Connections the peer (the client under test) has closed. A client whose request timed out
    /// drops its connection, so this reaches the accepted count after the timeout.
    pub fn closed(&self) -> usize {
        self.closed.load(Ordering::SeqCst)
    }

    /// Complete requests read so far.
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    /// Request lines and headers of every request, lower-cased header names kept as sent.
    pub fn heads(&self) -> Vec<String> {
        self.heads.lock().expect("heads lock").clone()
    }
}

impl Drop for MockHttp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    mut stream: TcpStream,
    reply: Reply,
    requests: Arc<AtomicUsize>,
    heads: Arc<Mutex<Vec<String>>>,
    closed: Arc<AtomicUsize>,
) {
    struct CountClose(Arc<AtomicUsize>);
    impl Drop for CountClose {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let _count_close = CountClose(closed);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let Some(head_end) = read_until_head_end(&mut stream, &mut buf).await else {
            return;
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        buf.drain(..head_end + 4);
        let lower = head.to_ascii_lowercase();
        if lower.contains("transfer-encoding: chunked") {
            if !drain_chunked(&mut stream, &mut buf).await {
                return;
            }
        } else {
            let len = lower
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            while buf.len() < len {
                if !fill(&mut stream, &mut buf).await {
                    return;
                }
            }
            buf.drain(..len);
        }
        requests.fetch_add(1, Ordering::SeqCst);
        heads.lock().expect("heads lock").push(head);
        match &reply {
            // Never answer; return (and count a close) once the client drops the connection.
            Reply::Hang => while fill(&mut stream, &mut buf).await {},
            Reply::Fixed {
                status,
                content_type,
                body,
            } => {
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                if stream.write_all(head.as_bytes()).await.is_err()
                    || stream.write_all(body).await.is_err()
                {
                    return;
                }
            }
        }
    }
}

async fn fill(stream: &mut TcpStream, buf: &mut Vec<u8>) -> bool {
    let mut tmp = [0u8; 4096];
    match stream.read(&mut tmp).await {
        Ok(0) | Err(_) => false,
        Ok(n) => {
            buf.extend_from_slice(&tmp[..n]);
            true
        }
    }
}

async fn read_until_head_end(stream: &mut TcpStream, buf: &mut Vec<u8>) -> Option<usize> {
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            return Some(pos);
        }
        if !fill(stream, buf).await {
            return None;
        }
    }
}

/// Consumes a chunked request body (up to and including the terminating `0` chunk).
async fn drain_chunked(stream: &mut TcpStream, buf: &mut Vec<u8>) -> bool {
    loop {
        let line_end = loop {
            if let Some(pos) = buf.windows(2).position(|w| w == b"\r\n") {
                break pos;
            }
            if !fill(stream, buf).await {
                return false;
            }
        };
        let size_text = String::from_utf8_lossy(&buf[..line_end]).to_string();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("0").trim(), 16)
            .unwrap_or(0);
        buf.drain(..line_end + 2);
        // chunk data + CRLF (for the last chunk: the final CRLF of the trailer section)
        while buf.len() < size + 2 {
            if !fill(stream, buf).await {
                return false;
            }
        }
        buf.drain(..size + 2);
        if size == 0 {
            return true;
        }
    }
}

/// A 16-bit mono PCM WAV (`rate` Hz) holding `ms` milliseconds of a quiet tone.
pub fn wav_16bit_mono(rate: u32, ms: u32) -> Vec<u8> {
    let samples = (rate as usize * ms as usize) / 1000;
    let data_len = (samples * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..samples {
        let v: i16 = if i % 2 == 0 { 800 } else { -800 };
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// `ms` milliseconds of quiet 16-bit mono PCM at `rate` Hz, no header.
pub fn pcm_16bit_mono(rate: u32, ms: u32) -> Vec<u8> {
    wav_16bit_mono(rate, ms)[44..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn counts_accepts_and_requests_on_a_kept_alive_connection() {
        let mock = MockHttp::start(Reply::ok("text/plain", b"hi".to_vec())).await;
        let mut s = TcpStream::connect(mock.host_port()).await.unwrap();
        for _ in 0..2 {
            s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .unwrap();
            let mut got = Vec::new();
            while !got.ends_with(b"hi") {
                let mut tmp = [0u8; 256];
                let n = s.read(&mut tmp).await.unwrap();
                got.extend_from_slice(&tmp[..n]);
            }
        }
        assert_eq!((mock.accepted(), mock.requests()), (1, 2));
    }

    #[test]
    fn wav_header_is_44_bytes() {
        let wav = wav_16bit_mono(16_000, 100);
        assert_eq!(wav.len(), 44 + 3200);
        assert_eq!(pcm_16bit_mono(16_000, 100).len(), 3200);
    }
}
