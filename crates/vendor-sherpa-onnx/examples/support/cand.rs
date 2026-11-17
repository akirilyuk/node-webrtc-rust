//! caller-reject-rate candidate switches shared by the bar harnesses (`near_miss_bench`,
//! `false_accept_bench`, `speaker_bench`, ...): the `NM_CAND` token parser, the C3/C4 pure rules and
//! the product enrollment (whole utterances appended until the span, one embed; C2: mean of the
//! per-utterance unit embeddings; C3: reject a poor one). Bench-only; nothing here is product code.

#![allow(dead_code)]

use node_webrtc_rust_speech::speaker_gate::{CONFIDENT_OTHER_MAX_SCORE, NEAR_MISS_MIN_SCORE};

pub const ENROLL_MS_DEFAULT: usize = 3000;
const SAMPLES_PER_MS: usize = 16;
const WINDOW_MS: usize = 1500;

/// Env var holding the candidate list: `NM_CAND` for every harness (one token set).
pub fn cand_env() -> &'static str {
    "NM_CAND"
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// caller-reject-rate chunk 2: candidate switches from `NM_CAND` (comma list, default none =
/// today's rules): `e<ms>` enrollment span, `c2` multi-segment enrollment, `c3` reject a poor
/// enrollment (bound `NM_C3_MIN`, default 0.35), `c4n<N>` (N >= 1) repeated near-miss accept,
/// `c7r<N>` (N >= 2) run of N `other` windows for a caller -> other cut, `c7nm` near-miss
/// windows are neutral for the caller -> other run, `c7scan` the 750/125 caller sub-window
/// scan vetoes a caller -> other cut. An unknown token is an error.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cand {
    pub enroll_ms: usize,
    pub c2: bool,
    pub c3: bool,
    pub c3_min: f32,
    pub c4: usize,
    pub c7_run: usize,
    pub c7_nm: bool,
    pub c7_scan: bool,
}

pub fn parse_cand(s: &str, c3_min: f32) -> Result<Cand, String> {
    let mut c = Cand {
        enroll_ms: ENROLL_MS_DEFAULT,
        c2: false,
        c3: false,
        c3_min,
        c4: 0,
        c7_run: 2,
        c7_nm: false,
        c7_scan: false,
    };
    for t in s.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        match t {
            "c2" => c.c2 = true,
            "c3" => c.c3 = true,
            "c7nm" => c.c7_nm = true,
            "c7scan" => c.c7_scan = true,
            _ => {
                if let Some(v) = t.strip_prefix('e').and_then(|v| v.parse().ok()) {
                    c.enroll_ms = v;
                } else if let Some(v) =
                    t.strip_prefix("c4n").and_then(|v| v.parse::<usize>().ok())
                {
                    if v == 0 {
                        return Err("c4n0: N must be >= 1".into());
                    }
                    c.c4 = v;
                } else if let Some(v) =
                    t.strip_prefix("c7r").and_then(|v| v.parse::<usize>().ok())
                {
                    if v < 2 {
                        return Err(format!("c7r{v}: the run must be >= 2"));
                    }
                    c.c7_run = v;
                } else {
                    return Err(format!("unknown NM_CAND token {t:?}"));
                }
            }
        }
    }
    Ok(c)
}

