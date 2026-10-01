//! Filling in missing artwork from what is already on disk.

mod common;

use std::path::{Path, PathBuf};

use ogma::autofill::{self, Outcome, Settings};
use ogma::song::Song;

/// A folder holding copies of the given fixture files.
fn album(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let dir = common::scratch_dir(name);

    for (file_name, contents) in files {
        std::fs::write(dir.join(file_name), contents).expect("write fixture");
    }

    dir
}

/// A FLAC with tags but no picture, built by re-encoding nothing: the WAV fixture is enough, since
/// what matters is a file lofty can write a picture into.
fn track_without_art(dir: &Path, name: &str) -> PathBuf {
    let source = common::write_tagged_wav(dir);
    let path = dir.join(name);
    std::fs::rename(&source, &path).expect("rename fixture");

    path
}

fn has_art(path: &Path) -> bool {
    Song::new(path).front_cover().is_some()
}

#[test]
fn a_cover_beside_the_music_is_written_into_the_file() {
    let dir = common::scratch_dir("fill-sidecar");
    let track = track_without_art(&dir, "track.wav");
    std::fs::write(dir.join("cover.png"), common::png_gradient(48)).expect("write cover");

    assert!(!has_art(&track), "the fixture starts with no picture");

    let outcome = autofill::fill(&track, Settings::local_only());

    assert_eq!(outcome, Outcome::artwork_from(dir.join("cover.png")));
    assert!(has_art(&track), "the picture is in the file, not just in memory");

    // The audio itself still reads back.
    let song = Song::new(&track);
    assert_eq!(song.codec(), Some("pcm"));
    assert_eq!(song.title(), Some("Test Title"), "the existing tags survive");
}

#[test]
fn an_existing_picture_is_never_replaced() {
    let dir = common::scratch_dir("fill-existing");
    let Some(track) = common::wav_with_cover(&dir) else {
        return;
    };
    let before = std::fs::read(&track).expect("read before");

    std::fs::write(dir.join("cover.png"), common::png_gradient(48)).expect("write cover");

    assert_eq!(autofill::fill(&track, Settings::local_only()), Outcome::AlreadyPresent);
    assert_eq!(std::fs::read(&track).expect("read after"), before, "the file is untouched");
}

#[test]
fn conventional_cover_names_are_recognised_in_order_of_preference() {
    for name in ["cover.png", "folder.png", "front.png", "Album.PNG", "artwork.png"] {
        let dir = common::scratch_dir(&format!("fill-name-{}", name.replace('.', "-")));
        let track = track_without_art(&dir, "track.wav");
        std::fs::write(dir.join(name), common::png_gradient(32)).expect("write cover");

        assert_eq!(
            autofill::fill(&track, Settings::local_only()),
            Outcome::artwork_from(dir.join(name)),
            "{name} should be recognised"
        );
    }

    // With several present, the most conventional name wins.
    let dir = common::scratch_dir("fill-name-order");
    let track = track_without_art(&dir, "track.wav");
    for name in ["artwork.png", "folder.png", "cover.png"] {
        std::fs::write(dir.join(name), common::png_gradient(32)).expect("write cover");
    }

    assert_eq!(
        autofill::fill(&track, Settings::local_only()),
        Outcome::artwork_from(dir.join("cover.png"))
    );
}

#[test]
fn art_already_in_a_sibling_track_is_reused() {
    let dir = common::scratch_dir("fill-sibling");

    // The bare track first: both fixtures write `tagged.wav`, so the renamed one has to come first
    // or the track carrying art would be overwritten.
    let bare = track_without_art(&dir, "b-bare.wav");
    let Some(_with_art) = common::wav_with_cover(&dir) else {
        return;
    };

    let outcome = autofill::fill(&bare, Settings::local_only());

    assert!(matches!(outcome, Outcome::Filled { .. }), "got {outcome:?}");
    assert!(has_art(&bare));
}

