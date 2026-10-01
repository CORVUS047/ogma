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
//! The filling runs in the background and keeps running: [`watch`] walks the library as soon as it
//! is opened and again every [`RESCAN_INTERVAL`], so a track that arrives afterwards — a download
//! finishing, an album copied in — is filled in without the player having to be reopened. Each file
//! is dealt with once; only one that could not be read is tried again, since that usually means it
//! was still arriving.
//!
//! **This writes to the user's music files**, so it only ever runs when
//! [`Config::auto_fill_metadata`](crate::config::Config::auto_fill_metadata) is on, and it only ever
//! adds a picture to a file that has none. An existing picture is never replaced.
//!
//! A file is [claimed](crate::claim) for as long as it is being filled, so that two interfaces open
//! on one library take turns over it rather than both rewriting it at once.
//!
//! **Looking online is a second, separate switch**
//! ([`Config::fetch_artwork_online`](crate::config::Config::fetch_artwork_online)), also off by
//! default: reading a file that is already on the machine and sending an album title to a third party
//! are different kinds of act, and consenting to the first is not consenting to the second.

pub mod online;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag, TagType};

use crate::claim;
use crate::genre;
use crate::library;
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
    /// Another player holds the file, so it was left alone and will be looked at again.
    Busy,
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
    /// Files another player was writing to, which this run left for later.
    pub busy: usize,
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
            Outcome::Busy => report.busy += 1,
            Outcome::Unsupported => report.unsupported += 1,
            Outcome::Failed(_) => report.failed += 1,
        }
    }

    report
}

/// How often the library is walked again, looking for files that were not there before.
///
/// A pass over a library nothing has been added to costs a directory walk and no file reads at all,
/// so this is soon enough to catch a download a moment after it lands and rare enough to leave the
/// disk alone.
const RESCAN_INTERVAL: Duration = Duration::from_secs(20);

/// A fill running in the background, and the files it has finished with.
///
/// It keeps watching rather than ending with its first pass: tracks arrive in a library after the
/// player has opened — a download finishes, a folder is copied in — and a cover that shows up only
/// once the user next restarts the player looks like a cover that is missing.
///
/// Dropping this stops the watching. The thread notices between files, so a pass already under way
/// stops at the next one rather than finishing the library.
#[derive(Debug)]
pub struct Watch {
    /// The folder being watched, so a watch of somewhere else can be told from this one.
    root: PathBuf,
    /// What the watching is allowed to do, for the same reason.
    settings: Settings,
    /// Files that gained a cover or a genre, as they are written.
    filled: Receiver<PathBuf>,
    /// Held only to be dropped: the thread waits on the other end, so losing this is how it is
    /// told to stop.
    _leash: Sender<()>,
    /// Set once the thread is gone, which happens only if it could not be started.
    ended: bool,
}

impl Watch {
    /// Files filled in since this was last asked, in the order they were written.
    ///
    /// Nothing needs reloading on their account — a [`Song`](crate::song::Song) reads the file again
    /// when it is refreshed — so a caller with nothing to display may drop the list.
    pub fn collect(&mut self) -> Vec<PathBuf> {
        let mut filled = Vec::new();

        loop {
            match self.filled.try_recv() {
                Ok(path) => filled.push(path),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.ended = true;
                    break;
                }
            }
        }

        filled
    }

    /// Whether the watching has stopped, which means it will report nothing more.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// The folder being watched.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// What the watching is allowed to do.
    pub fn settings(&self) -> Settings {
        self.settings
    }
}

/// Fill in what is missing under `root`, and keep doing it as files appear.
///
/// The first pass starts at once, so opening a library is what checks it over; every
/// [`RESCAN_INTERVAL`] after that the folder is walked again and whatever is new is filled in.
/// Reading and writing a whole library takes long enough that doing it on the interface's thread
/// would freeze the display, so it happens on a thread of its own and results arrive as they come.
pub fn watch(root: impl Into<PathBuf>, settings: Settings) -> Watch {
    watch_every(root, settings, RESCAN_INTERVAL)
}

/// [`watch`], looking again every `interval` rather than at the usual cadence.
pub fn watch_every(root: impl Into<PathBuf>, settings: Settings, interval: Duration) -> Watch {
    let root = root.into();
    let (sender, filled) = mpsc::channel();
    let (leash, held) = mpsc::channel();

    let walked = root.clone();

    // A failed spawn is not worth reporting: the worst case is that no covers get filled in. The
    // watch then says it has ended, the sender having gone with the closure.
    let _ = std::thread::Builder::new()
        .name("ogma-autofill".to_string())
        .spawn(move || {
            // What has been dealt with already, so each pass only costs what the library gained.
            let mut handled: HashSet<PathBuf> = HashSet::new();

            loop {
                if !pass(&walked, settings, &mut handled, &sender, &held) {
                    return;
                }

                // Waiting on the leash is both the pause between passes and how the end of the
                // watch is noticed, so a dropped `Watch` is not slept through.
                if let Err(RecvTimeoutError::Disconnected) = held.recv_timeout(interval) {
                    return;
                }
            }
        });

    Watch { root, settings, filled, _leash: leash, ended: false }
}

/// One walk of `root`, filling in every file not in `handled`.
///
/// Returns whether the watching should go on: a dropped [`Watch`] ends it, and so does a receiver
/// nobody is reading any more.
///
/// The covers found are remembered for the pass and no longer. Keeping them would spare a long-
/// lived watch some work, but a folder that had no art when it was first looked at is exactly the
/// folder a `cover.jpg` is about to be dropped into, and a remembered miss would hide it.
fn pass(
    root: &Path,
    settings: Settings,
    handled: &mut HashSet<PathBuf>,
    filled: &Sender<PathBuf>,
    leash: &Receiver<()>,
) -> bool {
    let mut covers = CoverCache::default();

    for path in library::paths(root) {
        if let Err(TryRecvError::Disconnected) = leash.try_recv() {
            return false;
        }

        if handled.contains(&path) {
            continue;
        }

        let outcome = fill_with(&path, &mut covers, settings);

        // A file that could not be read is left for the next pass: the usual reason is that it is
        // still arriving — a download part-written, a copy in progress — and that fixes itself. So
        // is one another player holds, which it will not hold for long. Everything else is settled,
        // and asking again every twenty seconds for the rest of the session would mean reading the
        // whole library over and over.
        if !matches!(outcome, Outcome::Failed(_) | Outcome::Busy) {
            handled.insert(path.clone());
        }

        if let Outcome::Filled { .. } = outcome
            && filled.send(path).is_err()
        {
            // Nobody is listening any more, which means the application has moved on.
            return false;
        }
    }

    true
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
    // Several interfaces can be open on one library, each filling in metadata of its own accord, and
    // two of them rewriting one file at the same moment is how a track gets truncated. The claim is
    // held for the whole of this function: claiming only around the write would let the other player
    // read the tags before this one has finished changing them.
    let Some(_claim) = claim::Claim::on(path) else {
        return Outcome::Busy;
    };

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
