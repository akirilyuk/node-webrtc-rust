//! Counters of one inbound audio stage (one per remote track).

use std::collections::HashMap;
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

/// Adds the stage counters to every `inbound-rtp` entry of a serialized `getStats` report
/// whose `trackIdentifier` has a stage. W3C names: `packetsDiscarded`, `concealedSamples`,
/// `concealmentEvents`; nwr-specific fields are prefixed `nwr`.
pub(crate) fn merge_inbound_stage_stats(
    mut report: serde_json::Value,
    stats: &HashMap<String, InboundAudioStatsSnapshot>,
) -> serde_json::Value {
    let Some(entries) = report.as_object_mut() else {
        return report;
    };
    for entry in entries.values_mut() {
        let Some(obj) = entry.as_object_mut() else {
            continue;
        };
        if obj.get("type").and_then(|t| t.as_str()) != Some("inbound-rtp") {
            continue;
        }
        let Some(snap) = obj
            .get("trackIdentifier")
            .and_then(|t| t.as_str())
            .and_then(|id| stats.get(id))
        else {
            continue;
        };
        for (key, value) in [
            ("packetsDiscarded", snap.packets_discarded),
            ("concealedSamples", snap.concealed_samples),
            ("concealmentEvents", snap.concealment_events),
            ("nwrReorderedPackets", snap.reordered),
            ("nwrLostPackets", snap.lost),
            ("nwrLateAfterLossPackets", snap.late_after_loss),
            ("nwrFecDecodes", snap.fec_decodes),
            ("nwrPlcDecodes", snap.plc_decodes),
            ("nwrLostNotConcealed", snap.lost_not_concealed),
            ("nwrUndecodablePackets", snap.undecodable),
            ("nwrReorderWaitMs", snap.current_wait_ms),
        ] {
            obj.insert(key.to_owned(), serde_json::Value::from(value));
        }
    }
    report
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    use serde_json::json;

    fn snap() -> InboundAudioStatsSnapshot {
        InboundAudioStatsSnapshot {
            packets_received: 100,
            packets_discarded: 2,
            reordered: 3,
            lost: 4,
            late_after_loss: 5,
            concealed_samples: 6,
            concealment_events: 7,
            fec_decodes: 8,
            plc_decodes: 9,
            lost_not_concealed: 10,
            resets: 11,
            current_wait_ms: 40,
            undecodable: 12,
        }
    }

    #[test]
    fn matching_inbound_entry_gets_every_field() {
        let report = json!({"a": {"type": "inbound-rtp", "trackIdentifier": "t1"}});
        let map = HashMap::from([("t1".to_owned(), snap())]);
        let out = merge_inbound_stage_stats(report, &map);
        let e = &out["a"];
        assert_eq!(e["packetsDiscarded"], 2);
        assert_eq!(e["concealedSamples"], 6);
        assert_eq!(e["concealmentEvents"], 7);
        assert_eq!(e["nwrReorderedPackets"], 3);
        assert_eq!(e["nwrLostPackets"], 4);
        assert_eq!(e["nwrLateAfterLossPackets"], 5);
        assert_eq!(e["nwrFecDecodes"], 8);
        assert_eq!(e["nwrPlcDecodes"], 9);
        assert_eq!(e["nwrLostNotConcealed"], 10);
        assert_eq!(e["nwrUndecodablePackets"], 12);
        assert_eq!(e["nwrReorderWaitMs"], 40);
    }

    #[test]
    fn other_tracks_and_other_types_are_untouched() {
        let report = json!({
            "a": {"type": "inbound-rtp", "trackIdentifier": "other"},
            "b": {"type": "outbound-rtp", "trackIdentifier": "t1"},
            "c": {"type": "transport"}
        });
        let map = HashMap::from([("t1".to_owned(), snap())]);
        let out = merge_inbound_stage_stats(report.clone(), &map);
        assert_eq!(out, report);
    }
}
