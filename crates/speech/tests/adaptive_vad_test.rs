//! Adaptive energy VAD: deterministic synthetic 48 kHz stereo 20 ms frames through
//! `VadEngine::process_webrtc_pcm`.

use node_webrtc_rust_speech::config::{AdaptiveVadConfig, VadConfig};
use node_webrtc_rust_speech::vad::{
    AdaptiveEnergyVad, VadEngine, VadTransition, VoiceActivityDetector,
};
use std::f32::consts::PI;

const FRAME_MS: u32 = 20;
const SAMPLES: usize = 960; // per channel, 20 ms at 48 kHz

/// Deterministic white noise in [-1, 1) (rms 1/sqrt(3)).
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    }
}

fn amp(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// What a frame holds at time `t_ms`.
#[derive(Clone, Copy)]
struct Part {
    /// White noise at this rms level (dBFS), optionally 4 Hz +-6 dB modulated.
    noise_db: Option<f32>,
    /// Depth in dB of the 4 Hz level modulation (0 = steady).
    mod_db: f32,
    /// 440 Hz tone at this rms level (dBFS).
    tone_db: Option<f32>,
}

const SILENT: Part = Part {
    noise_db: None,
    mod_db: 0.0,
    tone_db: None,
};

fn noise(db: f32) -> Part {
    Part {
        noise_db: Some(db),
        ..SILENT
    }
}

fn babble(db: f32) -> Part {
    babble_mod(db, 6.0)
}

fn babble_mod(db: f32, mod_db: f32) -> Part {
    Part {
        noise_db: Some(db),
        mod_db,
        ..SILENT
    }
}

fn with_tone(p: Part, db: f32) -> Part {
    Part {
        tone_db: Some(db),
        ..p
    }
}

fn to_pcm(rng: &mut Noise, frame_idx: usize, part: Part) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(SAMPLES * 4);
    for i in 0..SAMPLES {
        let n = frame_idx * SAMPLES + i;
        let t = n as f32 / 48_000.0;
        let mut v = 0.0f32;
        if let Some(db) = part.noise_db {
            let gain = if part.mod_db > 0.0 {
                amp(part.mod_db * (2.0 * PI * 4.0 * t).sin())
            } else {
                1.0
            };
            // white noise rms is 1/sqrt(3)
            v += rng.next() * 3f32.sqrt() * amp(db) * gain;
        }
        if let Some(db) = part.tone_db {
            v += (2.0 * PI * 440.0 * t).sin() * 2f32.sqrt() * amp(db);
        }
        let s = (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        pcm.extend_from_slice(&s.to_le_bytes());
        pcm.extend_from_slice(&s.to_le_bytes());
    }
    pcm
}

fn adaptive_config() -> VadConfig {
    VadConfig {
        provider: "adaptive".to_string(),
        ..VadConfig::default()
    }
}

/// Runs `total_ms` of frames, returns `(time_ms_after_frame, transition)`.
fn run(
    engine: &mut VadEngine,
    total_ms: u32,
    part_at: impl Fn(u32) -> Part,
) -> Vec<(u32, VadTransition)> {
    run_traced(engine, total_ms, part_at).0
}

/// Like [`run`], plus `is_speaking` after every frame.
fn run_traced(
    engine: &mut VadEngine,
    total_ms: u32,
    part_at: impl Fn(u32) -> Part,
) -> (Vec<(u32, VadTransition)>, Vec<bool>) {
    let mut rng = Noise(42);
    let mut out = Vec::new();
    let mut speaking = Vec::new();
    for idx in 0..(total_ms / FRAME_MS) as usize {
        let t = idx as u32 * FRAME_MS;
        let pcm = to_pcm(&mut rng, idx, part_at(t));
        let (tr, _) = engine.process_webrtc_pcm(&pcm, FRAME_MS).unwrap();
        for x in tr {
            out.push((t + FRAME_MS, x));
        }
        speaking.push(engine.is_speaking());
    }
    (out, speaking)
}

fn speaking_between(trace: &[bool], from_ms: u32, to_ms: u32) -> bool {
    trace[(from_ms / FRAME_MS) as usize..(to_ms / FRAME_MS) as usize]
        .iter()
        .any(|s| *s)
}

fn starts(v: &[(u32, VadTransition)]) -> Vec<u32> {
    v.iter()
        .filter(|(_, t)| *t == VadTransition::SpeechStart)
        .map(|(ms, _)| *ms)
        .collect()
}

fn ends(v: &[(u32, VadTransition)]) -> Vec<u32> {
    v.iter()
        .filter(|(_, t)| *t == VadTransition::SpeechEnd)
        .map(|(ms, _)| *ms)
        .collect()
}

fn quiet_caller_scene(t: u32) -> Part {
    if (3000..4000).contains(&t) {
        with_tone(noise(-55.0), -30.0)
    } else {
        noise(-55.0)
    }
}

#[test]
fn adaptive_detects_quiet_caller() {
    let mut engine = VadEngine::new(adaptive_config()).unwrap();
    let ev = run(&mut engine, 6000, quiet_caller_scene);
    let s = starts(&ev);
    assert_eq!(s.len(), 1, "{ev:?}");
    assert!(s[0] >= 3000 && s[0] <= 3400, "start at {} ms", s[0]);

    let mut energy = VadEngine::new(VadConfig::default()).unwrap();
    let ev = run(&mut energy, 6000, quiet_caller_scene);
    assert!(starts(&ev).is_empty(), "energy must not fire: {ev:?}");
}

#[test]
fn adaptive_ignores_steady_babble() {
    let mut engine = VadEngine::new(adaptive_config()).unwrap();
    let ev = run(&mut engine, 10_000, |_| babble(-30.0));
    assert!(starts(&ev).is_empty(), "{ev:?}");
}

#[test]
fn adaptive_caller_over_babble() {
    let mut engine = VadEngine::new(adaptive_config()).unwrap();
    let ev = run(&mut engine, 10_000, |t| {
        if (4000..6000).contains(&t) {
            with_tone(babble(-30.0), -20.0)
        } else {
            babble(-30.0)
        }
    });
    let s = starts(&ev);
    assert_eq!(s.len(), 1, "{ev:?}");
    assert!(s[0] >= 4000 && s[0] <= 4400, "start at {} ms", s[0]);
    assert_eq!(ends(&ev).len(), 1, "{ev:?}");
}

#[test]
fn adaptive_stuck_guard_recovers() {
    let mut engine = VadEngine::new(adaptive_config()).unwrap();
    let (ev, trace) = run_traced(&mut engine, 17_000, |t| {
        if t < 3000 {
            noise(-55.0)
        } else {
            with_tone(noise(-55.0), -30.0)
        }
    });
    assert!(starts(&ev).len() <= 1, "{ev:?}");
    assert!(ends(&ev).len() <= 1, "{ev:?}");
    assert!(
        !speaking_between(&trace, 14_000, 17_000),
        "still speaking after 14 s: {ev:?}"
    );
}

#[test]
fn adaptive_recovers_after_noise_step() {
    let mut engine = VadEngine::new(adaptive_config()).unwrap();
    let (ev, trace) = run_traced(&mut engine, 22_000, |t| {
        if t < 6000 {
            if (2000..3000).contains(&t) || (4000..5000).contains(&t) {
                with_tone(babble(-45.0), -20.0)
            } else {
                babble(-45.0)
            }
        } else {
            babble_mod(-30.0, 10.0)
        }
    });
    assert!(
        ends(&ev).iter().any(|ms| *ms <= 16_000),
        "no SpeechEnd by 16 s: {ev:?}"
    );
    assert!(
        !speaking_between(&trace, 18_000, 22_000),
        "locked open after the noise step: {ev:?}"
    );
}

#[test]
fn adaptive_call_start_speech_still_ends() {
    let mut cfg = adaptive_config();
    cfg.adaptive.use_speech_level = false;
    let mut engine = VadEngine::new(cfg).unwrap();
    let ev = run(&mut engine, 5000, |t| {
        if t < 1000 {
            with_tone(noise(-45.0), -10.0)
        } else {
            noise(-45.0)
        }
    });
    assert_eq!(starts(&ev).len(), 1, "{ev:?}");
    let e = ends(&ev);
    assert_eq!(e.len(), 1, "{ev:?}");
    assert!(e[0] <= 2600, "end at {} ms: {ev:?}", e[0]);
}

#[test]
fn adaptive_hysteresis_keeps_word_gaps() {
    let cfg = VadConfig {
        min_silence_duration_ms: 100,
        ..adaptive_config()
    };
    let mut engine = VadEngine::new(cfg.clone()).unwrap();
    // Learn T on a first second of speech.
    let _ = run(&mut engine, 4000, quiet_caller_scene);
    let t_db = engine.adaptive_state().unwrap().threshold_db;
    assert!(t_db.is_finite());
    let scene = move |t: u32| {
        if t < 3000 {
            noise(-55.0)
        } else if (3600..3800).contains(&t) {
            with_tone(noise(-55.0), t_db - 2.0)
        } else if t < 5000 {
            with_tone(noise(-55.0), -30.0)
        } else {
            noise(-55.0)
        }
    };
    let mut engine = VadEngine::new(cfg.clone()).unwrap();
    let ev = run(&mut engine, 8000, scene);
    assert_eq!(starts(&ev).len(), 1, "{ev:?}");
    assert_eq!(ends(&ev).len(), 1, "{ev:?}");

    // Without hysteresis the same dip splits the utterance (the dip is below T).
    let mut no_hyst = cfg;
    no_hyst.adaptive.hysteresis_db = 0.0;
    let mut engine = VadEngine::new(no_hyst).unwrap();
    let ev = run(&mut engine, 8000, scene);
    assert_eq!(starts(&ev).len(), 2, "{ev:?}");
}

#[test]
fn adaptive_warmup_uses_energy_threshold() {
    let scene = |t: u32| {
        if t < 1000 {
            with_tone(noise(-60.0), -10.0)
        } else {
            noise(-60.0)
        }
    };
    let mut adaptive = VadEngine::new(adaptive_config()).unwrap();
    let a = run(&mut adaptive, 3000, scene);
    let mut energy = VadEngine::new(VadConfig::default()).unwrap();
    let e = run(&mut energy, 3000, scene);
    assert_eq!(starts(&a).first(), starts(&e).first(), "{a:?} vs {e:?}");
    assert!(!starts(&a).is_empty());
}

#[test]
fn adaptive_reset_clears_state() {
    let mut fresh = VadEngine::new(adaptive_config()).unwrap();
    let expected = run(&mut fresh, 6000, quiet_caller_scene);

    let mut engine = VadEngine::new(adaptive_config()).unwrap();
    let _ = run(&mut engine, 5000, |_| with_tone(noise(-25.0), -12.0));
    engine.reset();
    assert!(!engine.is_speaking());
    let got = run(&mut engine, 6000, quiet_caller_scene);
    assert_eq!(got, expected);
}

#[test]
fn adaptive_config_rejects_invalid() {
    type Mutate = Box<dyn Fn(&mut AdaptiveVadConfig)>;
    let cases: Vec<(&str, Mutate)> = vec![
        ("noisePercentile", Box::new(|c| c.noise_percentile = 0)),
        ("noisePercentile", Box::new(|c| c.noise_percentile = 100)),
        ("noiseWindowMs", Box::new(|c| c.noise_window_ms = 400)),
        ("warmupMs", Box::new(|c| c.warmup_ms = 3001)),
        ("noiseMarginDb", Box::new(|c| c.noise_margin_db = 31.0)),
        ("hysteresisDb", Box::new(|c| c.hysteresis_db = -1.0)),
        (
            "minThresholdDbfs",
            Box::new(|c| c.min_threshold_dbfs = c.max_threshold_dbfs),
        ),
        ("maxThresholdDbfs", Box::new(|c| c.max_threshold_dbfs = 3.0)),
        ("stuckActiveMs", Box::new(|c| c.stuck_active_ms = 999)),
    ];
    for (field, mutate) in cases {
        let mut cfg = adaptive_config();
        mutate(&mut cfg.adaptive);
        match VadEngine::new(cfg) {
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains(&format!("vad.adaptive.{field}")),
                    "{field}: {msg}"
                );
            }
            Ok(_) => panic!("{field} must be rejected"),
        }
    }
    assert!(VadEngine::new(adaptive_config()).is_ok());
}

