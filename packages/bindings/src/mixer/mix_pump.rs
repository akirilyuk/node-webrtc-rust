//! Native client-mix pump: one Rust task per listener replaces the per-frame JS pump.
//!
//! The TTS drain pushes PCM into a [`NativeTtsQueue`]; the actor task renders the listener mix,
//! pans the TTS frame, saturating-sums both, writes the result to the PC outbound track and
//! notifies that track's write tee. Scheduling mirrors the JS pump in
//! `packages/helpers/src/client-audio-mixer.ts` (`pumpMixFrame`): TTS frames are never dropped.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_core::LocalAudioTrack;
use node_webrtc_rust_mixer::{Frame, MixGraph, FRAME_BYTES};
use tokio::sync::{mpsc, oneshot};

use crate::media::WriteSampleTee;

/// One output frame.
pub(crate) const PUMP_FRAME: Duration = Duration::from_millis(20);
/// Max slots emitted from one tick (avoids silence catch-up bursts).
pub(crate) const PUMP_MAX_FRAMES_PER_TICK: u64 = 4;
/// A TTS enqueue within this window keeps the stream "active".
pub(crate) const TTS_ACTIVE_WINDOW: Duration = Duration::from_millis(60);
/// After this many consecutive active-window skips the stream counts as ended.
pub(crate) const TTS_MAX_CONSECUTIVE_SKIPS: u32 = 3;

#[derive(Default)]
struct NativeTtsQueueInner {
    frames: VecDeque<Bytes>,
    carry: Vec<u8>,
    last_enqueue: Option<Instant>,
    max_depth: usize,
}

/// Unbounded FIFO of full 20 ms TTS frames fed by the TTS drain. Never drops.
#[derive(Default)]
pub struct NativeTtsQueue {
    inner: Mutex<NativeTtsQueueInner>,
}

impl NativeTtsQueue {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, NativeTtsQueueInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Appends PCM; every complete [`FRAME_BYTES`] chunk becomes one queued frame.
    pub fn push_pcm(&self, pcm: &[u8]) {
        let mut g = self.lock();
        g.carry.extend_from_slice(pcm);
        let n = g.carry.len() / FRAME_BYTES;
        if n == 0 {
            return;
        }
        for i in 0..n {
            let frame = Bytes::copy_from_slice(&g.carry[i * FRAME_BYTES..(i + 1) * FRAME_BYTES]);
            g.frames.push_back(frame);
        }
        g.carry.drain(..n * FRAME_BYTES);
        g.last_enqueue = Some(Instant::now());
        let depth = g.frames.len();
        if depth > g.max_depth {
            g.max_depth = depth;
        }
    }

    pub fn pop(&self) -> Option<Bytes> {
        self.lock().frames.pop_front()
    }

    pub fn len(&self) -> usize {
        self.lock().frames.len()
    }

    /// Drops queued frames and the partial carry; returns the discarded frame count.
    pub fn discard(&self) -> u32 {
        let mut g = self.lock();
        let n = g.frames.len() as u32;
        g.frames.clear();
        g.carry.clear();
        n
    }

    /// Deepest backlog (frames) since the last call; resets to the current length.
    pub fn take_max_depth(&self) -> usize {
        let mut g = self.lock();
        let max = g.max_depth.max(g.frames.len());
        g.max_depth = g.frames.len();
        max
    }

    pub fn last_enqueue(&self) -> Option<Instant> {
        self.lock().last_enqueue
    }
}

/// What the pump writes for one due slot. Skipped slots produce no action.
pub(crate) enum SlotAction {
    Tts { frame: Bytes, at: Instant },
    Silence { at: Instant },
}

/// Pure wall-clock slot scheduler (no I/O); port of the JS `pumpMixFrame` accounting.
pub(crate) struct PumpScheduler {
    pub(crate) anchor: Instant,
    pub(crate) written: u64,
    pub(crate) consecutive_skips: u32,
    pub(crate) queue_was_non_empty: bool,
    pub(crate) rebases: u64,
    pub(crate) skipped: u64,
}

