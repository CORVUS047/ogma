//! Filling in metadata a file is missing, from what is already on disk.
//!
//! Only artwork for now. Covers are looked for on disk first — an image sitting beside the music, or
//! another track in the same folder *from the same release* that already carries one — and then, if
//! the user has asked for it, from the internet. See [`online`] for what that contacts and what it
//! sends.
//!
//! A folder is not a release. One can hold a whole library kept flat, or every YouTube download
//! ever made, so art is only borrowed between tracks whose tags agree on which album they are from
//! ([`Release`]); a track that does not say borrows nothing. Guessing from the folder is how one
//! song's cover ends up on a dozen unrelated ones.
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

use crate::genre;
use crate::meta;

/// What the filling is allowed to do.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// Whether to ask the internet for a cover when nothing on disk supplies one.
    pub online: bool,
    /// Whether to ask the internet for the genre of a file that has none.
    ///
    /// Its own switch rather than part of [`Settings::online`]: a genre can only come from a
    /// service, so someone who wants covers from the disk alone and no lookups at all must be able
    /// to say so without that deciding the other question for them.
    pub genres: bool,
}

impl Settings {
    /// Disk only, contacting nobody.
    pub fn local_only() -> Self {
        Settings { online: false, genres: false }
    }

    /// Disk first, then the services in [`online`], for covers and genres both.
    pub fn with_online() -> Self {
        Settings { online: true, genres: true }
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
    /// Nothing was missing that the filling had been asked to supply, so the file was left alone.
    AlreadyPresent,
    /// Something was written into the file: a cover, a genre, or both.
    Filled {
        /// What supplied the cover, when one was written.
        artwork: Option<PathBuf>,
        /// The genre written in, when one was.
        genre: Option<String>,
    },
    /// Nothing could be found to fill in what was missing.
    NothingFound,
    /// The file's format can carry neither a picture nor a genre, so nothing was written.
    Unsupported,
    /// The file could not be read or written.
    Failed(String),
}

impl Outcome {
    /// A cover written in from `source`, and nothing else: what a fill from the disk comes to.
    pub fn artwork_from(source: impl Into<PathBuf>) -> Self {
        Outcome::Filled { artwork: Some(source.into()), genre: None }
    }
}

/// What a run over several files came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Files that gained something, be it a cover or a genre.
    pub filled: usize,
    /// Files that gained a cover, counted again here so the two can be told apart.
    pub artwork: usize,
    /// Files that gained a genre.
    pub genres: usize,
    pub already_present: usize,
    pub nothing_found: usize,
    /// Files whose format can hold neither.
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

/// What marks two tracks as belonging to the same release.
///
/// Art is only ever borrowed between tracks that agree on this. A folder is not a release: a
/// downloads folder, or a library kept flat, holds as many releases as it has tracks, and a cover
/// from the wrong one is worse than no cover at all.
///
/// Text is folded to lowercase and trimmed, so tags that differ only in case or spacing still name
/// one release.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Release {
    /// The album title. A file with none has no release identity, and so never borrows art.
    album: String,
    /// The album artist, when the file names one. The *track* artist is deliberately not used: a
    /// compilation is one release with a different artist on every track.
    artist: Option<String>,
    /// The MusicBrainz release id, when the file carries one.
    mbid: Option<String>,
}

impl Release {
    /// The release `tagged` describes, or `None` when it does not say which album it is from.
    fn of_tagged(tagged: &lofty::file::TaggedFile) -> Option<Self> {
        let primary = tagged.primary_tag_type();

        let text = |key: ItemKey| -> Option<String> {
            tagged
                .tag(primary)
                .and_then(|tag| tag.get_string(key))
                .or_else(|| tagged.tags().iter().find_map(|tag| tag.get_string(key)))
                .and_then(normalise)
        };

        Some(Release {
            album: text(ItemKey::AlbumTitle)?,
            artist: text(ItemKey::AlbumArtist),
            mbid: text(ItemKey::MusicBrainzReleaseId),
        })
    }

    /// The release a probed file is from, or `None` when its tags do not say.
    fn of_file(file: &dyn meta::AudioFile) -> Option<Self> {
        Some(Release {
            album: file.album().and_then(normalise)?,
            artist: file.album_artist().and_then(normalise),
            mbid: file.musicbrainz_release_id().and_then(normalise),
        })
    }

