//! Wall-clock pacing for a mix outbound loop: frame n is due at `anchor + n × 20 ms`.

use std::time::{Duration, Instant};

use crate::frame::FRAME_MS;

/// Most frames an outbound loop writes back to back (80 ms). After a longer stall the
/// oldest due slots are not rendered: their ticks are beyond the mix history and the graph
/// counts them in `listener_skipped`. Lockstep: bindings `PumpScheduler::plan_tick`,
/// helpers `ClientAudioMixer.pumpMixFrame`.
pub const MAX_BURST_WRITES: u64 = 4;

/// Slots to render now, oldest first, and how many older slots were skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueSlots {
    /// Scheduled times of the slots to render.
    pub at: Vec<Instant>,
    /// Oldest due slots that were not rendered.
    pub skipped: u64,
}

/// Pure slot scheduler (no I/O): the caller sleeps until [`OutboundPacer::next_due`].
#[derive(Debug, Clone)]
pub struct OutboundPacer {
    anchor: Instant,
    written: u64,
    skipped_total: u64,
    reanchors: u64,
}

impl OutboundPacer {
    pub fn new(anchor: Instant) -> Self {
        Self {
            anchor,
            written: 0,
            skipped_total: 0,
            reanchors: 0,
        }
    }

    fn slot_at(&self, n: u64) -> Instant {
        self.anchor + Duration::from_millis(FRAME_MS as u64 * n)
    }

    /// Scheduled time of the next slot: `anchor + (written + 1) × FRAME_MS`.
    pub fn next_due(&self) -> Instant {
        self.slot_at(self.written + 1)
    }

    /// elapsed = floor((now − anchor) / FRAME_MS) (0 if now < anchor); due = elapsed − written.
    /// due > MAX_BURST_WRITES → skipped = due − MAX_BURST_WRITES, written += skipped,
    /// skipped_total += skipped, reanchors += 1. Returns the scheduled times of slots
    /// written+1..=elapsed (oldest first) and sets written = elapsed.
    pub fn due_slots(&mut self, now: Instant) -> DueSlots {
        let elapsed =
            (now.saturating_duration_since(self.anchor).as_millis() / FRAME_MS as u128) as u64;
        let due = elapsed.saturating_sub(self.written);
        let mut skipped = 0;
        if due > MAX_BURST_WRITES {
            skipped = due - MAX_BURST_WRITES;
            self.written += skipped;
            self.skipped_total += skipped;
            self.reanchors += 1;
        }
        let at = ((self.written + 1)..=elapsed)
            .map(|n| self.slot_at(n))
            .collect();
        self.written = self.written.max(elapsed);
        DueSlots { at, skipped }
    }

    pub fn written(&self) -> u64 {
        self.written
    }

    pub fn skipped_total(&self) -> u64 {
        self.skipped_total
    }

    pub fn reanchors(&self) -> u64 {
        self.reanchors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn on_time_slot_renders_once() {
        let anchor = Instant::now();
        let mut p = OutboundPacer::new(anchor);
        let d = p.due_slots(anchor + ms(20));
        assert_eq!(d.at, vec![anchor + ms(20)]);
        assert_eq!(d.skipped, 0);
        assert!(p.due_slots(anchor + ms(39)).at.is_empty());
    }

    #[test]
    fn jitter_of_three_slots_renders_each_at_its_scheduled_time() {
        let anchor = Instant::now();
        let mut p = OutboundPacer::new(anchor);
        let d = p.due_slots(anchor + ms(60));
        assert_eq!(
            d.at,
            vec![anchor + ms(20), anchor + ms(40), anchor + ms(60)]
        );
        assert_eq!(d.skipped, 0);
        assert_eq!(p.reanchors(), 0);
    }

    #[test]
    fn one_second_stall_renders_only_the_newest_four() {
        let anchor = Instant::now();
        let mut p = OutboundPacer::new(anchor);
        let d = p.due_slots(anchor + ms(1000));
        assert_eq!(
            d.at,
            vec![
                anchor + ms(940),
                anchor + ms(960),
                anchor + ms(980),
                anchor + ms(1000)
            ]
        );
        assert_eq!(d.skipped, 46);
        assert_eq!(p.written(), 50);
        assert_eq!(p.skipped_total(), 46);
        assert_eq!(p.reanchors(), 1);
        assert_eq!(p.next_due(), anchor + ms(1020));
        assert_eq!(p.due_slots(anchor + ms(1020)).at, vec![anchor + ms(1020)]);
    }

    #[test]
    fn exactly_four_due_is_not_a_stall() {
        let anchor = Instant::now();
        let mut p = OutboundPacer::new(anchor);
        let d = p.due_slots(anchor + ms(80));
        assert_eq!(d.at.len(), 4);
        assert_eq!(d.skipped, 0);
        assert_eq!(p.reanchors(), 0);
    }

    #[test]
    fn now_before_anchor_is_empty() {
        let anchor = Instant::now() + ms(500);
        let mut p = OutboundPacer::new(anchor);
        let d = p.due_slots(anchor - ms(100));
        assert!(d.at.is_empty());
        assert_eq!(d.skipped, 0);
        assert_eq!(p.written(), 0);
    }
}
