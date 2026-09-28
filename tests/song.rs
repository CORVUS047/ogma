//! What a `Song` reports, and what it does with a file it cannot read.

mod common;

use std::time::Duration;

use ogma::song::Song;

#[test]
fn reports_the_metadata_of_a_real_file() {
    let path = common::tagged_wav("song-real");
    let song = Song::new(&path);

    assert_eq!(song.path(), path);
    assert!(song.is_readable());
    assert!(song.error().is_none());

    assert_eq!(song.title(), Some("Test Title"));
    assert_eq!(song.artist(), Some("Test Artist"));
    assert_eq!(song.album(), Some("Test Album"));
    assert_eq!(song.genre(), Some("Ambient"));
    assert_eq!(song.track_number(), Some(3));
    assert_eq!(song.year(), Some(2024));

    assert_eq!(song.duration(), Some(Duration::from_millis(500)));
    assert_eq!(song.codec(), Some("pcm"));
    assert_eq!(song.sample_rate(), Some(44_100));
    assert_eq!(song.bits_per_sample(), Some(16));
    assert_eq!(song.channel_count(), Some(1));
    assert_eq!(song.is_lossless(), Some(true));

    // Anything the accessors do not cover is still reachable.
    assert!(song.meta().is_some_and(|meta| meta.container().is_some()));
}

#[test]
fn absent_metadata_reads_as_none() {
    let song = Song::new(common::tagged_wav("song-absent"));

    assert_eq!(song.composer(), None);
    assert_eq!(song.disc_number(), None);
    assert_eq!(song.bpm(), None);
    assert_eq!(song.replay_gain(), None);
    assert!(song.front_cover().is_none());
}

#[test]
fn an_unreadable_file_reports_none_rather_than_failing() {
    let dir = common::scratch_dir("song-garbage");
    let path = dir.join("not-audio.bin");
    std::fs::write(&path, b"nothing musical in here").expect("write file");

    let song = Song::new(&path);

    assert!(!song.is_readable());
    assert!(song.error().is_some());
    assert_eq!(song.title(), None);
    assert_eq!(song.duration(), None);
    assert_eq!(song.codec(), None);

    // Still displayable: the file name stands in for the missing title.
    assert_eq!(song.display_title(), "not-audio");
    assert_eq!(song.display_artist(), "Unknown Artist");
    assert_eq!(song.display_album(), "Unknown Album");
    assert_eq!(song.display_duration(), "--:--");
}

#[test]
fn load_rejects_what_new_tolerates() {
    let dir = common::scratch_dir("song-load");
    let bad = dir.join("bad.bin");
    std::fs::write(&bad, b"junk").expect("write file");

    assert!(Song::load(&bad).is_err());
    assert!(Song::load(common::write_tagged_wav(&dir)).is_ok());
}

#[test]
fn display_helpers_prefer_tags_then_fall_back() {
    let song = Song::new(common::tagged_wav("song-display"));

    assert_eq!(song.display_title(), "Test Title");
    assert_eq!(song.display_artist(), "Test Artist");
    assert_eq!(song.display_album(), "Test Album");
    // Half a second rounds down to zero seconds, minutes first.
    assert_eq!(song.display_duration(), "0:00");
}

#[test]
fn songs_are_equal_when_they_are_the_same_file() {
    let path = common::tagged_wav("song-eq");

    let first = Song::new(&path);
    let second = Song::new(&path);
    // Reading metadata on one must not change how they compare.
    let _ = second.title();

    assert_eq!(first, second);
    assert_ne!(first, Song::new(path.with_file_name("other.wav")));
}

#[test]
fn a_clone_shares_the_cached_metadata() {
    let song = Song::new(common::tagged_wav("song-clone"));
    assert_eq!(song.title(), Some("Test Title"));

    let clone = song.clone();
    // The clone answers from the cache the original filled, even if the file is now gone.
    std::fs::remove_file(song.path()).expect("remove file");

    assert_eq!(clone.title(), Some("Test Title"));
    assert_eq!(Song::new(song.path()).title(), None, "a fresh song reads the missing file");
}
