//! Pure RTP reorder buffer with an adaptive, bounded wait and an injected clock.
//!
//! No tokio, no decoder. Callers `push` arriving packets and `poll` for ordered output; a
//! gap is declared lost only after the adaptive wait (20..=60 ms) has passed or more than
//! 50 packets are held. A received packet is never dropped for capacity.

use super::stats::InboundAudioStageStats;
use crate::media::RtpPacket;
use bytes::Bytes;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MIN_WAIT: Duration = Duration::from_millis(20);
const MAX_WAIT: Duration = Duration::from_millis(60);
const WAIT_MARGIN: Duration = Duration::from_millis(5);
const OBSERVATION_WINDOW: Duration = Duration::from_secs(10);
const MAX_OBSERVATIONS: usize = 64;
const RESET_JUMP: i64 = 1_000;
const MAX_HELD: usize = 50;
const MAX_LOST_RANGES: usize = 64;

/// Output of the reorder buffer, in sequence order.
#[derive(Debug, Clone)]
pub enum StageOut {
    /// The next packet in sequence.
    Packet(RtpPacket),
    /// `count` packets were declared lost. `next` is the payload of the packet right after
    /// the gap (it is also emitted as the following `Packet`).
    Lost { count: u64, next: Option<Bytes> },
}

struct Held {
    packet: RtpPacket,
    arrived: Instant,
}

struct LostRange {
    start: i64,
    end: i64,
    gap_since: Instant,
}

pub struct InboundReorderBuffer {
    stats: Arc<InboundAudioStageStats>,
    /// Next extended sequence number to emit; `None` before the first packet.
    next: Option<i64>,
    /// Highest extended sequence number seen (reference for unwrapping).
    max_ext: i64,
    held: BTreeMap<i64, Held>,
    ready: VecDeque<StageOut>,
    gap_since: Option<Instant>,
    lost_ranges: VecDeque<LostRange>,
    observations: VecDeque<(Instant, Duration)>,
    wait: Duration,
}

impl InboundReorderBuffer {
    pub fn new(stats: Arc<InboundAudioStageStats>) -> Self {
        stats
            .current_wait_ms
            .store(MIN_WAIT.as_millis() as u64, Ordering::Relaxed);
        Self {
            stats,
            next: None,
            max_ext: 0,
            held: BTreeMap::new(),
            ready: VecDeque::new(),
            gap_since: None,
            lost_ranges: VecDeque::new(),
            observations: VecDeque::new(),
            wait: MIN_WAIT,
        }
    }

    /// Current adaptive wait.
    pub fn wait(&self) -> Duration {
        self.wait
    }

    /// When the head gap will be declared lost; `None` when nothing is held.
    pub fn next_deadline(&self) -> Option<Instant> {
        if self.held.is_empty() {
            return None;
        }
        self.gap_since.map(|t| t + self.wait)
    }

    pub fn push(&mut self, packet: RtpPacket, now: Instant) {
        InboundAudioStageStats::add(&self.stats.packets_received, 1);
        self.refresh_wait(now);
        let seq = packet.sequence_number;
        let Some(next) = self.next else {
            let ext = i64::from(seq);
            self.next = Some(ext + 1);
            self.max_ext = ext;
            self.ready.push_back(StageOut::Packet(packet));
            return;
        };

        let diff = i64::from(seq.wrapping_sub(self.max_ext as u16) as i16);
        if diff.abs() > RESET_JUMP {
            self.reset(packet);
            return;
        }
        let ext = self.max_ext + diff;

        if ext < next {
            // Duplicate, or too late: its gap was already declared lost (or it was emitted).
            InboundAudioStageStats::add(&self.stats.packets_discarded, 1);
            if let Some(r) = self
                .lost_ranges
                .iter()
                .find(|r| ext >= r.start && ext < r.end)
            {
                let lateness = now.saturating_duration_since(r.gap_since);
                InboundAudioStageStats::add(&self.stats.late_after_loss, 1);
                self.observe(now, lateness);
            }
            return;
        }
        if self.held.contains_key(&ext) {
            InboundAudioStageStats::add(&self.stats.packets_discarded, 1);
            return;
        }

        if ext < self.max_ext {
            InboundAudioStageStats::add(&self.stats.reordered, 1);
        }
        if ext > self.max_ext {
            self.max_ext = ext;
        }
        if ext == next && self.held.is_empty() {
            self.next = Some(next + 1);
            self.ready.push_back(StageOut::Packet(packet));
            return;
        }
        if ext == next {
            // Fills the head gap: its delay is what the adaptive wait must cover.
            if let Some(since) = self.gap_since {
                self.observe(now, now.saturating_duration_since(since));
            }
        }
        self.held.insert(
            ext,
            Held {
                packet,
                arrived: now,
            },
        );
        self.advance(now);
    }