impl PumpScheduler {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            anchor: now,
            written: 0,
            consecutive_skips: 0,
            queue_was_non_empty: false,
            rebases: 0,
            skipped: 0,
        }
    }

    pub(crate) fn resume(&mut self, now: Instant) {
        self.anchor = now;
        self.written = 0;
        self.consecutive_skips = 0;
    }

    /// Plans the slots due at `now`. `tts` pops the next queued frame; `queue_len_after` is read
    /// once after the loop to set the "queue was non-empty" flag for the next tick.
    pub(crate) fn plan_tick(
        &mut self,
        now: Instant,
        tts: &mut dyn FnMut() -> Option<Bytes>,
        tts_active_recent: bool,
        queue_len_after: &dyn Fn() -> usize,
    ) -> Vec<SlotAction> {
        let elapsed_slots =
            (now.saturating_duration_since(self.anchor).as_nanos() / PUMP_FRAME.as_nanos()) as u64;
        let mut due = elapsed_slots.saturating_sub(self.written);
        let mut actions = Vec::new();
        if due == 0 {
            return actions;
        }
        let original_due = due;
        let budget = due.min(PUMP_MAX_FRAMES_PER_TICK);
        let tts_active = self.queue_was_non_empty || tts_active_recent;
        let mut attempted = 0;
        while attempted < budget && due > 0 {
            let at = self.anchor + PUMP_FRAME * (self.written + 1) as u32;
            if let Some(frame) = tts() {
                actions.push(SlotAction::Tts { frame, at });
                self.written += 1;
                self.consecutive_skips = 0;
            } else if tts_active && self.consecutive_skips < TTS_MAX_CONSECUTIVE_SKIPS {
                self.consecutive_skips += 1;
                self.written += 1;
                self.skipped += 1;
            } else {
                actions.push(SlotAction::Silence { at });
                self.written += 1;
                self.consecutive_skips = 0;
            }
            due -= 1;
            attempted += 1;
        }
        self.queue_was_non_empty = queue_len_after() > 0;
        if original_due > PUMP_MAX_FRAMES_PER_TICK {
            let back = PUMP_FRAME * self.written as u32;
            self.anchor = now.checked_sub(back).unwrap_or(now);
            self.rebases += 1;
        }
        actions
    }
}

/// Saturating sum of two stereo i16 LE PCM buffers.
fn sum_pcm(a: &[u8], b: &[u8]) -> Bytes {
    let len = a.len().min(b.len());
    let mut out = Vec::with_capacity(len);
    let mut i = 0;
    while i + 1 < len {
        let x = i16::from_le_bytes([a[i], a[i + 1]]) as i32;
        let y = i16::from_le_bytes([b[i], b[i + 1]]) as i32;
        let s = (x + y).clamp(-32_768, 32_767) as i16;
        out.extend_from_slice(&s.to_le_bytes());
        i += 2;
    }
    Bytes::from(out)
}

pub(crate) enum PumpCmd {
    Pause(oneshot::Sender<()>),
    Resume,
    Burst {
        frames: u32,
        paced: bool,
        done: oneshot::Sender<()>,
    },
    Stop,
}

/// Counters read by the 1 Hz JS stats poll.
#[derive(Default)]
pub(crate) struct PumpStats {
    pub(crate) frames_written: AtomicU64,
    pub(crate) slots_skipped: AtomicU64,
    pub(crate) rebases: AtomicU64,
    pub(crate) paused: AtomicBool,
}

/// Handle to one listener's pump task.
pub(crate) struct NativeMixPump {
    pub(crate) cmd_tx: mpsc::UnboundedSender<PumpCmd>,
    pub(crate) queue: Arc<NativeTtsQueue>,
    pub(crate) stats: Arc<PumpStats>,
}

struct PumpIo {
    graph: Arc<Mutex<MixGraph>>,
    listener: String,
    queue: Arc<NativeTtsQueue>,
    stats: Arc<PumpStats>,
    pc_outbound: Arc<LocalAudioTrack>,
    pc_tee: Arc<Mutex<Option<WriteSampleTee>>>,
}

impl PumpIo {
    async fn write_mix(&self, tts: Bytes, at: Instant) {
        let frame = {
            let mut g = self.graph.lock().unwrap_or_else(|e| e.into_inner());
            let mixed = g.render_output(&self.listener, at);
            let panned = g.pan_tts_frame(&Frame::new(tts, None), &self.listener);
            sum_pcm(panned.pcm.as_ref(), mixed.pcm.as_ref())
        };
        if let Err(err) = self
            .pc_outbound
            .write_sample(frame.clone(), PUMP_FRAME)
            .await
        {
            log::debug!(
                "native mix pump write failed listener={}: {err}",
                self.listener
            );
        }
        crate::media::notify_write_tee_handle(&self.pc_tee, frame.as_ref(), 20);
        self.stats.frames_written.fetch_add(1, Ordering::Relaxed);
    }
}

