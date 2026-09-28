//! Filling in metadata a file is missing, from what is already on disk.
//!
//! Only artwork for now. Covers are looked for on disk first — an image sitting beside the music, or
//! another track in the same folder that already carries one — and then, if the user has asked for
//! it, from the internet. See [`online`] for what that contacts and what it sends.
//!
//! **This writes to the user's music files**, so it only ever runs when
//! [`Config::auto_fill_metadata`](crate::config::Config::auto_fill_metadata) is on, and it only ever
//! adds a picture to a file that has none. An existing picture is never replaced.
//!
//! **Looking online is a second, separate switch**
//! ([`Config::fetch_artwork_online`](crate::config::Config::fetch_artwork_online)), also off by
//! default: reading a file that is already on the machine and sending an album title to a third party
//! are different kinds of act, and consenting to the first is not consenting to the second.

pub mod online;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag, TagType};

use crate::meta;

/// What the filling is allowed to do.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// Whether to ask the internet when nothing on disk supplies a cover.
    pub online: bool,
}

impl Settings {
    /// Disk only, contacting nobody.
    pub fn local_only() -> Self {
        Settings { online: false }
    }

    /// Disk first, then the services in [`online`].
    pub fn with_online() -> Self {
        Settings { online: true }
    }
}

/// Base names an album cover conventionally uses, in the order they are preferred.
const COVER_NAMES: &[&str] = &[
    "cover", "folder", "front", "album", "albumart", "albumartsmall", "artwork", "art", "thumb",
];

/// Image extensions considered, lowercase.
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif"];

/// Pictures larger than this are left alone: embedding one would bloat every file in the album more
/// than a cover is worth.
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

/// How many siblings are probed while looking for art embedded in the same album.
const MAX_SIBLINGS_PROBED: usize = 24;

/// What happened to one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The file already had a picture, so it was left alone.
    AlreadyPresent,
    /// A picture was found and written into the file.
    Filled { from: PathBuf },
    /// Nothing on disk could supply a picture.
    NothingFound,
    /// The file's format cannot carry a picture, so nothing was written.
    Unsupported,
    /// The file could not be read or written.
    Failed(String),
}

/// What a run over several files came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub filled: usize,
    pub already_present: usize,
    pub nothing_found: usize,
    /// Files whose format cannot hold a picture.
    pub unsupported: usize,
    pub failed: usize,
}

/// Where a cover came from.
#[derive(Clone, Debug)]
struct Cover {
    /// The image itself.
    data: Vec<u8>,
    /// What supplied it, for reporting.
    source: PathBuf,
}

/// Add a cover to `path` if it has none and one can be found.
pub fn fill(path: &Path, settings: Settings) -> Outcome {
    let mut covers = CoverCache::default();

    fill_with(path, &mut covers, settings)
}

/// Add covers to everything in `paths` that is missing one.
///
/// Files are handled in the order given; grouping an album's tracks together lets the folder's cover
/// be found once and reused, so a library scan does not re-read the same image for every track.
pub fn fill_all<I, P>(paths: I, settings: Settings) -> Report
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut covers = CoverCache::default();
    let mut report = Report::default();

    for path in paths {
        match fill_with(path.as_ref(), &mut covers, settings) {
            Outcome::Filled { .. } => report.filled += 1,
            Outcome::AlreadyPresent => report.already_present += 1,
            Outcome::NothingFound => report.nothing_found += 1,
            Outcome::Unsupported => report.unsupported += 1,
            Outcome::Failed(_) => report.failed += 1,
        }
    }

    report
}