    /// Ordered output that is ready at `now`.
    pub fn poll(&mut self, now: Instant) -> Vec<StageOut> {
        self.refresh_wait(now);
        self.advance(now);
        self.ready.drain(..).collect()
    }

    /// End of stream: everything held, in order, gaps as `Lost`.
    pub fn flush(&mut self) -> Vec<StageOut> {
        self.flush_held();
        self.ready.drain(..).collect()
    }

    fn reset(&mut self, packet: RtpPacket) {
        InboundAudioStageStats::add(&self.stats.resets, 1);
        self.flush_held();
        self.lost_ranges.clear();
        self.gap_since = None;
        let ext = i64::from(packet.sequence_number);
        self.next = Some(ext + 1);
        self.max_ext = ext;
        self.ready.push_back(StageOut::Packet(packet));
    }

    fn flush_held(&mut self) {
        while let Some((&first, _)) = self.held.first_key_value() {
            let next = self.next.unwrap_or(first);
            if first != next {
                self.declare_lost(next, first, Instant::now());
            }
            self.emit_held_head();
        }
        self.gap_since = None;
    }

    fn emit_held_head(&mut self) {
        if let Some((ext, h)) = self.held.pop_first() {
            self.next = Some(ext + 1);
            self.ready.push_back(StageOut::Packet(h.packet));
        }
    }

    fn declare_lost(&mut self, start: i64, end: i64, gap_since_fallback: Instant) {
        let count = (end - start) as u64;
        let next = self.held.get(&end).map(|h| h.packet.payload.clone());
        InboundAudioStageStats::add(&self.stats.lost, count);
        self.lost_ranges.push_back(LostRange {
            start,
            end,
            gap_since: self.gap_since.unwrap_or(gap_since_fallback),
        });
        while self.lost_ranges.len() > MAX_LOST_RANGES {
            self.lost_ranges.pop_front();
        }
        self.ready.push_back(StageOut::Lost { count, next });
        self.next = Some(end);
    }

    fn advance(&mut self, now: Instant) {
        loop {
            let Some((&first, head)) = self.held.first_key_value() else {
                self.gap_since = None;
                return;
            };
            let next = self.next.expect("held implies a first packet");
            if first == next {
                self.emit_held_head();
                self.gap_since = None;
                continue;
            }
            let since = *self.gap_since.get_or_insert(head.arrived);
            if now >= since + self.wait || self.held.len() > MAX_HELD {
                self.declare_lost(next, first, now);
                self.gap_since = None;
                continue;
            }
            return;
        }
    }

    fn observe(&mut self, now: Instant, delay: Duration) {
        self.observations.push_back((now, delay));
        while self.observations.len() > MAX_OBSERVATIONS {
            self.observations.pop_front();
        }
        self.refresh_wait(now);
    }

