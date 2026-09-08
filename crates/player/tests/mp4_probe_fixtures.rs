//! Probe layout classification on committed fixtures.

use std::path::PathBuf;

use node_webrtc_rust_player::{Mp4Layout, probe_mp4_layout};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn fast_start_fixture_has_moov_before_mdat() {
    let data = std::fs::read(fixtures_dir().join("tone-fast.m4a")).expect("read");
    let layout = probe_mp4_layout(&data);
    assert!(
        layout == Mp4Layout::FastStart || layout == Mp4Layout::MoovAvailable,
        "layout={layout:?}"
    );
}

#[test]
fn moov_end_fixture_classified() {
    let data = std::fs::read(fixtures_dir().join("tone-moov-end.m4a")).expect("read");
    let layout = probe_mp4_layout(&data);
    // Full file may show MoovAvailable; partial prefix should be MoovAtEnd.
    let prefix_len = (data.len() as f64 * 0.85) as usize;
    let prefix_layout = probe_mp4_layout(&data[..prefix_len]);
    assert_eq!(
        prefix_layout,
        Mp4Layout::MoovAtEnd,
        "full layout={layout:?}"
    );
}