#[test]
fn adaptive_frame_cost() {
    let mut vad = AdaptiveEnergyVad::new(0.15, AdaptiveVadConfig::default());
    let mut rng = Noise(7);
    let frame: Vec<i16> = (0..320).map(|_| (rng.next() * 800.0) as i16).collect();
    let start = std::time::Instant::now();
    for i in 0..100_000 {
        // vary the level a little so the ring content changes
        let mut f = frame.clone();
        f[0] = (i % 1000) as i16;
        vad.process_mono_frame(&f, 16_000).unwrap();
    }
    let limit = if cfg!(debug_assertions) { 10 } else { 2 };
    let took = start.elapsed();
    assert!(
        took.as_secs_f32() < limit as f32,
        "100k frames took {took:?}"
    );
}

// ---- Long-turn reliability (chunk 2b). Every randomized test loops seeds 0..50. ----

/// Samples with a 4 Hz +-8 dB syllable envelope applied on top of `to_pcm` output.
fn syllable_pcm(rng: &mut Noise, idx: usize, part: Part) -> Vec<u8> {
    let mut pcm = to_pcm(rng, idx, part);
    if part.tone_db.is_some() {
        for i in 0..SAMPLES {
            let t = (idx * SAMPLES + i) as f32 / 48_000.0;
            let g = amp(8.0 * (2.0 * PI * 4.0 * t).sin());
            for ch in 0..2 {
                let o = i * 4 + ch * 2;
                let s = i16::from_le_bytes([pcm[o], pcm[o + 1]]);
                let v = (s as f32 * g).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                pcm[o..o + 2].copy_from_slice(&v.to_le_bytes());
            }
        }
    }
    pcm
}