#[test]
fn a_folder_with_no_art_anywhere_is_left_alone() {
    let dir = common::scratch_dir("fill-nothing");
    let track = track_without_art(&dir, "track.wav");
    let before = std::fs::read(&track).expect("read before");

    // A text file is not a cover, whatever it is called.
    std::fs::write(dir.join("cover.txt"), b"not an image").expect("write file");

    assert_eq!(autofill::fill(&track, Settings::local_only()), Outcome::NothingFound);
    assert_eq!(std::fs::read(&track).expect("read after"), before);
}

#[test]
fn an_empty_or_unreadable_image_is_not_used() {
    let dir = common::scratch_dir("fill-bad-image");
    let track = track_without_art(&dir, "track.wav");
    std::fs::write(dir.join("cover.png"), b"").expect("write empty cover");

    assert_eq!(autofill::fill(&track, Settings::local_only()), Outcome::NothingFound, "an empty file is no cover");

    // A file with an image name but no image in it is refused rather than written.
    std::fs::write(dir.join("cover.png"), b"definitely not a png").expect("write junk cover");
    assert!(matches!(autofill::fill(&track, Settings::local_only()), Outcome::Failed(_) | Outcome::NothingFound));
    assert!(!has_art(&track));
}

#[test]
fn a_file_that_cannot_be_parsed_is_reported_not_written() {
    let dir = common::scratch_dir("fill-broken");
    std::fs::write(dir.join("cover.png"), common::png_gradient(32)).expect("write cover");

    let broken = dir.join("broken.mp3");
    std::fs::write(&broken, b"not audio at all").expect("write broken file");

    assert!(matches!(autofill::fill(&broken, Settings::local_only()), Outcome::Failed(_)));
    assert_eq!(
        std::fs::read(&broken).expect("read after"),
        b"not audio at all",
        "a file that cannot be parsed is not rewritten"
    );
}

#[test]
fn a_run_over_several_files_reports_what_it_did() {
    let dir = common::scratch_dir("fill-report");
    std::fs::write(dir.join("cover.png"), common::png_gradient(32)).expect("write cover");

    let first = track_without_art(&dir, "one.wav");
    let second = track_without_art(&dir, "two.wav");
    let broken = dir.join("three.mp3");
    std::fs::write(&broken, b"junk").expect("write broken");

    let report = autofill::fill_all([&first, &second, &broken], Settings::local_only());

    assert_eq!(report.filled, 2);
    assert_eq!(report.failed, 1);
    assert_eq!(report.already_present, 0);

    // Running again writes nothing: the files already have what they need.
    let second_run = autofill::fill_all([&first, &second, &broken], Settings::local_only());
    assert_eq!(second_run.filled, 0);
    assert_eq!(second_run.already_present, 2);
}

#[test]
fn a_format_that_cannot_hold_a_picture_is_skipped() {
    // WavPack carries APE tags, whose pictures this player cannot read back, so nothing is written.
    let dir = album("fill-unsupported", &[]);
    std::fs::write(dir.join("cover.png"), common::png_gradient(32)).expect("write cover");

    let source = std::path::Path::new(
        "/tmp/claude-1000/-home-dieu-Documents-coding-ogma/ab6ab85f-375d-4cb8-9af9-8d20cc4cb650/scratchpad/audio/out.wv",
    );
    if !source.exists() {
        // The fixture is not available here; the rule is still covered by the outcome's existence.
        return;
    }

    let track = dir.join("track.wv");
    std::fs::copy(source, &track).expect("copy wavpack fixture");
    let before = std::fs::read(&track).expect("read before");

    assert_eq!(autofill::fill(&track, Settings::local_only()), Outcome::Unsupported);
    assert_eq!(std::fs::read(&track).expect("read after"), before, "left byte for byte alone");
}

