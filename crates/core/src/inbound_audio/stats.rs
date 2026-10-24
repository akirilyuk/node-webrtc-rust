//! Counters of one inbound audio stage (one per remote track).

use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counters shared between the reorder buffer, concealment and stats readers.
#[derive(Debug, Default)]
pub struct InboundAudioStageStats {
    pub packets_received: AtomicU64,
    pub packets_discarded: AtomicU64,
    pub reordered: AtomicU64,
    pub lost: AtomicU64,
    pub late_after_loss: AtomicU64,
    pub concealed_samples: AtomicU64,
    pub concealment_events: AtomicU64,
    pub fec_decodes: AtomicU64,
    pub plc_decodes: AtomicU64,
    pub lost_not_concealed: AtomicU64,
    pub resets: AtomicU64,
    pub current_wait_ms: AtomicU64,
    /// Packets (received or concealed) that produced no audio because the decoder failed.
    pub undecodable: AtomicU64,
}

/// Plain copy of [`InboundAudioStageStats`] at one instant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InboundAudioStatsSnapshot {
    pub packets_received: u64,
    pub packets_discarded: u64,
    pub reordered: u64,
    pub lost: u64,
    pub late_after_loss: u64,
    pub concealed_samples: u64,
    pub concealment_events: u64,
    pub fec_decodes: u64,
    pub plc_decodes: u64,
    pub lost_not_concealed: u64,
    pub resets: u64,
    pub current_wait_ms: u64,
    pub undecodable: u64,
}

impl InboundAudioStageStats {
    pub(crate) fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> InboundAudioStatsSnapshot {
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        InboundAudioStatsSnapshot {
            packets_received: g(&self.packets_received),
            packets_discarded: g(&self.packets_discarded),
            reordered: g(&self.reordered),
            lost: g(&self.lost),
            late_after_loss: g(&self.late_after_loss),
            concealed_samples: g(&self.concealed_samples),
            concealment_events: g(&self.concealment_events),
            fec_decodes: g(&self.fec_decodes),
            plc_decodes: g(&self.plc_decodes),
            lost_not_concealed: g(&self.lost_not_concealed),
            resets: g(&self.resets),
            current_wait_ms: g(&self.current_wait_ms),
            undecodable: g(&self.undecodable),
        }
    }
}
