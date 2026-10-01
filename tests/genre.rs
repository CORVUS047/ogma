//! Writing genres into music files by hand.

mod common;

use std::path::{Path, PathBuf};

use common::WavSpec;
use ogma::genre::{self, Outcome};
use ogma::song::Song;

/// A tagged WAV under a name of its own, since the fixture always writes the same one.
fn track(dir: &Path, name: &str) -> PathBuf {
    let source = common::write_tagged_wav(dir);
    let path = dir.join(name);
    std::fs::rename(&source, &path).expect("rename fixture");

    path
}

fn genre_of(path: &Path) -> Option<String> {
    Song::new(path).genre().map(str::to_owned)
}

#[test]
fn a_genre_is_written_into_the_file() {
    let dir = common::scratch_dir("genre-write");
    let path = track(&dir, "track.wav");

    assert_eq!(genre_of(&path).as_deref(), Some("Ambient"), "what the fixture carries");

    assert_eq!(genre::set(&path, "Shoegaze"), Outcome::Written);
    assert_eq!(genre_of(&path).as_deref(), Some("Shoegaze"), "in the file, not just in memory");

    // The rest of the tags are left exactly as they were.
    let song = Song::new(&path);
    assert_eq!(song.title(), Some("Test Title"));
    assert_eq!(song.artist(), Some("Test Artist"));
    assert_eq!(song.album(), Some("Test Album"));
    assert_eq!(song.codec(), Some("pcm"), "and the audio still reads back");
}

#[test]
fn typing_a_genre_replaces_the_one_that_was_there() {
    let dir = common::scratch_dir("genre-replace");
    let path = track(&dir, "track.wav");

    // A genre typed in by hand is an instruction, not a guess: it overrules whatever it finds.
    genre::set(&path, "First");
    genre::set(&path, "Second");

    assert_eq!(genre_of(&path).as_deref(), Some("Second"));

    // And exactly one is left behind, rather than the new one being added beside the old.
    let song = Song::new(&path);
    assert_eq!(song.meta().and_then(|meta| meta.genres()).map(|all| all.len()), Some(1));
}

#[test]
fn a_run_over_several_files_says_what_it_did() {
    let dir = common::scratch_dir("genre-several");
    let one = track(&dir, "one.wav");
    let two = track(&dir, "two.wav");

    // A file that is not music at all, to be reported rather than to stop the run.
    let broken = dir.join("broken.wav");
    std::fs::write(&broken, b"not a wav").expect("write file");

    let report = genre::set_all([&one, &two, &broken], "Drum and Bass");

    assert_eq!(report.written, 2);
    assert_eq!(report.failed, 1);
    assert_eq!(genre_of(&one).as_deref(), Some("Drum and Bass"));
    assert_eq!(genre_of(&two).as_deref(), Some("Drum and Bass"));

    let summary = report.summary("Drum and Bass");
    assert!(summary.contains("2 tracks"), "{summary}");
    assert!(summary.contains("1 failed"), "{summary}");
}

#[test]
fn a_format_that_cannot_hold_a_genre_is_reported_not_written() {
    let dir = common::scratch_dir("genre-unsupported");

    // A bare file with no container a tag can live in.
    let path = dir.join("track.xyz");
    std::fs::write(&path, b"nothing lofty can read").expect("write file");

    assert!(matches!(genre::set(&path, "Ambient"), Outcome::Failed(_)));
}

#[test]
fn the_background_run_reports_when_it_is_done() {
    let dir = common::scratch_dir("genre-background");
    let paths: Vec<PathBuf> = ["a.wav", "b.wav"].iter().map(|name| track(&dir, name)).collect();

    let done = genre::set_in_background(paths.clone(), "Breakcore".to_string());
    let report = done.recv_timeout(std::time::Duration::from_secs(10)).expect("a report");

    assert_eq!(report.written, 2);

    for path in &paths {
        assert_eq!(genre_of(path).as_deref(), Some("Breakcore"));
    }
}

#[test]
fn a_wav_takes_a_genre_even_though_it_cannot_take_a_picture() {
    let dir = common::scratch_dir("genre-wav");
    let path = common::write_wav(
        &dir,
        &WavSpec { name: "plain.wav", title: Some("Plain"), ..WavSpec::default() },
    );

    assert_eq!(genre::set(&path, "Jungle"), Outcome::Written);
    assert_eq!(genre_of(&path).as_deref(), Some("Jungle"));
}