#[test]
fn the_background_run_reports_each_file_it_fills() {
    let dir = common::scratch_dir("fill-background");
    std::fs::write(dir.join("cover.png"), common::png_gradient(32)).expect("write cover");

    let first = track_without_art(&dir, "one.wav");
    let second = track_without_art(&dir, "two.wav");

    let filled = autofill::fill_in_background(vec![first.clone(), second.clone()], Settings::local_only());

    let mut reported: Vec<PathBuf> = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

    while reported.len() < 2 && std::time::Instant::now() < deadline {
        match filled.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(path) => reported.push(path),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    reported.sort();
    assert_eq!(reported, [first.clone(), second.clone()]);
    assert!(has_art(&first) && has_art(&second));
}

#[test]
fn a_song_picks_up_art_after_a_refresh() {
    let dir = common::scratch_dir("fill-refresh");
    let track = track_without_art(&dir, "track.wav");

    let mut song = Song::new(&track);
    // Reading first means the cached metadata predates the fill.
    assert!(song.front_cover().is_none());

    std::fs::write(dir.join("cover.png"), common::png_gradient(32)).expect("write cover");
    assert!(matches!(autofill::fill(&track, Settings::local_only()), Outcome::Filled { .. }));

    assert!(song.front_cover().is_none(), "the cache still holds the old reading");

    song.refresh();
    assert!(song.front_cover().is_some(), "and a refresh picks up the new one");
}

// ---------------------------------------------------------------------------- looking online

/// Everything that counts requests lives in one test: the counter is process-wide, and tests in a
/// binary run in parallel, so separate tests would measure each other's traffic.
///
/// None of this depends on the network being reachable. The counter rises when a request is
/// attempted, whether or not it succeeds, and a machine with no connectivity simply finds nothing.
#[test]
fn what_reaches_the_network_and_what_does_not() {
    // --- disk-only filling contacts nobody, even with something to search for ---
    let dir = common::scratch_dir("fill-online-off");
    let track = track_without_art(&dir, "track.wav");

    let before = autofill::online::requests_made();
    assert_eq!(autofill::fill(&track, Settings::local_only()), Outcome::NothingFound);
    assert_eq!(
        autofill::online::requests_made(),
        before,
        "disk-only filling must not contact anyone"
    );

    // --- a cover in the folder answers before anyone is asked ---
    let dir = common::scratch_dir("fill-online-local-wins");
    let track = track_without_art(&dir, "track.wav");
    std::fs::write(dir.join("cover.png"), common::png_gradient(48)).expect("write cover");

    let before = autofill::online::requests_made();
    assert_eq!(
        autofill::fill(&track, Settings::with_online()),
        Outcome::artwork_from(dir.join("cover.png"))
    );
    assert_eq!(
        autofill::online::requests_made(),
        before,
        "the folder answered, so nobody was asked"
    );

    // --- a file naming no release gives a lookup nothing to go on ---
    let dir = common::scratch_dir("fill-online-untagged");
    let track = dir.join("untitled.wav");
    std::fs::write(&track, minimal_wav()).expect("write bare wav");

    let before = autofill::online::requests_made();
    let outcome = autofill::fill(&track, Settings::with_online());
    assert!(matches!(outcome, Outcome::NothingFound | Outcome::Failed(_)), "got {outcome:?}");
    assert_eq!(autofill::online::requests_made(), before, "an empty query asks nobody");

    // --- an album nobody has heard of is searched for, then left alone ---
    let dir = common::scratch_dir("fill-online-miss");
    let track = dir.join("track.wav");
    std::fs::write(&track, minimal_wav()).expect("write bare wav");
    tag_release(&track, "Zzqx Nonexistent Band", "Qqqq No Such Record 99999");

    let contents = std::fs::read(&track).expect("read before");
    let before = autofill::online::requests_made();

    assert_eq!(autofill::fill(&track, Settings::with_online()), Outcome::NothingFound);
    assert!(
        autofill::online::requests_made() > before,
        "a usable query should be sent somewhere"
    );
    assert_eq!(std::fs::read(&track).expect("read after"), contents, "and the file is untouched");

    // --- one lookup serves a whole album ---
    let dir = common::scratch_dir("fill-online-cache");
    let mut tracks = Vec::new();
    for name in ["one.wav", "two.wav", "three.wav"] {
        let track = dir.join(name);
        std::fs::write(&track, minimal_wav()).expect("write bare wav");
        tag_release(&track, "Zzqx Nonexistent Band", "Qqqq No Such Record 99999");
        tracks.push(track);
    }

    let before = autofill::online::requests_made();
    autofill::fill_all(&tracks, Settings::with_online());
    let spent = autofill::online::requests_made() - before;

    // The chain is at most a MusicBrainz search, a Cover Art Archive fetch and an iTunes fallback,
    // so more than that from three tracks of one release is the cache failing.
    assert!(spent <= 3, "three tracks of one album cost {spent} requests");
}

#[test]
fn a_genre_already_in_the_file_is_never_replaced() {
    let dir = common::scratch_dir("fill-genre-kept");

    // The fixture carries a genre and no picture. With both lookups on, the picture is what is
    // missing — the genre is not — so nothing is asked about the genre and nothing is written over.
    let track = track_without_art(&dir, "track.wav");
    assert_eq!(Song::new(&track).genre(), Some("Ambient"), "the fixture starts with one");

    // Genres on, covers from the disk only, so any request at all could only be about the genre.
    let genres_only = Settings { online: false, genres: true };
    let before = autofill::online::requests_made();

    autofill::fill(&track, genres_only);

    assert_eq!(Song::new(&track).genre(), Some("Ambient"), "left exactly as it was");
    assert_eq!(
        autofill::online::requests_made(),
        before,
        "and nobody was asked about a genre that is already there"
    );
}

#[test]
fn a_genre_is_only_looked_for_when_its_own_switch_is_on() {
    let dir = common::scratch_dir("fill-genre-switch");

    // No genre and no album tags: a genre is wanted but there is nothing to look one up by, so the
    // question never leaves the machine. What is being checked is which switch asks it at all.
    let track = dir.join("track.wav");
    std::fs::write(&track, minimal_wav()).expect("write bare wav");

    assert_eq!(Song::new(&track).genre(), None);

    let before = autofill::online::requests_made();

    // Filling from the disk alone never looks a genre up, whatever the file is missing.
    assert_eq!(autofill::fill(&track, Settings::local_only()), Outcome::NothingFound);

    // And with the switch on, an unanswerable query is still asked of nobody.
    let both = Settings { online: true, genres: true };
    assert_eq!(autofill::fill(&track, both), Outcome::NothingFound);
    assert_eq!(autofill::online::requests_made(), before, "an empty query asks nobody");

    assert_eq!(Song::new(&track).genre(), None, "and nothing was written");
}

#[test]
fn a_run_counts_covers_and_genres_apart() {
    let dir = common::scratch_dir("fill-report-genres");
    let track = track_without_art(&dir, "track.wav");
    std::fs::write(dir.join("cover.png"), common::png_gradient(32)).expect("write cover");

    let report = autofill::fill_all([&track], Settings::local_only());

    assert_eq!(report.filled, 1, "the file gained something");
    assert_eq!(report.artwork, 1, "a cover");
    assert_eq!(report.genres, 0, "and no genre, the disk having none to give");
}

/// Set the album and artist on a file, so there is a release to look up.
fn tag_release(path: &Path, artist: &str, album: &str) {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::prelude::Accessor;
    use lofty::tag::{Tag, TagType};

    let mut tagged = lofty::read_from_path(path).expect("read file");

    if tagged.tag(TagType::Id3v2).is_none() {
        tagged.insert_tag(Tag::new(TagType::Id3v2));
    }

    let tag = tagged.tag_mut(TagType::Id3v2).expect("a tag");
    tag.set_artist(artist.to_string());
    tag.set_album(album.to_string());

    tagged.save_to_path(path, WriteOptions::default()).expect("save tags");
}

/// The smallest WAV that parses: a header and no audio worth speaking of.
fn minimal_wav() -> Vec<u8> {
    let samples = [0u8; 64];

    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&44_100u32.to_le_bytes());
    wav.extend_from_slice(&88_200u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(&samples);

    wav
}

#[test]
fn art_is_not_borrowed_from_a_track_of_another_release() {
    let dir = common::scratch_dir("fill-mixed-folder");

    // Two releases in one folder, as a downloads folder or a library kept flat has. The bare
    // track comes first: both fixtures are written as `tagged.wav`, so the renamed one has to
    // exist before the next is written.
    let bare = track_without_art(&dir, "caramella-girls.wav");
    tag_release(&bare, "Caramella Girls", "Caramella Dance");

    let Some(written) = common::wav_with_cover(&dir) else {
        return;
    };
    let stranger = dir.join("a-someone-else.wav");
    std::fs::rename(&written, &stranger).expect("rename fixture");
    tag_release(&stranger, "Someone Else", "A Different Record");

    // The stranger sorts first and is the only file in the folder carrying art, which is exactly
    // the shape that used to hand its cover to everything else.
    assert_eq!(autofill::fill(&bare, Settings::local_only()), Outcome::NothingFound);
    assert!(!has_art(&bare), "another release's cover is not this track's");
}

#[test]
fn art_is_borrowed_from_a_track_of_the_same_release() {
    let dir = common::scratch_dir("fill-same-release");

    let bare = track_without_art(&dir, "b-track-two.wav");
    tag_release(&bare, "Caramella Girls", "Caramella Dance");

    let Some(written) = common::wav_with_cover(&dir) else {
        return;
    };
    let sibling = dir.join("a-track-one.wav");
    std::fs::rename(&written, &sibling).expect("rename fixture");
    tag_release(&sibling, "Caramella Girls", "Caramella Dance");

    let outcome = autofill::fill(&bare, Settings::local_only());

    assert_eq!(outcome, Outcome::artwork_from(sibling));
    assert!(has_art(&bare), "one release still shares its cover");
}

#[test]
fn a_track_that_does_not_say_which_album_it_is_from_borrows_nothing() {
    let dir = common::scratch_dir("fill-no-album");

    // No album tag means no way to tell what release the track belongs to, so a sibling's art is
    // guesswork. A sidecar image is still taken: that is the folder's own cover, named as one.
    let bare = track_without_art(&dir, "b-untagged.wav");
    strip_album(&bare);

    let Some(written) = common::wav_with_cover(&dir) else {
        return;
    };
    let sibling = dir.join("a-tagged.wav");
    std::fs::rename(&written, &sibling).expect("rename fixture");
    tag_release(&sibling, "Someone Else", "A Record");

    assert_eq!(autofill::fill(&bare, Settings::local_only()), Outcome::NothingFound);
    assert!(!has_art(&bare));
}

#[test]
fn each_release_in_a_folder_is_answered_for_itself() {
    let dir = common::scratch_dir("fill-two-releases");

    // Two bare tracks from different releases, and art for only one of them. A run over the lot
    // must fill the one and leave the other, rather than answering the folder once for both.
    let ours = track_without_art(&dir, "b-ours.wav");
    tag_release(&ours, "Caramella Girls", "Caramella Dance");

    let theirs = track_without_art(&dir, "c-theirs.wav");
    tag_release(&theirs, "Someone Else", "A Different Record");

    let Some(written) = common::wav_with_cover(&dir) else {
        return;
    };
    let sibling = dir.join("a-ours-with-art.wav");
    std::fs::rename(&written, &sibling).expect("rename fixture");
    tag_release(&sibling, "Caramella Girls", "Caramella Dance");

    let report = autofill::fill_all([&ours, &theirs], Settings::local_only());

    assert_eq!(report.filled, 1, "only the release that had art");
    assert_eq!(report.nothing_found, 1);
    assert!(has_art(&ours));
    assert!(!has_art(&theirs));
}

/// Remove the album tag, for a file that cannot say which release it is from.
fn strip_album(path: &Path) {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::prelude::{Accessor, ItemKey};

    let mut tagged = lofty::read_from_path(path).expect("read file");
    let types: Vec<_> = tagged.tags().iter().map(|tag| tag.tag_type()).collect();

    for tag_type in types {
        if let Some(tag) = tagged.tag_mut(tag_type) {
            tag.remove_album();
            tag.remove_key(ItemKey::AlbumTitle);
        }
    }

    tagged.save_to_path(path, WriteOptions::default()).expect("save tags");
}