    fn refresh_wait(&mut self, now: Instant) {
        while let Some(&(t, _)) = self.observations.front() {
            if now.saturating_duration_since(t) >= OBSERVATION_WINDOW {
                self.observations.pop_front();
            } else {
                break;
            }
        }
        let max = self
            .observations
            .iter()
            .map(|&(_, d)| d)
            .max()
            .unwrap_or_default();
        self.wait = (max + WAIT_MARGIN).clamp(MIN_WAIT, MAX_WAIT);
        self.stats
            .current_wait_ms
            .store(self.wait.as_millis() as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::super::impair::{Impairment, ImpairmentConfig};
    use super::*;
    use std::collections::BTreeSet;

    fn pkt(seq: u16) -> RtpPacket {
        RtpPacket {
            payload: Bytes::from(vec![(seq & 0xFF) as u8]),
            sequence_number: seq,
            timestamp: u32::from(seq).wrapping_mul(960),
            payload_type: 111,
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn new_buf() -> (InboundReorderBuffer, Arc<InboundAudioStageStats>) {
        let stats = Arc::new(InboundAudioStageStats::default());
        (InboundReorderBuffer::new(stats.clone()), stats)
    }

    fn seqs(out: &[StageOut]) -> Vec<u16> {
        out.iter()
            .filter_map(|o| match o {
                StageOut::Packet(p) => Some(p.sequence_number),
                StageOut::Lost { .. } => None,
            })
            .collect()
    }

    #[test]
    fn in_order_stream_is_ready_on_push_with_no_deadline() {
        let (mut b, stats) = new_buf();
        let t0 = Instant::now();
        for i in 0..500u16 {
            let now = t0 + ms(u64::from(i) * 20);
            b.push(pkt(i), now);
            assert!(b.next_deadline().is_none());
            let out = b.poll(now);
            assert_eq!(seqs(&out), vec![i]);
            assert_eq!(out.len(), 1);
        }
        let s = stats.snapshot();
        assert_eq!(s.packets_received, 500);
        assert_eq!(s.reordered, 0);
        assert_eq!(s.lost, 0);
    }

    #[test]
    fn swapped_pair_is_reordered_without_loss() {
        let (mut b, stats) = new_buf();
        let t0 = Instant::now();
        b.push(pkt(0), t0);
        b.push(pkt(1), t0 + ms(20));
        b.push(pkt(3), t0 + ms(40));
        assert_eq!(seqs(&b.poll(t0 + ms(40))), vec![0, 1]);
        b.push(pkt(2), t0 + ms(50));
        let out = b.poll(t0 + ms(50));
        assert_eq!(seqs(&out), vec![2, 3]);
        assert!(out.iter().all(|o| matches!(o, StageOut::Packet(_))));
        let s = stats.snapshot();
        assert_eq!(s.reordered, 1);
        assert_eq!(s.lost, 0);
        assert!(b.next_deadline().is_none());
    }

    #[test]
    fn single_loss_is_declared_exactly_at_the_wait() {
        let (mut b, stats) = new_buf();
        let t0 = Instant::now();
        b.push(pkt(0), t0);
        b.poll(t0);
        let gap = t0 + ms(40);
        b.push(pkt(2), gap);
        assert!(b.poll(gap).is_empty());
        assert_eq!(b.next_deadline(), Some(gap + ms(20)));
        assert!(b.poll(gap + ms(19)).is_empty());
        let out = b.poll(gap + ms(20));
        assert!(matches!(
            out[0],
            StageOut::Lost {
                count: 1,
                next: Some(_)
            }
        ));
        assert_eq!(seqs(&out), vec![2]);
        assert_eq!(stats.snapshot().lost, 1);
    }

    #[test]
    fn burst_loss_of_three_is_one_lost_with_next() {
        let (mut b, _) = new_buf();
        let t0 = Instant::now();
        b.push(pkt(0), t0);
        b.poll(t0);
        b.push(pkt(4), t0 + ms(80));
        let out = b.poll(t0 + ms(100));
        match &out[0] {
            StageOut::Lost { count, next } => {
                assert_eq!(*count, 3);
                assert_eq!(next.as_deref(), Some(&[4u8][..]));
            }
            other => panic!("expected Lost, got {other:?}"),
        }
        assert_eq!(seqs(&out), vec![4]);
    }

    #[test]
    fn ten_packet_loss_is_one_lost_of_ten() {
        let (mut b, stats) = new_buf();
        let t0 = Instant::now();
        b.push(pkt(0), t0);
        b.poll(t0);
        b.push(pkt(11), t0 + ms(200));
        let out = b.poll(t0 + ms(220));
        assert!(matches!(out[0], StageOut::Lost { count: 10, .. }));
        assert_eq!(stats.snapshot().lost, 10);
    }

    #[test]
    fn duplicates_are_discarded_and_never_emitted_twice() {
        let (mut b, stats) = new_buf();
        let t0 = Instant::now();
        let mut emitted = Vec::new();
        let mut dups = 0u64;
        for i in 0..300u16 {
            let now = t0 + ms(u64::from(i) * 20);
            b.push(pkt(i), now);
            if i % 100 == 7 {
                b.push(pkt(i), now + ms(1));
                dups += 1;
            }
            emitted.extend(seqs(&b.poll(now + ms(1))));
        }
        assert_eq!(emitted, (0..300).collect::<Vec<u16>>());
        let s = stats.snapshot();
        assert_eq!(s.packets_discarded, dups);
        assert_eq!(s.packets_received, 300 + dups);
    }

    #[test]
    fn late_after_loss_grows_the_wait_and_it_decays() {
        let (mut b, stats) = new_buf();
        let t0 = Instant::now();
        b.push(pkt(0), t0);
        b.poll(t0);
        let gap = t0 + ms(40);
        b.push(pkt(2), gap);
        let out = b.poll(gap + ms(20));
        assert!(matches!(out[0], StageOut::Lost { count: 1, .. }));
        // The missing packet shows up 35 ms after the gap was first seen.
        b.push(pkt(1), gap + ms(35));
        assert!(b.poll(gap + ms(35)).is_empty());
        let s = stats.snapshot();
        assert_eq!(s.late_after_loss, 1);
        assert_eq!(s.packets_discarded, 1);
        assert_eq!(s.current_wait_ms, 40);
        assert_eq!(b.wait(), ms(40));
        // 10 s later the observation has expired.
        b.poll(gap + ms(35) + Duration::from_secs(10));
        assert_eq!(stats.snapshot().current_wait_ms, 20);
    }

    #[test]
    fn wait_never_exceeds_sixty_ms() {
        let (mut b, stats) = new_buf();
        let t0 = Instant::now();
        b.push(pkt(0), t0);
        b.poll(t0);
        b.push(pkt(2), t0 + ms(20));
        b.poll(t0 + ms(40));
        b.push(pkt(1), t0 + ms(220));
        assert_eq!(stats.snapshot().current_wait_ms, 60);
        assert_eq!(b.wait(), ms(60));
    }

    #[test]
    fn u16_wraparound_with_reorder_is_ordered() {
        let (mut b, stats) = new_buf();
        let t0 = Instant::now();
        let order: [u16; 8] = [65_530, 65_531, 65_532, 65_534, 65_533, 0, 65_535, 1];
        // 0 arrives before 65_535 (swap across the wrap).
        let mut emitted = Vec::new();
        for (i, s) in order.iter().enumerate() {
            let now = t0 + ms(i as u64 * 5);
            b.push(pkt(*s), now);
            emitted.extend(seqs(&b.poll(now)));
        }
        assert_eq!(
            emitted,
            vec![65_530, 65_531, 65_532, 65_533, 65_534, 65_535, 0, 1]
        );
        assert_eq!(stats.snapshot().lost, 0);
    }

    #[test]
    fn stream_reset_flushes_and_rebases() {
        let (mut b, stats) = new_buf();
        let t0 = Instant::now();
        b.push(pkt(0), t0);
        b.push(pkt(1), t0);
        b.push(pkt(3), t0 + ms(5));
        let mut out = b.poll(t0 + ms(5));
        b.push(pkt(5_003), t0 + ms(10));
        out.extend(b.poll(t0 + ms(10)));
        // 0, 1, [lost 2], 3 flushed, then the new stream.
        assert_eq!(seqs(&out), vec![0, 1, 3, 5_003]);
        assert!(out
            .iter()
            .any(|o| matches!(o, StageOut::Lost { count: 1, .. })));
        assert_eq!(stats.snapshot().resets, 1);
        b.push(pkt(5_004), t0 + ms(30));
        assert_eq!(seqs(&b.poll(t0 + ms(30))), vec![5_004]);
    }

    #[test]
    fn capacity_declares_the_gap_lost_and_drops_no_received_packet() {
        let (mut b, stats) = new_buf();
        let t0 = Instant::now(); // frozen clock
        b.push(pkt(0), t0);
        assert_eq!(seqs(&b.poll(t0)), vec![0]);
        // seq 1 is missing; hold 2..=51 (50 packets): still waiting.
        for s in 2..=51u16 {
            b.push(pkt(s), t0);
        }
        assert!(b.poll(t0).is_empty());
        // The 51st held packet exceeds capacity.
        b.push(pkt(52), t0);
        let mut emitted = Vec::new();
        let out = b.poll(t0);
        assert!(matches!(out[0], StageOut::Lost { count: 1, .. }));
        emitted.extend(seqs(&out));
        for s in 53..=61u16 {
            b.push(pkt(s), t0);
            emitted.extend(seqs(&b.poll(t0)));
        }
        assert_eq!(emitted, (2..=61).collect::<Vec<u16>>());
        let s = stats.snapshot();
        assert_eq!(s.lost, 1);
        assert_eq!(s.packets_discarded, 0);
    }

    #[test]
    fn unfilled_gap_never_waits_longer_than_sixty_ms() {
        let (mut b, _) = new_buf();
        let t0 = Instant::now();
        b.push(pkt(0), t0);
        b.poll(t0);
        b.push(pkt(2), t0 + ms(10));
        let deadline = b.next_deadline().unwrap();
        assert!(deadline <= t0 + ms(10) + ms(60));
        assert!(!b.poll(deadline).is_empty());
    }

    #[test]
    fn flush_emits_held_packets_in_order_with_lost_for_gaps() {
        let (mut b, _) = new_buf();
        let t0 = Instant::now();
        b.push(pkt(0), t0);
        b.push(pkt(2), t0);
        b.push(pkt(5), t0);
        let out = b.flush();
        assert_eq!(seqs(&out), vec![0, 2, 5]);
        assert!(matches!(out[1], StageOut::Lost { count: 1, .. }));
        assert!(matches!(out[3], StageOut::Lost { count: 2, .. }));
        assert!(b.next_deadline().is_none());
    }

    fn run_impaired(cfg: &str) {
        let (mut b, stats) = new_buf();
        let mut imp = Impairment::new(ImpairmentConfig::parse(cfg).unwrap());
        let t0 = Instant::now();
        let start: u16 = 64_900; // crosses the u16 wrap
        let total = 3_000u32;
        let mut received = 0u64;
        let mut received_set = BTreeSet::new();
        let mut cursor: Option<u16> = None; // next seq the output must account for (set by the first packet)
        let mut emitted = BTreeSet::new();
        let mut emitted_total = 0u64;
        let mut check = |out: Vec<StageOut>, cursor: &mut Option<u16>| {
            for o in out {
                match o {
                    StageOut::Packet(p) => {
                        let want = cursor.unwrap_or(p.sequence_number);
                        assert_eq!(p.sequence_number, want, "output must tile seq space");
                        *cursor = Some(want.wrapping_add(1));
                        assert!(emitted.insert(p.sequence_number), "emitted twice");
                        emitted_total += 1;
                    }
                    StageOut::Lost { count, .. } => {
                        let c = cursor.expect("Lost cannot precede the first packet");
                        *cursor = Some(c.wrapping_add(count as u16));
                    }
                }
            }
        };
        for i in 0..total {
            let now = t0 + ms(u64::from(i) * 20);
            let seq = start.wrapping_add(i as u16);
            for p in imp.apply(pkt(seq)) {
                received += 1;
                received_set.insert(p.sequence_number);
                b.push(p, now);
            }
            // Poll at the buffer's own deadline when it falls before the next tick.
            if let Some(d) = b.next_deadline() {
                if d < now + ms(20) {
                    check(b.poll(d), &mut cursor);
                }
            }
            check(b.poll(now), &mut cursor);
        }
        let tail_now = t0 + ms(u64::from(total) * 20);
        for p in imp.flush() {
            received += 1;
            received_set.insert(p.sequence_number);
            b.push(p, tail_now);
        }
        check(b.flush(), &mut cursor);
        let s = stats.snapshot();
        assert_eq!(s.packets_received, received);
        assert_eq!(received, emitted_total + s.packets_discarded);
        // Every unique received seq was either emitted or discarded as late.
        assert!(emitted.is_subset(&received_set));
        assert!(s.lost > 0, "impairment must create loss");
        assert!(s.reordered > 0 || cfg.contains("reorder=0.0,"));
    }

    #[test]
    fn seeded_impairment_two_percent_keeps_every_invariant() {
        run_impaired(
            "loss=0.02,burst=3,burst_rate=0.01,reorder=0.03,reorder_depth=2,dup=0.02,seed=42",
        );
    }

    #[test]
    fn seeded_impairment_five_percent_keeps_every_invariant() {
        run_impaired(
            "loss=0.05,burst=3,burst_rate=0.01,reorder=0.03,reorder_depth=2,dup=0.02,seed=1234",
        );
    }
}