/// Fill in the background, reporting each file that gained a cover.
///
/// Reading and writing a whole library takes long enough that doing it on the interface's thread
/// would freeze the display, so it happens on its own thread and results arrive as they come. The
/// receiver simply ends when the work is done, or when it is dropped.
pub fn fill_in_background(paths: Vec<PathBuf>, settings: Settings) -> Receiver<PathBuf> {
    let (sender, receiver) = mpsc::channel();

    // A failed spawn is not worth reporting: the worst case is that no covers get filled in.
    let _ = std::thread::Builder::new()
        .name("ogma-autofill".to_string())
        .spawn(move || {
            let mut covers = CoverCache::default();

            for path in paths {
                if let Outcome::Filled { .. } = fill_with(&path, &mut covers, settings) {
                    // A closed channel means the application has moved on.
                    if sender.send(path).is_err() {
                        return;
                    }
                }
            }
        });

    receiver
}

/// Covers already found, so an album is searched once rather than once per track.
///
/// Both halves remember misses as well as hits: without that, an album with no art anywhere would
/// mean one fruitless round of network requests per track.
#[derive(Default)]
struct CoverCache {
    by_folder: HashMap<PathBuf, Option<Cover>>,
    by_release: HashMap<online::Query, Option<Cover>>,
}

impl CoverCache {
    /// A cover from the folder the music sits in.
    fn for_folder(&mut self, folder: &Path) -> Option<&Cover> {
        if !self.by_folder.contains_key(folder) {
            let found = find_sidecar(folder).or_else(|| find_in_siblings(folder));
            self.by_folder.insert(folder.to_path_buf(), found);
        }

        self.by_folder.get(folder).and_then(Option::as_ref)
    }

    /// A cover from the internet, for the release the tags describe.
    fn for_release(&mut self, query: online::Query) -> Option<&Cover> {
        if !self.by_release.contains_key(&query) {
            let found = online::cover_for(&query).map(|found| Cover {
                data: found.data,
                source: PathBuf::from(found.source),
            });

            self.by_release.insert(query.clone(), found);
        }

        self.by_release.get(&query).and_then(Option::as_ref)
    }
}

fn fill_with(path: &Path, covers: &mut CoverCache, settings: Settings) -> Outcome {
    // Reading the tags is the only way to know whether a picture is there already.
    let mut tagged = match Probe::open(path).and_then(|probe| probe.read()) {
        Ok(tagged) => tagged,
        Err(err) => return Outcome::Failed(err.to_string()),
    };

    let has_picture = tagged.tags().iter().any(|tag| tag.picture_count() > 0);
    if has_picture {
        return Outcome::AlreadyPresent;
    }

    // Formats that cannot hold a picture are left exactly as they are. A WAV's RIFF INFO chunk and
    // an AIFF's text chunks store text only, and forcing some other tag into the container to carry
    // art would be rewriting the file into a shape the user did not ask for.
    let Some(tag_type) = picture_tag_type(&tagged) else {
        return Outcome::Unsupported;
    };

    // Disk first: it is instant, it is certain to be the right album, and it asks nobody anything.
    let local = path.parent().and_then(|folder| covers.for_folder(folder).cloned());

    let cover = match local {
        Some(cover) => cover,
        None if settings.online => {
            let query = release_query(&tagged);

            match covers.for_release(query).cloned() {
                Some(cover) => cover,
                None => return Outcome::NothingFound,
            }
        }
        None => return Outcome::NothingFound,
    };

    // `from_reader` sniffs the media type from the bytes, which also rejects anything that is not an
    // image the tag formats can carry.
    let mut picture = match Picture::from_reader(&mut cover.data.as_slice()) {
        Ok(picture) => picture,
        Err(err) => return Outcome::Failed(err.to_string()),
    };
    picture.set_pic_type(PictureType::CoverFront);

    // A file with no tag at all needs one before a picture can go in it.
    if tagged.tag(tag_type).is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }

    let Some(tag) = tagged.tag_mut(tag_type) else {
        return Outcome::Failed("no writable tag".to_string());
    };

    tag.push_picture(picture);

    match tagged.save_to_path(path, WriteOptions::default()) {
        Ok(()) => Outcome::Filled { from: cover.source },
        Err(err) => Outcome::Failed(err.to_string()),
    }
}

