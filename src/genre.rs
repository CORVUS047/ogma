//! Writing a genre into music files.
//!
//! The reading side lives in [`crate::meta`]; this is the half that writes. Two callers use it: the
//! interface, where a genre is typed in by hand, and [`crate::autofill`], which fills in a genre a
//! file is missing from what a service says about the release.
//!
//! A file is [claimed](crate::claim) before it is written, so that a genre typed in here and a
//! genre the filling found cannot be written to one file at the same moment.
//!
//! **This writes to the user's music files.** A genre typed in by hand replaces whatever was there —
//! that is what typing it means — while the automatic filling only ever writes into a file that has
//! no genre at all. The two are deliberately different: one is an instruction, the other a guess.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::{Accessor, ItemKey};
use lofty::probe::Probe;
use lofty::tag::{Tag, TagType};

/// Tag types a genre can be written to and read back from.
///
/// RIFF INFO and AIFF text chunks carry a genre as happily as ID3v2 does, so a WAV takes one even
/// though it cannot take a picture.
const GENRE_TAGS: &[TagType] = &[
    TagType::Id3v2,
    TagType::Mp4Ilst,
    TagType::VorbisComments,
    TagType::RiffInfo,
    TagType::AiffText,
];

/// What one file came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The genre was written in.
    Written,
    /// The file is in a format that cannot carry a genre.
    Unsupported,
    /// The file could not be read or written.
    Failed(String),
}

/// What a run over several files came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub written: usize,
    pub unsupported: usize,
    pub failed: usize,
}

impl Report {
    /// A sentence saying what happened, for the interface to show.
    pub fn summary(&self, genre: &str) -> String {
        let mut text = match self.written {
            0 => "nothing tagged".to_string(),
            1 => format!("genre {genre} on 1 track"),
            count => format!("genre {genre} on {count} tracks"),
        };

        if self.unsupported > 0 {
            text.push_str(&format!(" · {} cannot hold one", self.unsupported));
        }
        if self.failed > 0 {
            text.push_str(&format!(" · {} failed", self.failed));
        }

        text
    }
}

/// Write `genre` into `path`, replacing whatever genre was there.
pub fn set(path: &Path, genre: &str) -> Outcome {
    // The automatic filling writes to these same files, in this player and in any other interface
    // the user has open, so the file is claimed first. This one waits for its turn rather than
    // giving up at once: the user asked for this write by hand, and whatever holds the file is
    // busy with it for milliseconds.
    let Some(_claim) = crate::claim::Claim::waited_for(path) else {
        return Outcome::Failed("another player is writing to this file".to_string());
    };

    let mut tagged = match Probe::open(path).and_then(|probe| probe.read()) {
        Ok(tagged) => tagged,
        Err(err) => return Outcome::Failed(err.to_string()),
    };

    let Some(tag_type) = tag_type(&tagged) else {
        return Outcome::Unsupported;
    };

    // A file can carry more than one tag — a WAV holding both a RIFF INFO chunk and an ID3v2 one is
    // ordinary — and the new genre goes in only one of them. The old genre is taken out of all of
    // them first, or the file reads back with whichever tag the reader happened to prefer.
    let present: Vec<TagType> = tagged.tags().iter().map(|tag| tag.tag_type()).collect();

    for present in present {
        if let Some(tag) = tagged.tag_mut(present) {
            clear(tag);
        }
    }

    if tagged.tag(tag_type).is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }

    let Some(tag) = tagged.tag_mut(tag_type) else {
        return Outcome::Failed("no writable tag".to_string());
    };

    write(tag, genre);

    match tagged.save_to_path(path, WriteOptions::default()) {
        Ok(()) => Outcome::Written,
        Err(err) => Outcome::Failed(err.to_string()),
    }
}

/// Write `genre` into every file in `paths`.
pub fn set_all<I, P>(paths: I, genre: &str) -> Report
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut report = Report::default();

    for path in paths {
        match set(path.as_ref(), genre) {
            Outcome::Written => report.written += 1,
            Outcome::Unsupported => report.unsupported += 1,
            Outcome::Failed(_) => report.failed += 1,
        }
    }

    report
}

/// Write `genre` into every file in `paths`, on a thread of its own.
///
/// Each file is rewritten where it sits, which a folder's worth of them makes slow enough to freeze
/// the display; the interface polls the receiver instead and carries on drawing meanwhile. The
/// receiver yields once, when the run is done.
pub fn set_in_background(paths: Vec<PathBuf>, genre: String) -> Receiver<Report> {
    let (sender, receiver) = mpsc::channel();

    // A failed spawn leaves the receiver disconnected, which the caller reads as the run not having
    // started — the same as any other way it could fail.
    let _ = std::thread::Builder::new()
        .name("ogma-genre".to_string())
        .spawn(move || {
            let report = set_all(&paths, &genre);

            let _ = sender.send(report);
        });

    receiver
}

/// Set the genre on `tag`, leaving nothing of the old one behind.
///
/// Shared with [`crate::autofill`], which writes a genre into the same save as a cover rather than
/// opening the file a second time.
pub(crate) fn write(tag: &mut Tag, genre: &str) {
    clear(tag);
    tag.set_genre(genre.to_string());
}

/// Take every genre out of `tag`.
///
/// A tag can hold several genre items at once — ID3v2 allows it, and a file tagged elsewhere may well
/// use it — so the key is emptied rather than the first value overwritten.
fn clear(tag: &mut Tag) {
    tag.remove_genre();
    tag.remove_key(ItemKey::Genre);
}

/// The tag this file should carry its genre in, or `None` when its format cannot hold one.
///
/// The file's own primary tag is preferred, so a FLAC gets a Vorbis comment and an MP4 a `©gen`
/// atom. Only tag types the format can be *written* with are considered.
pub(crate) fn tag_type(tagged: &lofty::file::TaggedFile) -> Option<TagType> {
    let primary = tagged.primary_tag_type();

    if GENRE_TAGS.contains(&primary) && tagged.tag_support(primary).is_writable() {
        return Some(primary);
    }

    GENRE_TAGS
        .iter()
        .copied()
        .find(|tag_type| tagged.tag_support(*tag_type).is_writable())
}