/// Noise floor (-45 dBFS) plus, while `caller_db_at(t)` is `Some`, the modulated caller.
/// Returns transitions `(ms_after_frame, kind)`, the speaking trace and the final engine.
fn run_scene(
    seed: u64,
    total_ms: u32,
    caller_db_at: impl Fn(u32) -> Option<f32>,
) -> (Vec<(u32, VadTransition)>, Vec<bool>, VadEngine) {
    let mut engine = VadEngine::new(adaptive_config()).unwrap();
    let mut rng = Noise(seed.wrapping_mul(7919).wrapping_add(1));
    let (mut out, mut trace) = (Vec::new(), Vec::new());
    for idx in 0..(total_ms / FRAME_MS) as usize {
        let t = idx as u32 * FRAME_MS;
        let part = match caller_db_at(t) {
            Some(db) => with_tone(noise(-45.0), db),
            None => noise(-45.0),
        };
        let pcm = syllable_pcm(&mut rng, idx, part);
        let (tr, _) = engine.process_webrtc_pcm(&pcm, FRAME_MS).unwrap();
        out.extend(tr.into_iter().map(|x| (t + FRAME_MS, x)));
        trace.push(engine.is_speaking());
    }
    (out, trace, engine)
}

/// Seeded turn plan: turns of 2..7.5 s separated by 2 s of noise; in-turn pauses 100..900 ms.
/// Returns (turn windows, pause windows).
type Windows = Vec<(u32, u32)>;