impl NativeMixPump {
    /// Spawns the actor task. Must be called inside a tokio runtime.
    pub(crate) fn spawn(
        graph: Arc<Mutex<MixGraph>>,
        listener: String,
        pc_outbound: Arc<LocalAudioTrack>,
        pc_tee: Arc<Mutex<Option<WriteSampleTee>>>,
        queue: Arc<NativeTtsQueue>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let stats = Arc::new(PumpStats::default());
        let io = PumpIo {
            graph,
            listener,
            queue: Arc::clone(&queue),
            stats: Arc::clone(&stats),
            pc_outbound,
            pc_tee,
        };
        tokio::spawn(run_pump(io, cmd_rx));
        Self {
            cmd_tx,
            queue,
            stats,
        }
    }
}

async fn run_pump(io: PumpIo, mut rx: mpsc::UnboundedReceiver<PumpCmd>) {
    let silence = Bytes::from(vec![0u8; FRAME_BYTES]);
    let mut sched = PumpScheduler::new(Instant::now());
    let mut paused = false;
    let mut seen_enqueue: Option<Instant> = None;
    loop {
        let tick = async {
            if paused {
                std::future::pending::<()>().await
            } else {
                tokio::time::sleep(PUMP_FRAME).await
            }
        };
        tokio::select! {
            cmd = rx.recv() => {
                match cmd {
                    None | Some(PumpCmd::Stop) => break,
                    Some(PumpCmd::Pause(ack)) => {
                        paused = true;
                        io.stats.paused.store(true, Ordering::Relaxed);
                        let _ = ack.send(());
                    }
                    Some(PumpCmd::Resume) => {
                        sched.resume(Instant::now());
                        paused = false;
                        io.stats.paused.store(false, Ordering::Relaxed);
                    }
                    Some(PumpCmd::Burst { frames, paced, done }) => {
                        for i in 0..frames {
                            let tts = io.queue.pop().unwrap_or_else(|| silence.clone());
                            io.write_mix(tts, Instant::now()).await;
                            if paced && i + 1 < frames {
                                tokio::time::sleep(PUMP_FRAME).await;
                            }
                        }
                        let _ = done.send(());
                    }
                }
            }
            _ = tick => {
                let now = Instant::now();
                let last = io.queue.last_enqueue();
                if last != seen_enqueue {
                    // A new TTS enqueue restarts the skip count (JS `enqueueTtsFrame`).
                    seen_enqueue = last;
                    sched.consecutive_skips = 0;
                }
                let recent = last.is_some_and(|t| now.saturating_duration_since(t) <= TTS_ACTIVE_WINDOW);
                let queue = Arc::clone(&io.queue);
                let queue_len = Arc::clone(&io.queue);
                let actions = sched.plan_tick(
                    now,
                    &mut || queue.pop(),
                    recent,
                    &|| queue_len.len(),
                );
                io.stats.slots_skipped.store(sched.skipped, Ordering::Relaxed);
                io.stats.rebases.store(sched.rebases, Ordering::Relaxed);
                for action in actions {
                    match action {
                        SlotAction::Tts { frame, at } => io.write_mix(frame, at).await,
                        SlotAction::Silence { at } => io.write_mix(silence.clone(), at).await,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(i: u16) -> Bytes {
        let mut v = vec![0u8; FRAME_BYTES];
        v[0..2].copy_from_slice(&i.to_le_bytes());
        Bytes::from(v)
    }

    fn frame_id(b: &Bytes) -> u16 {
        u16::from_le_bytes([b[0], b[1]])
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn plan(
        s: &mut PumpScheduler,
        now: Instant,
        q: &mut VecDeque<Bytes>,
        active: bool,
    ) -> Vec<SlotAction> {
        let cell = std::cell::RefCell::new(std::mem::take(q));
        let actions = s.plan_tick(now, &mut || cell.borrow_mut().pop_front(), active, &|| {
            cell.borrow().len()
        });
        *q = cell.into_inner();
        actions
    }

    #[test]
    fn due_zero_no_actions() {
        let base = Instant::now();
        let mut s = PumpScheduler::new(base);
        let mut q = VecDeque::from([frame(1)]);
        assert!(plan(&mut s, base + ms(10), &mut q, false).is_empty());
        assert_eq!(q.len(), 1);
        assert_eq!(s.written, 0);
    }

    #[test]
    fn due_one_with_queued_frame_emits_tts_at_slot_time() {
        let base = Instant::now();
        let mut s = PumpScheduler::new(base);
        let mut q = VecDeque::from([frame(7)]);
        let actions = plan(&mut s, base + ms(20), &mut q, false);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            SlotAction::Tts { frame, at } => {
                assert_eq!(frame_id(frame), 7);
                assert_eq!(*at, base + ms(20));
            }
            SlotAction::Silence { .. } => panic!("expected tts"),
        }
        assert_eq!(s.written, 1);
    }

    #[test]
    fn due_ten_emits_four_then_rebases() {
        let base = Instant::now();
        let mut s = PumpScheduler::new(base);
        let mut q = VecDeque::new();
        let now = base + ms(200);
        let actions = plan(&mut s, now, &mut q, false);
        assert_eq!(actions.len(), 4);
        assert_eq!(s.written, 4);
        assert_eq!(s.rebases, 1);
        assert_eq!(s.anchor, now - ms(80));
    }

    #[test]
    fn empty_queue_active_stream_skips_three_then_silence() {
        let base = Instant::now();
        let mut s = PumpScheduler::new(base);
        let mut q = VecDeque::new();
        let actions = plan(&mut s, base + ms(80), &mut q, true);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            SlotAction::Silence { at } => assert_eq!(*at, base + ms(80)),
            SlotAction::Tts { .. } => panic!("expected silence"),
        }
        assert_eq!(s.skipped, 3);
        assert_eq!(s.written, 4);
        assert_eq!(s.consecutive_skips, 0);
    }

    #[test]
    fn inactive_stream_writes_silence_immediately() {
        let base = Instant::now();
        let mut s = PumpScheduler::new(base);
        let mut q = VecDeque::new();
        let actions = plan(&mut s, base + ms(20), &mut q, false);
        assert!(matches!(actions.as_slice(), [SlotAction::Silence { .. }]));
        assert_eq!(s.skipped, 0);
    }

    #[test]
    fn queue_non_empty_last_tick_keeps_stream_active() {
        let base = Instant::now();
        let mut s = PumpScheduler::new(base);
        let mut q: VecDeque<Bytes> = (0..5).map(frame).collect();
        // Only one slot is due, so 4 frames stay queued: flag set for the next tick.
        let _ = plan(&mut s, base + ms(20), &mut q, false);
        assert!(s.queue_was_non_empty);
        q.clear();
        let actions = plan(&mut s, base + ms(40), &mut q, false);
        assert!(
            actions.is_empty(),
            "slot skipped while the stream is active"
        );
        assert_eq!(s.skipped, 1);
    }

    #[test]
    fn four_hundred_frames_all_come_out_once_in_order() {
        let base = Instant::now();
        let mut s = PumpScheduler::new(base);
        let mut q: VecDeque<Bytes> = (0..400u16).map(frame).collect();
        let mut out = Vec::new();
        let mut tick = 1u64;
        while out.len() < 400 && tick < 2_000 {
            let actions = plan(&mut s, base + ms(20 * tick), &mut q, true);
            assert!(actions.len() <= 4);
            for a in actions {
                if let SlotAction::Tts { frame, .. } = a {
                    out.push(frame_id(&frame));
                }
            }
            tick += 1;
        }
        assert_eq!(out, (0..400u16).collect::<Vec<_>>());
        assert!(q.is_empty());
    }

    #[test]
    fn tts_queue_carry_frames_pieces_in_order_and_discard_clears() {
        let q = NativeTtsQueue::new();
        let mut pcm = Vec::new();
        for i in 0..3u16 {
            let mut f = vec![0u8; FRAME_BYTES];
            f[0..2].copy_from_slice(&i.to_le_bytes());
            pcm.extend_from_slice(&f);
        }
        let mut popped = Vec::new();
        for piece in pcm.chunks(1000) {
            q.push_pcm(piece);
            while let Some(f) = q.pop() {
                assert_eq!(f.len(), FRAME_BYTES);
                popped.push(frame_id(&f));
            }
        }
        assert_eq!(popped, vec![0, 1, 2]);
        assert!(q.last_enqueue().is_some());

        q.push_pcm(&vec![1u8; FRAME_BYTES * 2 + 500]);
        assert_eq!(q.len(), 2);
        assert_eq!(q.take_max_depth(), 2);
        assert_eq!(q.discard(), 2);
        assert_eq!(q.len(), 0);
        // Carry was cleared: 3340 bytes alone do not complete a frame.
        q.push_pcm(&vec![1u8; FRAME_BYTES - 500]);
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn sum_pcm_saturates() {
        let a = 30_000i16.to_le_bytes();
        let b = 10_000i16.to_le_bytes();
        let out = sum_pcm(&a, &b);
        assert_eq!(i16::from_le_bytes([out[0], out[1]]), 32_767);
    }
}
