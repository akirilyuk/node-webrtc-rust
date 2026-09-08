//! Integration tests for clip player decode and playback status.

use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use node_webrtc_rust_mixer::{FRAME_BYTES, SAMPLE_RATE};
use node_webrtc_rust_player::{ClipSession, ClipStatus, Mp4Layout, probe_mp4_layout};

static TEST_SERIAL: Mutex<()> = Mutex::new(());

fn with_serial<F: FnOnce()>(f: F) {
    let _guard = TEST_SERIAL.lock().expect("test serial lock");
    f();
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn make_wav_pcm() -> Vec<u8> {
    let sample_rate = SAMPLE_RATE;
    let channels = 2u16;
    let duration_ms = 500u32;
    let num_samples = (sample_rate as usize * duration_ms as usize) / 1000;
    let data_size = num_samples * channels as usize * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * channels as u32 * 2;
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&(channels * 2).to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_size as u32).to_le_bytes());
    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        let sample = (std::f32::consts::PI * 2.0 * 440.0 * t).sin();
        let v = (sample * i16::MAX as f32 * 0.2) as i16;
        for _ in 0..channels {
            wav.extend_from_slice(&v.to_le_bytes());
        }
    }
    wav
}

fn wait_for_status(session: &ClipSession, want: ClipStatus, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let status = session.status();
        if status.status == want {
            return;
        }
        if status.status == ClipStatus::Error {
            panic!("unexpected error: {:?}", status.error);
        }
        if std::time::Instant::now() >= deadline {
            panic!("timeout waiting for {want:?}, last={status:?}");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn wav_pcm_decodes_and_plays() {
    with_serial(|| {
        let wav = make_wav_pcm();
        let session =
            ClipSession::start_from_bytes("test-wav".into(), wav).expect("play wav");
        wait_for_status(&session, ClipStatus::Playing, Duration::from_secs(2));
        assert!(session.status().buffered_ms >= 100);

        let frame = session
            .take_frame()
            .expect("take_frame while playing should return PCM");
        assert_eq!(frame.pcm.len(), FRAME_BYTES);
    });
}

#[test]
fn mp3_fixture_plays() {
    with_serial(|| {
        let path = fixtures_dir().join("tone.mp3");
        let session = ClipSession::start_from_path("test-mp3".into(), &path).expect("play mp3");
        wait_for_status(&session, ClipStatus::Playing, Duration::from_secs(3));
        assert!(session.status().buffered_ms > 0);
    });
}

#[test]
fn wav_preroll_before_eof() {
    with_serial(|| {
        let wav = make_wav_pcm();
        let chunk = wav.len() / 2;
        let (session, writer) = ClipSession::start_from_growing("test-wav-prog".into());
        writer.append(&wav[..chunk]);
        thread::sleep(Duration::from_millis(50));
        let status = session.status();
        assert!(
            status.status == ClipStatus::Playing
                || (status.status == ClipStatus::Buffering && status.buffered_ms > 0),
            "expected preroll before eof, got {status:?}"
        );
        writer.append(&wav[chunk..]);
        writer.mark_eof();
        wait_for_status(&session, ClipStatus::Playing, Duration::from_secs(3));
        assert!(session.status().buffered_ms > 0);
    });
}

#[test]
fn aac_adts_preroll_before_eof() {
    with_serial(|| {
        let data = std::fs::read(fixtures_dir().join("tone.aac")).expect("read aac");
        let chunk = data.len() / 2;
        let (session, writer) = ClipSession::start_from_growing("test-aac".into());
        writer.append(&data[..chunk]);
        thread::sleep(Duration::from_millis(100));
        writer.append(&data[chunk..]);
        writer.mark_eof();
        wait_for_status(&session, ClipStatus::Playing, Duration::from_secs(5));
        assert!(session.status().buffered_ms > 0);
    });
}

#[test]
fn fast_start_m4a_preroll_before_eof() {
    with_serial(|| {
        let data = std::fs::read(fixtures_dir().join("tone-fast.m4a")).expect("read m4a");
        assert!(
            probe_mp4_layout(&data) == Mp4Layout::FastStart
                || probe_mp4_layout(&data) == Mp4Layout::MoovAvailable
        );
        let chunk = data.len() / 2;
        let (session, writer) = ClipSession::start_from_growing("test-fast-m4a".into());
        writer.append(&data[..chunk]);
        thread::sleep(Duration::from_millis(150));
        writer.append(&data[chunk..]);
        writer.mark_eof();
        wait_for_status(&session, ClipStatus::Playing, Duration::from_secs(5));
    });
}

#[test]
fn moov_at_end_stays_buffering_until_complete() {
    with_serial(|| {
        let data = std::fs::read(fixtures_dir().join("tone-moov-end.m4a")).expect("read m4a");
        let split = (data.len() as f64 * 0.85) as usize;
        assert_eq!(probe_mp4_layout(&data[..split]), Mp4Layout::MoovAtEnd);
        let (session, writer) = ClipSession::start_from_growing("test-moov-end".into());
        writer.append(&data[..split]);
        thread::sleep(Duration::from_millis(200));
        assert_eq!(session.status().status, ClipStatus::Buffering);
        writer.append(&data[split..]);
        writer.mark_eof();
        wait_for_status(&session, ClipStatus::Playing, Duration::from_secs(5));
    });
}

#[test]
fn stop_mid_play() {
    with_serial(|| {
        let path = fixtures_dir().join("tone.mp3");
        let session = ClipSession::start_from_path("test-stop".into(), &path).expect("play");
        wait_for_status(&session, ClipStatus::Playing, Duration::from_secs(3));
        session.stop();
        thread::sleep(Duration::from_millis(50));
        assert_eq!(session.status().status, ClipStatus::Stopped);
        assert!(session.take_frame().is_none());
    });
}

#[test]
fn position_monotonic_while_playing() {
    with_serial(|| {
        let path = fixtures_dir().join("tone.mp3");
        let session = ClipSession::start_from_path("test-pos".into(), &path).expect("play");
        wait_for_status(&session, ClipStatus::Playing, Duration::from_secs(3));
        let mut last = 0u64;
        for _ in 0..5 {
            let frame = session.take_frame();
            if frame.is_none() {
                thread::sleep(Duration::from_millis(20));
                continue;
            }
            assert_eq!(frame.unwrap().pcm.len(), FRAME_BYTES);
            let status = session.status();
            assert!(status.position_ms >= last);
            assert!(status.position_ms > last || last == 0);
            last = status.position_ms;
        }
        assert!(last > 0, "position_ms should advance when frames are consumed");
    });
}

#[test]
fn truncated_garbage_errors() {
    with_serial(|| {
        let session =
            ClipSession::start_from_bytes("test-garbage".into(), vec![0xDE, 0xAD, 0xBE, 0xEF])
                .expect("play");
        wait_for_status(&session, ClipStatus::Error, Duration::from_secs(2));
        assert!(session.status().error.is_some());
    });
}

#[test]
fn raw_pcm_identity_frames() {
    with_serial(|| {
        let frames = 5;
        let pcm_len = frames * FRAME_BYTES;
        let pcm: Vec<u8> = vec![0u8; pcm_len];
        let pcm_path = std::env::temp_dir().join("nwr-player-test.pcm");
        std::fs::write(&pcm_path, &pcm).expect("write pcm");
        let session = ClipSession::start_from_path("test-raw".into(), &pcm_path).expect("play raw");
        wait_for_status(&session, ClipStatus::Playing, Duration::from_secs(2));
        assert!(session.status().duration_ms.unwrap_or(0) >= 100);
    });
}
