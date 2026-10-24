//! Seeded, deterministic RTP impairment for measurement (loss, bursts, reorder, duplicates).
//!
//! Packet-count based: no clock, no threads. The same seed and input give the same output.

use crate::media::RtpPacket;

/// Parsed `NWR_TEST_RTP_IMPAIRMENT` value.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpairmentConfig {
    /// Probability of dropping a packet.
    pub loss: f64,
    /// Length of a loss burst (packets dropped in a row, including the first).
    pub burst: u32,
    /// Probability that a packet starts a burst.
    pub burst_rate: f64,
    /// Probability of holding a packet back.
    pub reorder: f64,
    /// A held packet is released after this many later packets.
    pub reorder_depth: u32,
    /// Probability of duplicating a delivered packet.
    pub dup: f64,
    pub seed: u64,
}

impl Default for ImpairmentConfig {
    fn default() -> Self {
        Self {
            loss: 0.0,
            burst: 3,
            burst_rate: 0.0,
            reorder: 0.0,
            reorder_depth: 2,
            dup: 0.0,
            seed: 1,
        }
    }
}

impl ImpairmentConfig {
    /// Parses `loss=0.05,burst=3,burst_rate=0.01,reorder=0.02,reorder_depth=2,dup=0.01,seed=42`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut cfg = Self::default();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (key, value) = part
                .split_once('=')
                .ok_or_else(|| format!("impairment entry '{part}' is not key=value"))?;
            let (key, value) = (key.trim(), value.trim());
            let prob = |key: &str| -> Result<f64, String> {
                let v: f64 = value
                    .parse()
                    .map_err(|_| format!("impairment '{key}': invalid number '{value}'"))?;
                if (0.0..=1.0).contains(&v) {
                    Ok(v)
                } else {
                    Err(format!("impairment '{key}': {v} is outside 0..=1"))
                }
            };
            let count = |key: &str| -> Result<u32, String> {
                let v: u32 = value
                    .parse()
                    .map_err(|_| format!("impairment '{key}': invalid integer '{value}'"))?;
                if v == 0 {
                    Err(format!("impairment '{key}': must be at least 1"))
                } else {
                    Ok(v)
                }
            };
            match key {
                "loss" => cfg.loss = prob("loss")?,
                "burst" => cfg.burst = count("burst")?,
                "burst_rate" => cfg.burst_rate = prob("burst_rate")?,
                "reorder" => cfg.reorder = prob("reorder")?,
                "reorder_depth" => cfg.reorder_depth = count("reorder_depth")?,
                "dup" => cfg.dup = prob("dup")?,
                "seed" => {
                    cfg.seed = value
                        .parse()
                        .map_err(|_| format!("impairment 'seed': invalid integer '{value}'"))?;
                }
                other => return Err(format!("impairment: unknown key '{other}'")),
            }
        }
        Ok(cfg)
    }
}

/// Applies an [`ImpairmentConfig`] to a packet stream.
pub struct Impairment {
    cfg: ImpairmentConfig,
    state: u64,
    burst_left: u32,
    held: Vec<(u32, RtpPacket)>,
}

impl Impairment {
    pub fn new(cfg: ImpairmentConfig) -> Self {
        let state = if cfg.seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            cfg.seed
        };
        Self {
            cfg,
            state,
            burst_left: 0,
            held: Vec::new(),
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Feeds one packet; returns the packets that arrive now (0..n), in arrival order.
    pub fn apply(&mut self, packet: RtpPacket) -> Vec<RtpPacket> {
        let mut out = Vec::new();
        let mut release = Vec::new();
        self.held.retain_mut(|(left, p)| {
            *left -= 1;
            if *left == 0 {
                release.push(p.clone());
                false
            } else {
                true
            }
        });

        if self.burst_left > 0 {
            self.burst_left -= 1;
        } else if self.next_f64() < self.cfg.burst_rate {
            self.burst_left = self.cfg.burst - 1;
        } else if self.next_f64() < self.cfg.loss {
            // dropped
        } else if self.next_f64() < self.cfg.reorder {
            self.held.push((self.cfg.reorder_depth, packet));
        } else {
            let dup = self.next_f64() < self.cfg.dup;
            if dup {
                out.push(packet.clone());
            }
            out.push(packet);
        }
        out.extend(release);
        out
    }

    /// Packets still held back (end of stream).
    pub fn flush(&mut self) -> Vec<RtpPacket> {
        self.held.drain(..).map(|(_, p)| p).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn pkt(seq: u16) -> RtpPacket {
        RtpPacket {
            payload: Bytes::from_static(&[1]),
            sequence_number: seq,
            timestamp: u32::from(seq) * 960,
            payload_type: 111,
        }
    }

    fn run(cfg: &str, n: u16) -> Vec<u16> {
        let mut imp = Impairment::new(ImpairmentConfig::parse(cfg).unwrap());
        let mut out = Vec::new();
        for s in 0..n {
            out.extend(imp.apply(pkt(s)).into_iter().map(|p| p.sequence_number));
        }
        out.extend(imp.flush().into_iter().map(|p| p.sequence_number));
        out
    }

    #[test]
    fn parses_valid_string_and_rejects_bad_input() {
        let c = ImpairmentConfig::parse(
            "loss=0.05,burst=3,burst_rate=0.01,reorder=0.02,reorder_depth=2,dup=0.01,seed=42",
        )
        .unwrap();
        assert_eq!(c.loss, 0.05);
        assert_eq!(c.burst, 3);
        assert_eq!(c.burst_rate, 0.01);
        assert_eq!(c.reorder, 0.02);
        assert_eq!(c.reorder_depth, 2);
        assert_eq!(c.dup, 0.01);
        assert_eq!(c.seed, 42);
        let e = ImpairmentConfig::parse("loss=0.1,bogus=1").unwrap_err();
        assert!(e.contains("bogus"), "{e}");
        let e = ImpairmentConfig::parse("loss=abc").unwrap_err();
        assert!(e.contains("loss"), "{e}");
        let e = ImpairmentConfig::parse("dup=1.5").unwrap_err();
        assert!(e.contains("dup"), "{e}");
        let e = ImpairmentConfig::parse("burst=0").unwrap_err();
        assert!(e.contains("burst"), "{e}");
        assert!(ImpairmentConfig::parse("loss").is_err());
        assert_eq!(
            ImpairmentConfig::parse("").unwrap(),
            ImpairmentConfig::default()
        );
    }

    #[test]
    fn same_seed_same_pattern_different_seed_differs() {
        let cfg = "loss=0.05,burst_rate=0.01,reorder=0.03,dup=0.02,seed=7";
        let a = run(cfg, 2_000);
        assert_eq!(a, run(cfg, 2_000));
        assert_ne!(
            a,
            run(
                "loss=0.05,burst_rate=0.01,reorder=0.03,dup=0.02,seed=8",
                2_000
            )
        );
        assert!(
            a.len() < 2_000 + 100,
            "some packets must be dropped overall"
        );
        let unique: std::collections::BTreeSet<_> = a.iter().collect();
        assert!(unique.len() < 2_000, "loss must remove packets");
    }

    #[test]
    fn no_impairment_is_identity() {
        let out = run("", 100);
        assert_eq!(out, (0..100).collect::<Vec<u16>>());
    }
}
