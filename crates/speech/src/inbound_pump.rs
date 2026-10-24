//! Inbound audio pump: reads remote audio and feeds the agent on separate tasks.
//!
//! The reader task never waits for the agent. It reads a frame, runs `on_read` (mix-graph push,
//! read tee) and hands the frame to the processor task over an unbounded queue. While the agent
//! is busy (an STT finalize waiting on the vendor, a stream open retry) the queue grows and the
//! audio path keeps its pace. Frames are never dropped; the processor drains the backlog once
//! the agent is free again.
//!
//! This module is pure (no napi) so the behaviour is testable with virtual time.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Boxed future handed to the caller's `spawn` function.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Result of one read from the remote track.
#[derive(Debug, Clone)]
pub enum InboundRead {
    /// One decoded frame (48 kHz stereo s16le).
    Pcm {
        pcm: Bytes,
        duration_ms: u32,
        /// Synthesized by loss concealment, not decoded from a received packet.
        concealed: bool,
    },
    /// The receiver stopped: wait for the next RTP burst.
    StreamEnd,
    /// Any other read error (retried after a short pause).
    Error(String),
}

/// Injected after a stream end so the agent's VAD/STT sees the end of the utterance.
pub const INBOUND_SILENCE_TAIL_FRAMES: u32 = 75;
/// Duration of one inbound frame.
pub const INBOUND_FRAME_MS: u64 = 20;
/// Backlog (ms of audio waiting for the agent) at which a warning is printed once per episode.
pub const INBOUND_BACKLOG_WARN_MS: u64 = 1_000;

/// 20 ms of 48 kHz stereo s16le.
const SILENCE_FRAME_BYTES: usize = 3840;

/// `VOICE_DEBUG=1|true|yes`, read once per process.
fn voice_debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("VOICE_DEBUG").ok().as_deref(),
            Some("1") | Some("true") | Some("yes")
        )
    })
}

/// The two pump tasks plus backlog gauges.
pub struct InboundPumpHandles {
    pub reader: JoinHandle<()>,
    pub processor: JoinHandle<()>,
    /// Frames read and not yet processed (the frame being processed counts).
    pub backlog_frames: Arc<AtomicUsize>,
    /// Highest `backlog_frames` seen.
    pub max_backlog_frames: Arc<AtomicUsize>,
}

impl InboundPumpHandles {
    /// Abort both tasks.
    pub fn abort(&self) {
        self.reader.abort();
        self.processor.abort();
    }

    /// `true` only when both tasks have finished.
    pub fn is_finished(&self) -> bool {
        self.reader.is_finished() && self.processor.is_finished()
    }
}

