//! End-to-end checks against a file built here, so the suite needs no fixtures.

mod common;

use common::{scratch_dir, write_tagged_wav};
use ogma::meta;

#[test]
fn reads_pcm_stream_and_riff_info_tags() {
    let dir = scratch_dir("pcm");
    let path = write_tagged_wav(&dir);

    let file = meta::probe(&path).expect("probe wav");

    assert_eq!(file.codec(), "pcm");
    assert_eq!(file.is_lossless(), Some(true));
    assert_eq!(file.sample_rate(), Some(44_100));
    assert_eq!(file.bits_per_sample(), Some(16));
    assert_eq!(file.channel_count(), Some(1));
    assert_eq!(file.frame_count(), Some(22_050));
    assert_eq!(file.duration(), Some(std::time::Duration::from_millis(500)));

    assert_eq!(file.title(), Some("Test Title"));
    assert_eq!(file.artist(), Some("Test Artist"));
    assert_eq!(file.album(), Some("Test Album"));
    assert_eq!(file.genre(), Some("Ambient"));
    assert_eq!(file.track_number(), Some(3));
    // The file states a full date, which the year accessor narrows down.
    assert_eq!(file.year(), Some(2024));

    // Absent metadata reads as None rather than an empty string.
    assert_eq!(file.composer(), None);
    assert_eq!(file.lyrics(), None);
    assert_eq!(file.musicbrainz_recording_id(), None);
    assert_eq!(file.replay_gain_track_gain(), None);
    assert!(file.artwork().is_empty());
    assert!(file.front_cover().is_none());
}

#[test]
fn rejects_a_file_that_is_not_audio() {
    let dir = scratch_dir("garbage");
    let path = dir.join("garbage.bin");
    std::fs::write(&path, b"this is not audio, not even close").expect("write garbage");

    let err = meta::probe(&path).expect_err("garbage must not probe");
    assert!(matches!(err, meta::MetaError::Unreadable { .. }));
}

#[test]
fn reports_a_missing_file_rather_than_panicking() {
    let err = meta::probe("/nonexistent/track.flac").expect_err("missing file must not probe");
    assert!(matches!(err, meta::MetaError::Unreadable { .. }));
}