/// The tag this file should carry a picture in, or `None` when its format cannot hold one.
///
/// The file's own primary tag is preferred, so a FLAC gets a Vorbis comment and an MP4 a `covr`
/// atom. Only tag types the format can be *written* with are considered — lofty will read an ID3v2
/// tag out of a FLAC, for instance, but must not write one back.
fn picture_tag_type(tagged: &lofty::file::TaggedFile) -> Option<TagType> {
    /// Tag types whose pictures this player can both write and read back.
    ///
    /// ID3v1, RIFF INFO and AIFF text chunks cannot hold a picture at all. APE tags nominally can,
    /// but only as undifferentiated binary items that reading does not reconstruct, so a picture
    /// written there would be invisible — and the file would be rewritten on every scan. Files whose
    /// format offers none of these are left exactly as they are.
    const PICTURE_TAGS: &[TagType] = &[TagType::Id3v2, TagType::Mp4Ilst, TagType::VorbisComments];

    let primary = tagged.primary_tag_type();

    if PICTURE_TAGS.contains(&primary) && tagged.tag_support(primary).is_writable() {
        return Some(primary);
    }

    // Otherwise any picture-capable tag the format accepts will do.
    PICTURE_TAGS
        .iter()
        .copied()
        .find(|tag_type| tagged.tag_support(*tag_type).is_writable())
}

/// What the file's tags say about the release, for asking a service about it.
fn release_query(tagged: &lofty::file::TaggedFile) -> online::Query {
    // The primary tag is the authority: a file carrying both an ID3v2 tag and a RIFF INFO chunk must
    // not have the lookup pick whichever happened to be parsed first.
    let primary = tagged.primary_tag_type();

    let text = |key: ItemKey| -> Option<String> {
        tagged
            .tag(primary)
            .and_then(|tag| tag.get_string(key))
            .or_else(|| tagged.tags().iter().find_map(|tag| tag.get_string(key)))
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_owned())
    };

    online::Query {
        // The album artist names the release; the track artist stands in on a single-artist album.
        artist: text(ItemKey::AlbumArtist).or_else(|| text(ItemKey::TrackArtist)),
        album: text(ItemKey::AlbumTitle),
        release_mbid: text(ItemKey::MusicBrainzReleaseId),
    }
}

/// An image file sitting in the folder under one of the conventional names.
fn find_sidecar(folder: &Path) -> Option<Cover> {
    let entries: Vec<PathBuf> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect();

    // Conventional names in order of preference, so `cover.jpg` wins over `art.png`.
    for name in COVER_NAMES {
        for path in &entries {
            if !is_named_image(path, name) {
                continue;
            }

            if let Some(cover) = read_image(path) {
                return Some(cover);
            }
        }
    }

    None
}

/// Whether `path` is an image file whose stem is `name`, ignoring case.
fn is_named_image(path: &Path, name: &str) -> bool {
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };

    if !stem.eq_ignore_ascii_case(name) {
        return false;
    }

    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.as_str()))
}

fn read_image(path: &Path) -> Option<Cover> {
    let size = std::fs::metadata(path).ok()?.len();

    // Empty or enormous files are not usable covers.
    if size == 0 || size > MAX_IMAGE_BYTES {
        return None;
    }

    Some(Cover { data: std::fs::read(path).ok()?, source: path.to_path_buf() })
}

/// Art already embedded in another track in the same folder.
///
/// Albums are usually tagged as a set, so one track carrying a cover means the rest can share it.
fn find_in_siblings(folder: &Path) -> Option<Cover> {
    let mut siblings: Vec<PathBuf> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| meta::has_audio_extension(path))
        .collect();

    siblings.sort();

    for sibling in siblings.iter().take(MAX_SIBLINGS_PROBED) {
        let Ok(file) = meta::probe(sibling) else {
            continue;
        };

        if let Some(art) = file.front_cover()
            && !art.data.is_empty()
            && art.data.len() as u64 <= MAX_IMAGE_BYTES
        {
            return Some(Cover { data: art.data.clone(), source: sibling.clone() });
        }
    }

    None
}