fn turn_plan(seed: u64) -> (Windows, Windows) {
    let mut r = Noise(seed + 1000);
    let mut rnd = |lo: u32, hi: u32| lo + (((r.next() + 1.0) / 2.0) * (hi - lo) as f32) as u32;
    let (mut turns, mut pauses) = (Vec::new(), Vec::new());
    let mut t = 2000;
    for _ in 0..3 {
        let len = rnd(2000, 7500) / FRAME_MS * FRAME_MS;
        turns.push((t, t + len));
        let mut p = t + rnd(600, 1400) / FRAME_MS * FRAME_MS;
        while p + 900 + 400 < t + len {
            let pl = rnd(100, 900) / FRAME_MS * FRAME_MS;
            pauses.push((p, p + pl));
            p += pl + rnd(500, 1500) / FRAME_MS * FRAME_MS;
        }
        t += len + 2000;
    }
    (turns, pauses)
}

#[test]
fn adaptive_turn_under_guard_never_cut() {
    for seed in 0..50u64 {
        let (turns, pauses) = turn_plan(seed);
        let total = turns.last().unwrap().1 + 3000;
        let (ev, _, _) = run_scene(seed, total, |t| {
            let in_turn = turns.iter().any(|(a, b)| t >= *a && t < *b);
            let in_pause = pauses.iter().any(|(a, b)| t >= *a && t < *b);
            (in_turn && !in_pause).then_some(-20.0)
        });
        let (s, e) = (starts(&ev), ends(&ev));
        assert_eq!(s.len(), turns.len(), "seed {seed}: {ev:?} turns {turns:?}");
        assert_eq!(e.len(), turns.len(), "seed {seed}: {ev:?} turns {turns:?}");
        for (i, (a, b)) in turns.iter().enumerate() {
            assert!(s[i] >= *a, "seed {seed}: start {} before turn {a}", s[i]);
            assert!(e[i] >= *b, "seed {seed}: end {} before turn end {b}", e[i]);
        }
    }
}

