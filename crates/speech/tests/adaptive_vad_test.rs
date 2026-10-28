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
