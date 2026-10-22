//! Fingerprints: what identifies a track once its path stops being true.

mod common;

use common::WavSpec;
use ogma::fingerprint;

/// A WAV of `millis` of audio under its own scratch directory.
fn track(name: &str, file: &str, millis: u64) -> std::path::PathBuf {
    let dir = common::scratch_dir(name);

    common::write_wav(&dir, &WavSpec { name: file, millis, ..WavSpec::default() })
}

#[test]
fn the_same_audio_fingerprints_the_same_wherever_it_sits() {
    let here = track("print-same", "here.wav", 400);
    let dir = common::scratch_dir("print-same-elsewhere");
    let there = dir.join("renamed-entirely.wav");

    std::fs::copy(&here, &there).expect("copy the track");

    let left = fingerprint::of(&here).expect("a fingerprint");
    let right = fingerprint::of(&there).expect("a fingerprint");

    assert_eq!(left, right, "the name and the folder play no part");
}

#[test]
fn different_audio_fingerprints_differently() {
    let dir = common::scratch_dir("print-different");

    let short = common::write_wav(&dir, &WavSpec { name: "a.wav", millis: 300, ..Default::default() });
    let long = common::write_wav(&dir, &WavSpec { name: "b.wav", millis: 900, ..Default::default() });

    assert_ne!(fingerprint::of(&short), fingerprint::of(&long));
}

#[test]
fn embedding_a_cover_leaves_the_fingerprint_alone() {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::prelude::AudioFile;
    use lofty::tag::{Tag, TagType};

    // The background fill writes artwork into files as it finds it, so a fingerprint that covered
    // the file as a whole would be invalidated by ogma itself on nearly every track it touched.
    let path = track("print-cover", "track.wav", 500);
    let before = fingerprint::of(&path).expect("a fingerprint");

    let mut tagged = lofty::probe::Probe::open(&path).expect("probe").read().expect("read");

    if tagged.tag(tagged.primary_tag_type()).is_none() {
        tagged.insert_tag(Tag::new(TagType::Id3v2));
    }

    tagged.primary_tag_mut().expect("a tag").push_picture(
        Picture::unchecked(common::png_gradient(64))
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Png)
            .build(),
    );

    tagged.save_to_path(&path, WriteOptions::default()).expect("write the cover in");

    assert!(
        std::fs::metadata(&path).expect("stat").len() > 0,
        "the file is still there to read"
    );
    assert_eq!(fingerprint::of(&path), Some(before), "the audio packets did not move");
}

#[test]
fn a_tag_written_after_the_fact_does_not_change_the_fingerprint() {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::prelude::{Accessor, AudioFile};
    use lofty::tag::{Tag, TagType};

    let path = track("print-write-tag", "track.wav", 500);
    let before = fingerprint::of(&path).expect("a fingerprint");

    let mut tagged = lofty::probe::Probe::open(&path).expect("probe").read().expect("read");
    let kind = tagged.primary_tag_type();

    if tagged.tag(kind).is_none() {
        tagged.insert_tag(Tag::new(TagType::Id3v2));
    }

    let tag = tagged.primary_tag_mut().expect("a tag to write into");
    tag.set_genre("Ambient".to_string());
    tag.set_title("Renamed In Place".to_string());

    tagged.save_to_path(&path, WriteOptions::default()).expect("write the tags back");

    assert_eq!(
        fingerprint::of(&path),
        Some(before),
        "tags live outside the audio packets, which is the point"
    );
}

#[test]
fn something_that_is_not_audio_has_no_fingerprint() {
    let dir = common::scratch_dir("print-not-audio");
    let path = dir.join("notes.txt");

    std::fs::write(&path, b"this is not a song").expect("write");

    assert_eq!(fingerprint::of(&path), None);
    assert_eq!(fingerprint::of(&dir.join("nothing-here.wav")), None, "nor does a missing file");
}

#[test]
fn a_stream_is_never_opened_as_a_file() {
    let url = std::path::PathBuf::from("https://www.youtube.com/watch?v=dQw4w9WgXcQ");

    assert_eq!(fingerprint::of(&url), None);
}

#[test]
fn a_fingerprint_survives_being_written_down() {
    let path = track("print-hex", "track.wav", 400);
    let print = fingerprint::of(&path).expect("a fingerprint");

    let hex = fingerprint::to_hex(print);

    assert_eq!(hex.len(), 32, "{hex}");
    assert_eq!(fingerprint::from_hex(&hex), Some(print));
    assert_eq!(fingerprint::from_hex("not a fingerprint"), None);
}