/// Spawn the reader and processor tasks.
///
/// * `read` — next frame from the remote track.
/// * `on_read` — runs on the reader task for every real frame, before it is queued (mix graph,
///   read tee). Must not block.
/// * `process` — feeds the agent (`pcm`, `duration_ms`, `concealed`); runs on the processor task, one frame at a time, in order.
///   An `Err` is logged under `VOICE_DEBUG` only.
/// * `running` — the pump stops reading once this returns `false`; the processor then drains
///   what is queued and exits.
/// * `spawn` — how to start a task (`tokio::spawn`, or the napi runtime's `spawn`).
pub fn spawn_inbound_pump<Rd, RdFut, OnRead, Proc, ProcFut, Run>(
    mut read: Rd,
    on_read: OnRead,
    process: Proc,
    running: Run,
    spawn: impl Fn(BoxFuture<'static, ()>) -> JoinHandle<()>,
) -> InboundPumpHandles
where
    Rd: FnMut() -> RdFut + Send + 'static,
    RdFut: Future<Output = InboundRead> + Send + 'static,
    OnRead: Fn(&Bytes, u32) + Send + 'static,
    Proc: Fn(Bytes, u32, bool) -> ProcFut + Send + 'static,
    ProcFut: Future<Output = Result<(), String>> + Send + 'static,
    Run: Fn() -> bool + Send + 'static,
{
    let backlog = Arc::new(AtomicUsize::new(0));
    let max_backlog = Arc::new(AtomicUsize::new(0));
    let warned = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = mpsc::unbounded_channel::<(Bytes, u32, bool)>();
    let warn_frames = (INBOUND_BACKLOG_WARN_MS / INBOUND_FRAME_MS) as usize;

    let reader = {
        let backlog = Arc::clone(&backlog);
        let max_backlog = Arc::clone(&max_backlog);
        let warned = Arc::clone(&warned);
        spawn(Box::pin(async move {
            let enqueue = |pcm: Bytes, duration_ms: u32, concealed: bool| {
                let now = backlog.fetch_add(1, Ordering::SeqCst) + 1;
                max_backlog.fetch_max(now, Ordering::SeqCst);
                if now > warn_frames && !warned.swap(true, Ordering::SeqCst) {
                    eprintln!(
                        "[voice] inbound processing behind by {} ms (agent busy: STT finalize or CPU); audio path unaffected",
                        now as u64 * INBOUND_FRAME_MS
                    );
                }
                tx.send((pcm, duration_ms, concealed)).is_ok()
            };
            let mut awaiting_next_burst = false;
            while running() {
                match read().await {
                    InboundRead::Pcm {
                        pcm,
                        duration_ms,
                        concealed,
                    } => {
                        awaiting_next_burst = false;
                        on_read(&pcm, duration_ms);
                        if !enqueue(pcm, duration_ms, concealed) {
                            break;
                        }
                    }
                    InboundRead::StreamEnd => {
                        if !awaiting_next_burst {
                            awaiting_next_burst = true;
                            let silent = Bytes::from(vec![0u8; SILENCE_FRAME_BYTES]);
                            for _ in 0..INBOUND_SILENCE_TAIL_FRAMES {
                                if !running() {
                                    break;
                                }
                                if !enqueue(silent.clone(), INBOUND_FRAME_MS as u32, false) {
                                    return;
                                }
                                tokio::time::sleep(Duration::from_millis(INBOUND_FRAME_MS)).await;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    InboundRead::Error(err) => {
                        if voice_debug_enabled() {
                            eprintln!("[voice-debug] native inbound readSample error: {err}");
                        }
                        tokio::time::sleep(Duration::from_millis(INBOUND_FRAME_MS)).await;
                    }
                }
            }
        }))
    };

    let processor = {
        let backlog = Arc::clone(&backlog);
        let warned = Arc::clone(&warned);
        spawn(Box::pin(async move {
            while let Some((pcm, duration_ms, concealed)) = rx.recv().await {
                if let Err(err) = process(pcm, duration_ms, concealed).await {
                    if voice_debug_enabled() {
                        eprintln!("[voice-debug] native inbound processInboundPcm error: {err}");
                    }
                }
                if backlog.fetch_sub(1, Ordering::SeqCst) == 1 {
                    warned.store(false, Ordering::SeqCst);
                }
            }
        }))
    };

    InboundPumpHandles {
        reader,
        processor,
        backlog_frames: backlog,
        max_backlog_frames: max_backlog,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use tokio::sync::Notify;
    use tokio::time::{sleep, Instant};

    use super::*;

    fn spawn_tokio(f: BoxFuture<'static, ()>) -> JoinHandle<()> {
        tokio::spawn(f)
    }

    fn indexed_frame(i: u32) -> Bytes {
        Bytes::from(i.to_be_bytes().to_vec())
    }

    fn frame_index(pcm: &Bytes) -> u32 {
        u32::from_be_bytes(pcm.as_ref().try_into().expect("indexed frame"))
    }

    /// Reader that yields `count` indexed frames, one per 20 ms, then waits forever.
    fn paced_reader(
        count: u32,
    ) -> impl FnMut() -> BoxFuture<'static, InboundRead> + Send + 'static {
        let mut next = 0_u32;
        move || {
            let i = next;
            next += 1;
            Box::pin(async move {
                if i >= count {
                    std::future::pending::<()>().await;
                }
                sleep(Duration::from_millis(INBOUND_FRAME_MS)).await;
                InboundRead::Pcm {
                    pcm: indexed_frame(i),
                    duration_ms: INBOUND_FRAME_MS as u32,
                    concealed: false,
                }
            })
        }
    }

    /// Processor whose first frame blocks until the gate is opened.
    struct GatedProcessor {
        gate: Arc<Notify>,
        first_done: Arc<AtomicBool>,
        received: Arc<Mutex<Vec<Bytes>>>,
    }

    impl GatedProcessor {
        fn new() -> Self {
            Self {
                gate: Arc::new(Notify::new()),
                first_done: Arc::new(AtomicBool::new(false)),
                received: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn process_fn(
            &self,
        ) -> impl Fn(Bytes, u32, bool) -> BoxFuture<'static, Result<(), String>> + Send + 'static
        {
            let gate = Arc::clone(&self.gate);
            let first_done = Arc::clone(&self.first_done);
            let received = Arc::clone(&self.received);
            move |pcm, _ms, _concealed| {
                let gate = Arc::clone(&gate);
                let first_done = Arc::clone(&first_done);
                let received = Arc::clone(&received);
                Box::pin(async move {
                    received.lock().unwrap().push(pcm);
                    if !first_done.swap(true, Ordering::SeqCst) {
                        gate.notified().await;
                    }
                    Ok(())
                })
            }
        }

        fn received_len(&self) -> usize {
            self.received.lock().unwrap().len()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn audio_path_keeps_pace_while_processing_is_blocked() {
        let start = Instant::now();
        let proc = GatedProcessor::new();
        let read_at: Arc<Mutex<Vec<Instant>>> = Arc::new(Mutex::new(Vec::new()));
        let read_at_cb = Arc::clone(&read_at);
        let handles = spawn_inbound_pump(
            paced_reader(200),
            move |_pcm, _ms| read_at_cb.lock().unwrap().push(Instant::now()),
            proc.process_fn(),
            || true,
            spawn_tokio,
        );

        sleep(Duration::from_secs(3)).await;
        assert_eq!(proc.received_len(), 1, "processor is blocked on frame 0");
        proc.gate.notify_one();
        sleep(Duration::from_millis(1_200)).await;

        let times = read_at.lock().unwrap().clone();
        assert_eq!(times.len(), 200, "on_read ran for every frame");
        let last = times[199].duration_since(start);
        assert!(
            last <= Duration::from_millis(4_100),
            "200th frame read at {last:?}"
        );
        let received = proc.received.lock().unwrap().clone();
        let indices: Vec<u32> = received.iter().map(frame_index).collect();
        assert_eq!(indices, (0..200).collect::<Vec<_>>());
        assert!(
            handles.max_backlog_frames.load(Ordering::SeqCst) >= 140,
            "max backlog {}",
            handles.max_backlog_frames.load(Ordering::SeqCst)
        );
        assert_eq!(handles.backlog_frames.load(Ordering::SeqCst), 0);
        handles.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn no_frame_is_dropped_under_a_long_stall() {
        let proc = GatedProcessor::new();
        let handles = spawn_inbound_pump(
            paced_reader(1_500),
            |_pcm, _ms| {},
            proc.process_fn(),
            || true,
            spawn_tokio,
        );
        sleep(Duration::from_secs(30)).await;
        assert_eq!(proc.received_len(), 1);
        proc.gate.notify_one();
        sleep(Duration::from_secs(1)).await;

        let indices: Vec<u32> = proc
            .received
            .lock()
            .unwrap()
            .iter()
            .map(frame_index)
            .collect();
        assert_eq!(indices, (0..1_500).collect::<Vec<_>>());
        handles.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn stream_end_feeds_one_silence_tail_then_waits() {
        // Scripted reads; an empty script keeps answering StreamEnd.
        let script: Arc<Mutex<VecDeque<InboundRead>>> = Arc::new(Mutex::new(VecDeque::new()));
        let script_rd = Arc::clone(&script);
        let received: Arc<Mutex<Vec<Bytes>>> = Arc::new(Mutex::new(Vec::new()));
        let received_p = Arc::clone(&received);
        let handles = spawn_inbound_pump(
            move || {
                let next = script_rd
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(InboundRead::StreamEnd);
                Box::pin(async move { next }) as BoxFuture<'static, InboundRead>
            },
            |_pcm, _ms| {},
            move |pcm, _ms, _concealed| {
                received_p.lock().unwrap().push(pcm);
                Box::pin(async { Ok(()) }) as BoxFuture<'static, Result<(), String>>
            },
            || true,
            spawn_tokio,
        );

        let silent_count = |r: &Arc<Mutex<Vec<Bytes>>>| {
            r.lock()
                .unwrap()
                .iter()
                .filter(|b| b.len() == SILENCE_FRAME_BYTES && b.iter().all(|x| *x == 0))
                .count()
        };

        sleep(Duration::from_millis(1_700)).await;
        assert_eq!(
            silent_count(&received),
            INBOUND_SILENCE_TAIL_FRAMES as usize
        );
        sleep(Duration::from_secs(2)).await;
        assert_eq!(
            received.lock().unwrap().len(),
            INBOUND_SILENCE_TAIL_FRAMES as usize,
            "no more frames after the tail"
        );

        // A real frame re-arms the tail for the next stream end.
        script.lock().unwrap().push_back(InboundRead::Pcm {
            pcm: indexed_frame(7),
            duration_ms: 20,
            concealed: false,
        });
        sleep(Duration::from_millis(1_800)).await;
        assert_eq!(
            received.lock().unwrap().len(),
            2 * INBOUND_SILENCE_TAIL_FRAMES as usize + 1
        );
        assert_eq!(
            silent_count(&received),
            2 * INBOUND_SILENCE_TAIL_FRAMES as usize
        );
        handles.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn abort_stops_both_tasks() {
        let proc = GatedProcessor::new();
        let handles = spawn_inbound_pump(
            paced_reader(1_000),
            |_pcm, _ms| {},
            proc.process_fn(),
            || true,
            spawn_tokio,
        );
        sleep(Duration::from_millis(500)).await;
        assert!(!handles.is_finished());
        handles.abort();
        sleep(Duration::from_millis(100)).await;
        assert!(handles.reader.is_finished());
        assert!(handles.processor.is_finished());
        assert!(handles.is_finished());
    }

    #[tokio::test(start_paused = true)]
    async fn processor_drains_queue_after_reader_stops() {
        let proc = GatedProcessor::new();
        let run = Arc::new(AtomicBool::new(true));
        let run_rd = Arc::clone(&run);
        let reads = Arc::new(AtomicUsize::new(0));
        let handles = spawn_inbound_pump(
            paced_reader(1_000),
            move |_pcm, _ms| {
                if reads.fetch_add(1, Ordering::SeqCst) + 1 == 10 {
                    run_rd.store(false, Ordering::SeqCst);
                }
            },
            proc.process_fn(),
            move || run.load(Ordering::SeqCst),
            spawn_tokio,
        );

        sleep(Duration::from_secs(1)).await;
        assert!(
            handles.reader.is_finished(),
            "reader stopped after 10 frames"
        );
        assert!(!handles.processor.is_finished(), "processor still blocked");
        assert_eq!(proc.received_len(), 1);
        proc.gate.notify_one();
        sleep(Duration::from_millis(100)).await;

        let indices: Vec<u32> = proc
            .received
            .lock()
            .unwrap()
            .iter()
            .map(frame_index)
            .collect();
        assert_eq!(indices, (0..10).collect::<Vec<_>>());
        assert!(handles.processor.is_finished());
    }
}