pub fn cand() -> Cand {
    static C: std::sync::OnceLock<Cand> = std::sync::OnceLock::new();
    *C.get_or_init(|| {
        let c3_min = std::env::var("NM_C3_MIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.35);
        parse_cand(&std::env::var(cand_env()).unwrap_or_default(), c3_min)
            .unwrap_or_else(|e| panic!("NM_CAND: {e}"))
    })
}

/// C4 on window scores: at least `n` windows in the near-miss band [`NEAR_MISS_MIN_SCORE`,
/// thr) and none below [`CONFIDENT_OTHER_MAX_SCORE`]. `n == 0`: off. (Optimistic against the
/// product: an accept does not arm the caller -> other watch, as the product's upgrade arm.)
pub fn c4_scores(scores: &[f32], threshold: f32, n: usize) -> bool {
    n > 0
        && scores.iter().all(|s| *s >= CONFIDENT_OTHER_MAX_SCORE)
        && scores
            .iter()
            .filter(|s| **s >= NEAR_MISS_MIN_SCORE && **s < threshold)
            .count()
            >= n
}

/// What the C3 enrollment loop does after one embed whose own sub-windows' minimum is `mn`
/// (`best` = the best minimum of the earlier tries, `tries` includes this one).
#[derive(Debug, PartialEq)]
pub enum C3Step {
    Accept,
    Retry,
    /// Out of retries or files: the best minimum seen (this one included).
    TakeBest(f32),
}

pub fn c3_step(mn: f32, min: f32, tries: usize, best: Option<f32>, exhausted: bool) -> C3Step {
    if mn >= min {
        C3Step::Accept
    } else if tries >= 3 || exhausted {
        C3Step::TakeBest(best.map_or(mn, |b| b.max(mn)))
    } else {
        C3Step::Retry
    }
}

/// Product enrollment over `n` candidate utterances. `unit(i)` returns utterance `i`'s voiced audio
/// (bed applied by the caller) or `None` when it holds no speech; whole utterances are appended
/// until the buffer reaches `c.enroll_ms`, then one embed of the buffer is the reference (C2: the
/// sum of the unit embeddings of the utterances of at least 1 s, so cosine sees their mean; C3: the
/// enrollment's own 1.5 s sub-windows against the reference, retry up to 3 tries, then the best).
/// Returns (reference, utterances used, voiced ms consumed); `Ok(None)` when the files could not
/// enroll the gate.
pub fn enroll_product(
    n: usize,
    mut unit: impl FnMut(usize) -> Result<Option<Vec<f32>>, String>,
    mut embed: impl FnMut(&[f32]) -> Option<Vec<f32>>,
    c: &Cand,
) -> Result<Option<(Vec<f32>, usize, usize)>, String> {
    let mut buf: Vec<f32> = Vec::new();
    let mut acc: Vec<f32> = Vec::new();
    let (mut ms, mut tries, mut used) = (0usize, 0usize, 0usize);
    let mut best: Option<(f32, Vec<f32>)> = None;
    for i in 0..n {
        let Some(u) = unit(i)? else {
            continue;
        };
        ms += u.len() / SAMPLES_PER_MS;
        if c.c2 && u.len() >= SAMPLES_PER_MS * 1000 {
            if let Some(e) = embed(&u) {
                let nrm = e.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
                if acc.is_empty() {
                    acc = vec![0.0; e.len()];
                }
                for (a, x) in acc.iter_mut().zip(&e) {
                    *a += x / nrm;
                }
            }
        }
        buf.extend(u);
        if buf.len() < c.enroll_ms * SAMPLES_PER_MS {
            continue;
        }
        let reference = if c.c2 && !acc.is_empty() {
            Some(acc.clone())
        } else {
            embed(&buf)
        };
        let Some(r) = reference else {
            return Ok(None);
        };
        if !c.c3 {
            return Ok(Some((r, i + 1, ms)));
        }
        let w = WINDOW_MS * SAMPLES_PER_MS;
        let mn = buf
            .chunks_exact(w)
            .filter_map(|ch| embed(ch).map(|e| cosine(&e, &r)))
            .fold(f32::MAX, f32::min);
        tries += 1;
        if let Some(path) = std::env::var_os("NM_C3_DIST") {
            // Self-score distribution of the enrollment's own 1.5 s sub-windows (never committed).
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(f, "{tries}\t{mn:.4}");
            }
        }
        let prev_best = best.as_ref().map(|(m, _)| *m);
        if best.as_ref().map_or(true, |(m, _)| mn > *m) {
            best = Some((mn, r));
        }
        match c3_step(mn, c.c3_min, tries, prev_best, i + 1 == n) {
            C3Step::Accept | C3Step::TakeBest(_) => {
                used = i + 1;
                break;
            }
            C3Step::Retry => {
                buf.clear();
                acc.clear();
            }
        }
    }
    // C3 out of files before it accepted: take the best try instead of dropping the speaker.
    let used = if used == 0 { n } else { used };
    Ok(best.map(|(_, r)| (r, used, ms)))
}

#[cfg(test)]
mod cand_tests {
    use super::*;

    /// Embedding stub: [mean, len in ms / 1000].
    fn emb(a: &[f32]) -> Option<Vec<f32>> {
        let m = a.iter().sum::<f32>() / a.len().max(1) as f32;
        Some(vec![m, a.len() as f32 / 16_000.0])
    }

    fn units(lens_ms: &[usize], vals: &[f32]) -> Vec<Vec<f32>> {
        lens_ms
            .iter()
            .zip(vals)
            .map(|(l, v)| vec![*v; l * SAMPLES_PER_MS])
            .collect()
    }

    #[test]
    fn product_enrollment_appends_whole_utterances_until_the_span() {
        let u = units(&[2000, 2500, 2000], &[1.0, 1.0, 1.0]);
        let c = parse_cand("", 0.35).unwrap();
        let (r, used, ms) = enroll_product(3, |i| Ok(Some(u[i].clone())), emb, &c)
            .unwrap()
            .unwrap();
        // 2000 < 3000, 4500 >= 3000: two whole utterances, one embed of the 4.5 s buffer.
        assert_eq!((used, ms), (2, 4500));
        assert!((r[1] - 4.5).abs() < 1e-3);
    }

    #[test]
    fn c2_reference_is_the_sum_of_unit_embeddings_of_utterances_of_one_second_or_more() {
        let u = units(&[2000, 500, 2000], &[1.0, 5.0, 3.0]);
        let c = parse_cand("c2", 0.35).unwrap();
        let (r, used, _) = enroll_product(3, |i| Ok(Some(u[i].clone())), emb, &c)
            .unwrap()
            .unwrap();
        assert_eq!(used, 3);
        // The 500 ms utterance is not embedded; the unit embeddings of [1,2] and [3,2] add up.
        let e1 = {
            let n = (1f32 * 1.0 + 2.0 * 2.0).sqrt();
            [1.0 / n, 2.0 / n]
        };
        let e2 = {
            let n = (3f32 * 3.0 + 2.0 * 2.0).sqrt();
            [3.0 / n, 2.0 / n]
        };
        assert!((r[0] - (e1[0] + e2[0])).abs() < 1e-4 && (r[1] - (e1[1] + e2[1])).abs() < 1e-4);
    }

    #[test]
    fn too_little_speech_does_not_enroll() {
        let u = units(&[1000, 1000], &[1.0, 1.0]);
        let c = parse_cand("", 0.35).unwrap();
        assert!(enroll_product(2, |i| Ok(Some(u[i].clone())), emb, &c)
            .unwrap()
            .is_none());
    }

    #[test]
    fn unvoiced_utterances_are_skipped() {
        let u = units(&[2000, 2000], &[1.0, 1.0]);
        let c = parse_cand("", 0.35).unwrap();
        let (_, used, ms) = enroll_product(
            3,
            |i| Ok(if i == 0 { None } else { Some(u[i - 1].clone()) }),
            emb,
            &c,
        )
        .unwrap()
        .unwrap();
        assert_eq!((used, ms), (3, 4000));
    }
}