/// 12 s continuous caller from 3 s; returns the engine events and the trace.
fn long_turn(
    seed: u64,
    total_ms: u32,
    db: f32,
) -> (Vec<(u32, VadTransition)>, Vec<bool>, VadEngine) {
    run_scene(seed, total_ms, |t| {
        (3000..15_000).contains(&t).then_some(db)
    })
}

#[test]
fn adaptive_long_turn_cut_bounded() {
    let mut cuts = Vec::new();
    for seed in 0..50u64 {
        let (ev, _, _) = long_turn(seed, 20_000, -20.0);
        let (s, e) = (starts(&ev), ends(&ev));
        assert_eq!(s.len(), 1, "seed {seed}: second lock-open: {ev:?}");
        assert_eq!(e.len(), 1, "seed {seed}: {ev:?}");
        let dur = e[0] - s[0];
        assert!(
            (8000..=11_000).contains(&dur),
            "seed {seed}: cut after {dur} ms"
        );
        cuts.push(dur);
    }
    let (min, max) = (*cuts.iter().min().unwrap(), *cuts.iter().max().unwrap());
    eprintln!("long_turn cut ms: min {min} max {max}");
    assert!(
        (PIN_CUT_MS - 500..=PIN_CUT_MS + 500).contains(&min)
            && (PIN_CUT_MS - 500..=PIN_CUT_MS + 500).contains(&max),
        "cut range {min}..{max} left the pinned {PIN_CUT_MS} +- 500 ms"
    );
}

/// Measured cut time of a 12 s caller turn (ms after SpeechStart).
const PIN_CUT_MS: u32 = 10_940;