    /// Whether `other` is the same release, as far as the two files' tags can say.
    ///
    /// Two MusicBrainz ids settle it outright. Otherwise the album titles must agree, and so must
    /// the album artists when both files name one — a file that names none is taken at its album
    /// title, the two being in one folder already.
    fn matches(&self, other: &Self) -> bool {
        if let (Some(mine), Some(theirs)) = (&self.mbid, &other.mbid) {
            return mine == theirs;
        }

        if self.album != other.album {
            return false;
        }

        match (&self.artist, &other.artist) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => true,
        }
    }
}

/// A tag value as a release is identified by it: trimmed, lowercased, and `None` when it is blank.
fn normalise(value: &str) -> Option<String> {
    let trimmed = value.trim();

    (!trimmed.is_empty()).then(|| trimmed.to_lowercase())
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
            Outcome::Filled { artwork, genre } => {
                report.filled += 1;
                report.artwork += usize::from(artwork.is_some());
                report.genres += usize::from(genre.is_some());
            }
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
/// Every part remembers misses as well as hits: without that, an album with no art anywhere would
/// mean one fruitless round of network requests per track.
///
/// Art borrowed from a sibling track is remembered per folder *and* release, not per folder: one
/// folder can hold several releases, and each must be answered for itself.
#[derive(Default)]
struct CoverCache {
    sidecars: HashMap<PathBuf, Option<Cover>>,
    siblings: HashMap<(PathBuf, Release), Option<Cover>>,
    /// What the services said about a release, and what they have been asked for so far: a file
    /// missing only a genre must not be answered from a lookup that only went after a cover.
    online: HashMap<online::Query, (online::Answer, online::Wanted)>,
}

impl CoverCache {
    /// An image file sitting beside the music under a conventional cover name.
    fn sidecar(&mut self, folder: &Path) -> Option<&Cover> {
        if !self.sidecars.contains_key(folder) {
            let found = find_sidecar(folder);
            self.sidecars.insert(folder.to_path_buf(), found);
        }

        self.sidecars.get(folder).and_then(Option::as_ref)
    }

    /// Art already embedded in another track of the same release, in the same folder.
    fn sibling(&mut self, folder: &Path, release: &Release) -> Option<&Cover> {
        let key = (folder.to_path_buf(), release.clone());

        if !self.siblings.contains_key(&key) {
            let found = find_in_siblings(folder, release);
            self.siblings.insert(key.clone(), found);
        }

        self.siblings.get(&key).and_then(Option::as_ref)
    }

    /// What the services say about a release: a cover, a genre, or both, as `wanted` asks.
    ///
    /// Anything already known is kept and only what has not been asked for yet is looked up, so an
    /// album whose tracks are missing different things still costs one round of requests.
    fn online(&mut self, query: online::Query, wanted: online::Wanted) -> &online::Answer {
        let (answer, asked) = self
            .online
            .entry(query.clone())
            .or_insert_with(|| (online::Answer::default(), online::Wanted::default()));

        let missing = online::Wanted {
            artwork: wanted.artwork && !asked.artwork,
            genre: wanted.genre && !asked.genre,
        };

        if missing.any() {
            asked.artwork |= wanted.artwork;
            asked.genre |= wanted.genre;

            let found = online::look_up(&query, missing);
            answer.absorb(found);
        }

        &self.online[&query].0
    }
}

fn fill_with(path: &Path, covers: &mut CoverCache, settings: Settings) -> Outcome {
    // Reading the tags is the only way to know what is missing.
    let mut tagged = match Probe::open(path).and_then(|probe| probe.read()) {
        Ok(tagged) => tagged,
        Err(err) => return Outcome::Failed(err.to_string()),
    };

    // What is already in the file is never replaced — not a picture, and not a genre. A genre that
    // is there was either tagged by whoever made the file or typed in by hand, and a guess from a
    // service does not get to overrule either.
    let wants_picture = !tagged.tags().iter().any(|tag| tag.picture_count() > 0);
    let wants_genre = settings.genres
        && !tagged.tags().iter().any(|tag| {
            tag.get_string(ItemKey::Genre).is_some_and(|genre| !genre.trim().is_empty())
        });

    if !wants_picture && !wants_genre {
        return Outcome::AlreadyPresent;
    }

    // Formats that can hold neither are left exactly as they are. A WAV's RIFF INFO chunk and an
    // AIFF's text chunks store text only — a genre fits, a picture does not — and forcing another
    // tag into the container to carry art would be rewriting the file into a shape the user did
    // not ask for.
    let picture_tag = wants_picture.then(|| picture_tag_type(&tagged)).flatten();
    let genre_tag = wants_genre.then(|| genre::tag_type(&tagged)).flatten();

    if picture_tag.is_none() && genre_tag.is_none() {
        return Outcome::Unsupported;
    }

    // Both halves ask about the same release, so it is worked out once.
    let release = Release::of_tagged(&tagged);
    let local = picture_tag.and_then(|_| find_on_disk(path, &release, covers));

    // What is left for the services: a cover only when the disk had none and the user has asked for
    // covers online, and a genre whenever one is wanted, since a genre can come from nowhere else.
    let wanted = online::Wanted {
        artwork: picture_tag.is_some() && local.is_none() && settings.online,
        genre: genre_tag.is_some(),
    };

    let (fetched, genre) = if wanted.any() {
        let answer = covers.online(release_query(&tagged), wanted);

        (
            answer.artwork.as_ref().map(|found| Cover {
                data: found.data.clone(),
                source: PathBuf::from(found.source.clone()),
            }),
            answer.genre.clone(),
        )
    } else {
        (None, None)
    };

    let cover = local.or(fetched);

    if cover.is_none() && genre.is_none() {
        return Outcome::NothingFound;
    }

    // `from_reader` sniffs the media type from the bytes, which also rejects anything that is not an
    // image the tag formats can carry.
    let picture = match &cover {
        Some(cover) => match Picture::from_reader(&mut cover.data.as_slice()) {
            Ok(mut picture) => {
                picture.set_pic_type(PictureType::CoverFront);
                Some(picture)
            }
            Err(err) => return Outcome::Failed(err.to_string()),
        },
        None => None,
    };

    // A file with no tag at all needs one before anything can go in it. The picture and the genre
    // may want different tags — a WAV takes a genre in its RIFF INFO chunk and no picture at all —
    // so each is put where it belongs and the file is saved once.
    if let (Some(tag_type), Some(picture)) = (picture_tag, picture) {
        if tagged.tag(tag_type).is_none() {
            tagged.insert_tag(Tag::new(tag_type));
        }

        match tagged.tag_mut(tag_type) {
            Some(tag) => tag.push_picture(picture),
            None => return Outcome::Failed("no writable tag".to_string()),
        }
    }

    if let (Some(tag_type), Some(genre)) = (genre_tag, &genre) {
        if tagged.tag(tag_type).is_none() {
            tagged.insert_tag(Tag::new(tag_type));
        }

        match tagged.tag_mut(tag_type) {
            Some(tag) => genre::write(tag, genre),
            None => return Outcome::Failed("no writable tag".to_string()),
        }
    }

    match tagged.save_to_path(path, WriteOptions::default()) {
        Ok(()) => Outcome::Filled { artwork: cover.map(|cover| cover.source), genre },
        Err(err) => Outcome::Failed(err.to_string()),
    }
}

/// A cover for `path` from the disk, which is where one is always looked for first: it is instant
/// and it asks nobody anything.
///
/// A conventionally named image in the folder is taken as that folder's cover, as the convention
/// says; art inside another track is borrowed only when that track says it is the same release.
fn find_on_disk(path: &Path, release: &Option<Release>, covers: &mut CoverCache) -> Option<Cover> {
    let folder = path.parent()?;

    if let Some(cover) = covers.sidecar(folder).cloned() {
        return Some(cover);
    }

    let release = release.as_ref()?;

    covers.sibling(folder, release).cloned()
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

/// Art already embedded in another track of the same release, in the same folder.
///
/// Albums are usually tagged as a set, so one track carrying a cover means the rest of *that
/// release* can share it. Tracks from anything else in the folder are passed over: borrowing from
/// them is how one song's art ends up on a dozen unrelated ones.
///
/// At most [`MAX_SIBLINGS_PROBED`] files are opened, since each one is a read. In a folder holding
/// many releases that budget can run out before a track of this one is reached, which costs a cover
/// that could have been shared — the lesser of the two wrongs.
fn find_in_siblings(folder: &Path, release: &Release) -> Option<Cover> {
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

        // The sibling has to say it is from this release before its art is worth anything here.
        if !Release::of_file(file.as_ref()).is_some_and(|theirs| release.matches(&theirs)) {
            continue;
        }

        if let Some(art) = file.front_cover()
            && !art.data.is_empty()
            && art.data.len() as u64 <= MAX_IMAGE_BYTES
        {
            return Some(Cover { data: art.data.clone(), source: sibling.clone() });
        }
    }

    None
}