#[test]
fn adaptive_recovers_after_long_turn_cut() {
    for seed in 0..50u64 {
        // 12 s turn at 3 s, cut ~11 s, caller stops at 15 s, 3 s noise, -26 dBFS turn at 18 s.
        let (ev, _, _) = run_scene(seed, 22_000, |t| {
            if (3000..15_000).contains(&t) {
                Some(-20.0)
            } else if (18_000..19_500).contains(&t) {
                Some(-26.0)
            } else {
                None
            }
        });
        let s: Vec<u32> = starts(&ev).into_iter().filter(|ms| *ms >= 18_000).collect();
        assert_eq!(s.len(), 1, "seed {seed}: {ev:?}");
        assert!(
            s[0] <= 18_400,
            "seed {seed}: start {} ms after onset 18000",
            s[0]
        );
    }
}

#[test]
fn adaptive_twenty_second_monologue_bounded() {
    for seed in 0..50u64 {
        let (ev, trace, engine) = run_scene(seed, 30_000, |t| {
            (3000..23_000).contains(&t).then_some(-20.0)
        });
        let e = ends(&ev);
        assert!(e.len() <= 3, "seed {seed}: {ev:?}");
        let first_start = starts(&ev)[0];
        assert!(
            e.iter().all(|ms| *ms - first_start >= 8000),
            "seed {seed}: early end {ev:?}"
        );
        assert!(
            !speaking_between(&trace, 24_500, 27_000),
            "seed {seed}: still speaking after the caller stopped: {ev:?}"
        );
        // 4 s after the end, N is within 3 dB of what it read on the same noise before the
        // caller (the VAD's 16 kHz mono level of -45 dBFS white noise sits below -45).
        let base = run_scene(seed, 3000, |_| None)
            .2
            .adaptive_state()
            .unwrap()
            .noise_db;
        let n = engine.adaptive_state().unwrap().noise_db;
        assert!(
            (n - base).abs() <= 3.0,
            "seed {seed}: N {n} dB vs baseline {base} dB"
        );
    }
}

#[test]
fn adaptive_guard_vs_noise_step_seeds() {
    for seed in 0..50u64 {
        let mut engine = VadEngine::new(adaptive_config()).unwrap();
        let mut rng = Noise(seed + 77);
        let mut ends_ms = Vec::new();
        for idx in 0..(22_000 / FRAME_MS) as usize {
            let t = idx as u32 * FRAME_MS;
            let part = if t < 6000 {
                if (2000..3000).contains(&t) || (4000..5000).contains(&t) {
                    with_tone(babble(-45.0), -20.0)
                } else {
                    babble(-45.0)
                }
            } else {
                babble_mod(-30.0, 10.0)
            };
            let pcm = to_pcm(&mut rng, idx + seed as usize * 7, part);
            let (tr, _) = engine.process_webrtc_pcm(&pcm, FRAME_MS).unwrap();
            if tr.contains(&VadTransition::SpeechEnd) {
                ends_ms.push(t + FRAME_MS);
            }
        }
        assert!(
            ends_ms.iter().any(|ms| *ms <= 16_000),
            "seed {seed}: no SpeechEnd by 16 s: {ends_ms:?}"
        );
    }
}

#[test]
fn adaptive_guard_episode_logs_once() {
    // One guard episode per over-long utterance, however many frames it feeds; zero otherwise.
    let (_, _, engine) = long_turn(1, 20_000, -20.0);
    assert_eq!(engine.adaptive_state().unwrap().stuck_episodes, 1);

    let (_, _, engine) = run_scene(1, 12_000, |t| (3000..6000).contains(&t).then_some(-20.0));
    assert_eq!(engine.adaptive_state().unwrap().stuck_episodes, 0);

    // Two long turns: two episodes.
    let (_, _, engine) = run_scene(1, 40_000, |t| {
        ((3000..15_000).contains(&t) || (22_000..34_000).contains(&t)).then_some(-20.0)
    });
    assert_eq!(engine.adaptive_state().unwrap().stuck_episodes, 2);
}
