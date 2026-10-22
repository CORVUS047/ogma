//! Named collections of tracks, kept as TOML beside the config.
//!
//! A playlist is a list of paths, and a path stops being true the moment something moves the file.
//! Two things keep one honest. A move made through ogma rewrites every playlist that held the
//! track, so the paths follow the file. A move made by anything else is noticed the next time the
//! playlist is loaded, and the track is looked for by the fingerprint of its audio — see
//! [`crate::fingerprint`] and [`Playlist::resolve_missing`].
//!
//! A playlist holds its tracks in the order they were added and a sort setting on top of that. The
//! sort is not a view: [`Playlist::ordered`] is what the interface lists *and* what replaces the
//! queue, so what is seen and what plays are the same sequence. Switching back to
//! [`SortBy::Manual`] restores the order the tracks were added in, which is why that order is the
//! one stored.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fingerprint;
use crate::song::Song;

/// Directory under the config directory that playlists live in.
const DIRECTORY: &str = "playlists";

/// Extension for a saved playlist.
const EXTENSION: &str = "toml";

/// How a playlist's tracks are ordered.
///
/// Every sort but [`SortBy::Manual`] and [`SortBy::FileName`] reads the files' tags, so sorting a
/// large playlist for the first time costs a pass over those files.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    /// The order the tracks were added in.
    #[default]
    Manual,
    Title,
    /// Artist, then album, disc and track: an artist's records in order.
    Artist,
    /// Album, then disc and track: the running order of each record.
    Album,
    Duration,
    /// Year, then album.
    Year,
    /// The name of the file on disk.
    FileName,
}

impl SortBy {
    /// Every sort, in the order the interface cycles through them.
    pub const ALL: [SortBy; 7] = [
        Self::Manual,
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::Year,
        Self::Duration,
        Self::FileName,
    ];

    /// A name for the interface.
    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Title => "title",
            Self::Artist => "artist",
            Self::Album => "album",
            Self::Duration => "length",
            Self::Year => "year",
            Self::FileName => "file",
        }
    }

    /// The next sort in the cycle.
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|sort| *sort == self).unwrap_or(0);

        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

/// Why a playlist could not be read or written.
#[derive(Debug)]
pub enum PlaylistError {
    /// A playlist needs a name to have a file name.
    EmptyName,
    /// The platform reported no config directory to keep playlists in.
    NoConfigDirectory,
    Io(std::io::Error),
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
}

impl std::fmt::Display for PlaylistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaylistError::EmptyName => write!(f, "a playlist needs a name"),
            PlaylistError::NoConfigDirectory => {
                write!(f, "no config directory available on this platform")
            }
            PlaylistError::Io(err) => write!(f, "{err}"),
            PlaylistError::Parse(err) => write!(f, "invalid playlist file: {err}"),
            PlaylistError::Serialize(err) => write!(f, "cannot write playlist: {err}"),
        }
    }
}

impl std::error::Error for PlaylistError {}

impl From<std::io::Error> for PlaylistError {
    fn from(err: std::io::Error) -> Self {
        PlaylistError::Io(err)
    }
}

impl From<toml::de::Error> for PlaylistError {
    fn from(err: toml::de::Error) -> Self {
        PlaylistError::Parse(err)
    }
}

impl From<toml::ser::Error> for PlaylistError {
    fn from(err: toml::ser::Error) -> Self {
        PlaylistError::Serialize(err)
    }
}

/// What is known about one track beyond where it sits, so that it can be found again.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Print {
    /// The fingerprint of the track's audio, from [`crate::fingerprint::of`].
    audio: u128,
    /// The library roots that have been swept for this track without turning it up.
    ///
    /// A track that was deleted rather than moved is never going to be found, and without this
    /// every load would hash the whole library again looking for it. Kept per root rather than as
    /// one flag because a root that does not hold the track says nothing about one that does: a
    /// playlist can gather tracks from more than one place, and the library can be pointed
    /// somewhere else entirely. Writing a track off because it is absent from a folder it was
    /// never in would be worse than sweeping twice.
    ///
    /// Cleared by [`Playlist::forget_unresolved`], which is how a search is asked for again once
    /// the file might plausibly be back.
    searched: Vec<PathBuf>,
}

impl Print {
    /// A fingerprint nothing has gone looking for yet.
    fn new(audio: u128) -> Self {
        Print { audio, searched: Vec::new() }
    }
}

/// What a pass of [`Playlist::resolve_missing`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolution {
    /// Tracks found somewhere else: where they were, and where they now are.
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// Tracks that are gone and that a sweep of the library did not turn up.
    pub lost: Vec<PathBuf>,
    /// Tracks that were still where they should be but had no fingerprint, and now have one.
    ///
    /// A playlist written before fingerprints existed holds none, and a track is fingerprinted when
    /// it is added — so without this, the tracks already in a playlist at the time of the upgrade
    /// could never be searched for, however many times the playlist was written afterwards.
    pub filled: usize,
}

impl Resolution {
    /// Whether anything at all came of the pass, and so whether the playlist needs writing.
    pub fn any(&self) -> bool {
        !self.moved.is_empty() || !self.lost.is_empty() || self.filled > 0
    }
}

/// A named list of tracks.
#[derive(Clone, Debug)]
pub struct Playlist {
    name: String,
    /// Tracks in the order they were added; [`Playlist::ordered`] applies the sort.
    songs: Vec<Song>,
    sort: SortBy,
    descending: bool,
    /// How to find each track again if its path stops being true, by path.
    ///
    /// Kept beside the songs rather than inside them because a [`Song`] is a pointer at a path that
    /// costs nothing to make, and fingerprinting is a read: only a track that someone has committed
    /// to a playlist is worth the read. A track with no entry here — a stream, or a file that could
    /// not be read when it was added — simply cannot be followed, which is how every track behaved
    /// before fingerprints existed.
    prints: HashMap<PathBuf, Print>,
}

/// Two playlists are equal when they are the same tracks, named and ordered the same way.
///
/// Fingerprints are what is known *about* those tracks rather than part of the playlist, and a
/// playlist that has been saved and read back holds the same tracks whether or not every one of
/// them could be fingerprinted at the time.
impl PartialEq for Playlist {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.songs == other.songs
            && self.sort == other.sort
            && self.descending == other.descending
    }
}

/// The shape a playlist takes on disk.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OnDisk {
    name: String,
    #[serde(default)]
    sort: SortBy,
    #[serde(default)]
    descending: bool,
    /// Paths, in the order the tracks were added. A track played from the network is its URL, and
    /// what it is called is in `streams` below.
    #[serde(default)]
    tracks: Vec<String>,
    /// What the entries in `tracks` that are URLs are called.
    ///
    /// Kept beside the list rather than in it so that `tracks` stays a plain list of strings: a
    /// playlist written by an older version still loads, and one written here still opens in
    /// anything that only knows about paths. A stream has no tags to read, so without this it would
    /// come back as its own link.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    streams: Vec<OnDiskStream>,
    /// How to find the entries in `tracks` that are files, should they move.
    ///
    /// Beside the list for the same reason `streams` is: `tracks` stays a plain list of paths, so
    /// the interesting half of a playlist file is still readable and editable by hand.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<OnDiskFile>,
}

/// How one track can be found again after it moves, for the playlist file.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OnDiskFile {
    path: String,
    /// The fingerprint of the first megabyte of the track's audio, as hex.
    audio: String,
    /// The library roots already swept for this track without finding it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    unresolved: Vec<String>,
}

/// What a track played from the network is called, for the playlist file.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OnDiskStream {
    url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artist: Option<String>,
    /// Length in whole seconds, which is all a listing shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seconds: Option<u64>,
}

impl OnDiskStream {
    /// What a streamed song should be written as, or `None` for a song that is a file.
    fn of(song: &Song) -> Option<Self> {
        let info = song.stream_info()?;

        Some(OnDiskStream {
            url: song.uri(),
            title: info.title.clone(),
            artist: info.artist.clone(),
            seconds: info.duration.map(|length| length.as_secs()),
        })
    }

    /// The song this describes.
    fn song(&self) -> Song {
        Song::stream(
            &self.url,
            crate::song::StreamInfo {
                title: self.title.clone(),
                artist: self.artist.clone(),
                duration: self.seconds.map(std::time::Duration::from_secs),
            },
        )
    }
}

impl Playlist {
    /// An empty playlist called `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Playlist {
            name: name.into(),
            songs: Vec::new(),
            sort: SortBy::default(),
            descending: false,
            prints: HashMap::new(),
        }
    }

    /// A playlist called `name` holding `songs`, fingerprinting each of them.
    pub fn with_songs(name: impl Into<String>, songs: impl IntoIterator<Item = Song>) -> Self {
        let mut playlist = Self::new(name);
        playlist.add_all(songs);

        playlist
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Rename the playlist. The file name follows the name, so a saved playlist should be deleted
    /// under its old name or it will be left behind.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    pub fn sort(&self) -> SortBy {
        self.sort
    }

    pub fn descending(&self) -> bool {
        self.descending
    }

    pub fn set_sort(&mut self, sort: SortBy) {
        self.sort = sort;
    }

    /// Move to the next sort in the cycle, returning it.
    pub fn cycle_sort(&mut self) -> SortBy {
        self.sort = self.sort.next();

        self.sort
    }

    /// Reverse the sort direction, returning whether it is now descending.
    pub fn toggle_direction(&mut self) -> bool {
        self.descending = !self.descending;

        self.descending
    }

    pub fn len(&self) -> usize {
        self.songs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
    }

    /// The tracks in the order they were added, whatever the sort says.
    pub fn as_added(&self) -> &[Song] {
        &self.songs
    }

    /// Add a track to the end, fingerprinting it so it can be followed if it moves.
    ///
    /// Adding reads the start of the file. That is the price of being able to find the track again,
    /// and it is paid once, here, rather than on every load.
    pub fn add(&mut self, song: Song) {
        self.take_print(&song);
        self.songs.push(song);
    }

    /// Add several tracks to the end, skipping any the playlist already holds.
    ///
    /// Returns how many were added.
    pub fn add_all(&mut self, songs: impl IntoIterator<Item = Song>) -> usize {
        let before = self.songs.len();

        for song in songs {
            if !self.songs.contains(&song) {
                self.add(song);
            }
        }

        self.songs.len() - before
    }

    /// Fingerprint `song` and keep it, unless there is nothing to fingerprint.
    ///
    /// A stream has no file, and a file that cannot be read has no audio to hash. Either way the
    /// track is simply one that cannot be followed.
    fn take_print(&mut self, song: &Song) {
        if song.is_stream() || self.prints.contains_key(song.path()) {
            return;
        }

        if let Some(audio) = fingerprint::of(song.path()) {
            self.prints.insert(song.path().to_path_buf(), Print::new(audio));
        }
    }

    /// Remove `song` wherever it sits in the playlist, returning whether it was there.
    ///
    /// Tracks are matched rather than counted, so a caller holding a row from a sorted or narrowed
    /// listing does not have to work out which of the added-order positions it is.
    pub fn remove_song(&mut self, song: &Song) -> bool {
        match self.songs.iter().position(|candidate| candidate == song) {
            Some(position) => {
                let removed = self.songs.remove(position);

                // Only once no other row points at the path: the same file can be listed twice.
                if !self.songs.contains(&removed) {
                    self.prints.remove(removed.path());
                }

                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.songs.clear();
        self.prints.clear();
    }

    /// The tracks in the playlist's sort order.
    ///
    /// This is the order the interface lists and the order that replaces the queue: the sort is part
    /// of the playlist, not a way of looking at it.
    pub fn ordered(&self) -> Vec<&Song> {
        let mut songs: Vec<&Song> = self.songs.iter().collect();

        // A manual playlist is already in its order, and sorting it would lose that order.
        if self.sort != SortBy::Manual {
            songs.sort_by(|left, right| compare(left, right, self.sort));
        }

        if self.descending {
            songs.reverse();
        }

        songs
    }

    /// The tracks in sort order, cloned, ready to hand to the player.
    pub fn ordered_songs(&self) -> Vec<Song> {
        self.ordered().into_iter().cloned().collect()
    }

    // ------------------------------------------------------------- following tracks that move

    /// Point every track that was at `old` at `new` instead, returning how many moved.
    ///
    /// `old` may be a file or a folder: a folder that has been moved takes everything below it, so
    /// a playlist full of an album's tracks follows the album. The fingerprint moves with the
    /// track, since a move does not change the audio.
    ///
    /// This is the path a move made through ogma takes. Nothing is read and nothing is searched
    /// for — the new location is already known, so there is no reason to go looking for it.
    pub fn retarget(&mut self, old: &Path, new: &Path) -> usize {
        let mut moved = 0;

        for index in 0..self.songs.len() {
            if self.songs[index].is_stream() {
                continue;
            }

            let was = self.songs[index].path().to_path_buf();

            let Some(now) = moved_path(&was, old, new) else {
                continue;
            };

            if let Some(print) = self.prints.remove(&was) {
                self.prints.insert(now.clone(), print);
            }

            self.songs[index] = Song::new(now);
            moved += 1;
        }

        moved
    }

    /// Apply several moves at once, returning how many tracks moved in total.
    pub fn retarget_all<'a>(
        &mut self,
        moves: impl IntoIterator<Item = (&'a Path, &'a Path)>,
    ) -> usize {
        moves.into_iter().map(|(old, new)| self.retarget(old, new)).sum()
    }

    /// Look under `root` for every track whose file is no longer where the playlist says.
    ///
    /// This is the path a move made by anything other than ogma takes — a file manager, a shell, a
    /// tag editor that rewrites rather than edits in place. Each missing track is looked for by the
    /// fingerprint taken when it was added, so it is found wherever it went and whatever it is now
    /// called.
    ///
    /// Three things keep the cost down, because a sweep means hashing files:
    ///
    /// * nothing is read at all unless a track is actually missing, and the check for that is a
    ///   [`Path::exists`] per track;
    /// * files sharing a name with something missing are hashed first, since a move usually keeps
    ///   the name, and the sweep stops as soon as everything has been found;
    /// * every missing track is searched for in the *same* sweep, so a whole album that moved costs
    ///   one pass over the library rather than one per track.
    ///
    /// A track the sweep does not find is marked, and later passes skip it rather than sweeping the
    /// library again for a file that was deleted. [`Playlist::forget_unresolved`] takes the marks
    /// off.
    ///
    /// Tracks that are present but have no fingerprint get one here, which is what brings a
    /// playlist written before fingerprints existed up to date: see [`Resolution::filled`]. That
    /// read happens once, since the fingerprint is then saved with the playlist.
    ///
    /// The playlist is changed but not saved: the caller decides when to write, and
    /// [`Resolution::any`] says whether there is anything to write.
    pub fn resolve_missing(&mut self, root: &Path) -> Resolution {
        let mut resolution = Resolution { filled: self.fill_prints(), ..Resolution::default() };

        // What to look for, by fingerprint. A track with no fingerprint, and one already searched
        // for and not found, are both left exactly as they are.
        //
        // Several paths can share a fingerprint, since the same recording can sit in a library
        // twice. Each of them is a separate track to find, and each found file answers for one of
        // them — which one does not matter, the audio being identical.
        let mut wanted: HashMap<u128, Vec<PathBuf>> = HashMap::new();
        let mut asked = HashSet::new();

        for song in &self.songs {
            if song.is_stream() || song.path().exists() || !asked.insert(song.path()) {
                continue;
            }

            if let Some(print) = self.prints.get(song.path())
                && !print.searched.iter().any(|swept| swept == root)
            {
                wanted.entry(print.audio).or_default().push(song.path().to_path_buf());
            }
        }

        if wanted.is_empty() {
            return resolution;
        }

        // A file the playlist already points at is not a candidate: it is accounted for.
        let held: HashSet<PathBuf> =
            self.songs.iter().map(|song| song.path().to_path_buf()).collect();

        // The names being looked for, so the likely candidates can be hashed before the rest.
        let names: HashSet<OsString> = wanted
            .values()
            .flatten()
            .filter_map(|path| path.file_name().map(|name| name.to_os_string()))
            .collect();

        let (likely, rest): (Vec<PathBuf>, Vec<PathBuf>) = crate::library::paths(root)
            .into_iter()
            .filter(|path| !held.contains(path))
            .partition(|path| {
                path.file_name().is_some_and(|name| names.contains(name))
            });

        for candidate in likely.into_iter().chain(rest) {
            if wanted.is_empty() {
                break;
            }

            let Some(audio) = fingerprint::of(&candidate) else {
                continue;
            };

            if let Some(paths) = wanted.get_mut(&audio) {
                let was = paths.pop().expect("a fingerprint being looked for has a path");

                if paths.is_empty() {
                    wanted.remove(&audio);
                }

                self.retarget(&was, &candidate);
                resolution.moved.push((was, candidate));
            }
        }

        // Whatever is left is not under this library root. It may still be under another, so only
        // this root is written off for it.
        for was in wanted.into_values().flatten() {
            if let Some(print) = self.prints.get_mut(&was) {
                print.searched.push(root.to_path_buf());
            }

            resolution.lost.push(was);
        }

        resolution.moved.sort();
        resolution.lost.sort();

        resolution
    }

    /// Fingerprint every track that is where the playlist says but has nothing to be found by.
    ///
    /// Returns how many gained one. This is the upgrade path: a playlist from before fingerprints
    /// existed has none at all, and a track can only be fingerprinted while its file is still
    /// there — which makes the pass over a playlist whose files are present the one chance to do
    /// it.
    ///
    /// A file that is present but cannot be read has nothing to hash and so is tried again on the
    /// next pass. That is a failed probe on a handful of files rather than a read, and a playlist
    /// is a short list.
    fn fill_prints(&mut self) -> usize {
        // A file listed twice is one read, not two.
        let mut seen = HashSet::new();

        let missing: Vec<PathBuf> = self
            .songs
            .iter()
            .filter(|song| !song.is_stream() && !self.prints.contains_key(song.path()))
            .filter(|song| seen.insert(song.path()) && song.path().is_file())
            .map(|song| song.path().to_path_buf())
            .collect();

        let mut filled = 0;

        for path in missing {
            if let Some(audio) = fingerprint::of(&path) {
                self.prints.insert(path, Print::new(audio));
                filled += 1;
            }
        }

        filled
    }

    /// Let the next [`Playlist::resolve_missing`] search again for tracks it has given up on.
    ///
    /// Returns how many tracks were being skipped.
    pub fn forget_unresolved(&mut self) -> usize {
        let mut forgotten = 0;

        for print in self.prints.values_mut() {
            if !print.searched.is_empty() {
                print.searched.clear();
                forgotten += 1;
            }
        }

        forgotten
    }

    /// Whether `song` has no file left to play.
    ///
    /// What the interface marks a row with: a track that is listed but plays nothing should say so
    /// rather than being found out when it is reached. Whether it has been searched for does not
    /// come into it — the file is not there either way.
    pub fn is_lost(&self, song: &Song) -> bool {
        !song.is_stream() && !song.path().exists()
    }

    // ------------------------------------------------------------------------------ on disk

    /// The file name this playlist saves under: its name in lowercase, with runs of anything else
    /// turned into single underscores.
    pub fn file_stem(&self) -> String {
        slug(&self.name)
    }

    /// The file name including its extension.
    pub fn file_name(&self) -> String {
        format!("{}.{EXTENSION}", self.file_stem())
    }

    /// Where playlists are kept: `<config directory>/ogma/playlists`.
    pub fn directory() -> Result<PathBuf, PlaylistError> {
        let config = crate::config::Config::config_path().map_err(|_| {
            PlaylistError::NoConfigDirectory
        })?;

        let parent = config.parent().ok_or(PlaylistError::NoConfigDirectory)?;

        Ok(parent.join(DIRECTORY))
    }

    /// Where this playlist saves to.
    pub fn path(&self) -> Result<PathBuf, PlaylistError> {
        if self.file_stem().is_empty() {
            return Err(PlaylistError::EmptyName);
        }

        Ok(Self::directory()?.join(self.file_name()))
    }

    /// Write the playlist to the playlists directory.
    pub fn save(&self) -> Result<PathBuf, PlaylistError> {
        let path = self.path()?;
        self.save_to(&path)?;

        Ok(path)
    }

    /// Write the playlist to `path`, creating parent directories as needed.
    pub fn save_to(&self, path: &Path) -> Result<(), PlaylistError> {
        if self.file_stem().is_empty() {
            return Err(PlaylistError::EmptyName);
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Paths are stored in the order tracks were added, so `manual` still means something after a
        // reload even if the playlist is currently sorted some other way.
        let on_disk = OnDisk {
            name: self.name.clone(),
            sort: self.sort,
            descending: self.descending,
            tracks: self.songs.iter().map(|song| song.uri()).collect(),
            streams: self.songs.iter().filter_map(OnDiskStream::of).collect(),
            files: self.fingerprints(),
        };

        let text = toml::to_string_pretty(&on_disk)?;

        // Written beside the playlist and renamed over it, rather than written in place. A rename
        // within a directory is atomic, so anything reading the playlist sees either the whole of
        // the old file or the whole of the new one. Writing in place truncates first, which leaves
        // a window where the file is empty — and several interfaces can be open on one library,
        // each of them writing playlists as tracks are added and followed.
        let temporary = path.with_file_name(format!(
            "{}.{}.tmp",
            path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));

        std::fs::write(&temporary, text)?;

        if let Err(err) = std::fs::rename(&temporary, path) {
            // Nothing is left lying about in the playlists directory to be listed as a playlist.
            let _ = std::fs::remove_file(&temporary);

            return Err(err.into());
        }

        Ok(())
    }

    /// Read a playlist from `path`.
    pub fn load_from(path: &Path) -> Result<Self, PlaylistError> {
        let text = std::fs::read_to_string(path)?;
        let on_disk: OnDisk = toml::from_str(&text)?;

        // A track that one of the stream entries names comes back as a stream, with what it is
        // called; everything else is a file, as it always was.
        let songs = on_disk
            .tracks
            .into_iter()
            .map(|track| {
                match on_disk.streams.iter().find(|stream| stream.url == track) {
                    Some(stream) => stream.song(),
                    None => Song::new(track),
                }
            })
            .collect();

        let prints = on_disk
            .files
            .iter()
            .filter_map(|file| {
                let audio = fingerprint::from_hex(&file.audio)?;
                let searched = file.unresolved.iter().map(PathBuf::from).collect();

                Some((PathBuf::from(&file.path), Print { audio, searched }))
            })
            .collect();

        Ok(Playlist {
            name: on_disk.name,
            songs,
            sort: on_disk.sort,
            descending: on_disk.descending,
            prints,
        })
    }

    /// The fingerprints to write, in the order the tracks were added so the file is stable.
    fn fingerprints(&self) -> Vec<OnDiskFile> {
        let mut written = HashSet::new();

        self.songs
            .iter()
            .filter(|song| written.insert(song.path().to_path_buf()))
            .filter_map(|song| {
                let print = self.prints.get(song.path())?;

                Some(OnDiskFile {
                    path: song.uri(),
                    audio: fingerprint::to_hex(print.audio),
                    unresolved: print
                        .searched
                        .iter()
                        .map(|root| root.to_string_lossy().into_owned())
                        .collect(),
                })
            })
            .collect()
    }

    /// Every playlist in the playlists directory, by name.
    ///
    /// A file that cannot be read is skipped rather than failing the lot: one bad playlist should not
    /// hide the others.
    pub fn load_all() -> Vec<Self> {
        let Ok(directory) = Self::directory() else {
            return Vec::new();
        };

        Self::load_all_from(&directory)
    }

    /// Every playlist in `directory`.
    pub fn load_all_from(directory: &Path) -> Vec<Self> {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };

        let mut playlists: Vec<Playlist> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == EXTENSION))
            .filter_map(|path| Self::load_from(&path).ok())
            .collect();

        playlists.sort_by_key(|playlist| playlist.name.to_lowercase());

        playlists
    }

    /// Delete the playlist's file. A playlist that was never saved is not an error.
    pub fn delete(&self) -> Result<(), PlaylistError> {
        let path = self.path()?;

        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

/// Where `path` ends up when `old` is moved to `new`, or `None` when the move does not touch it.
///
/// `old` is either the path itself — one file moved — or a folder it sits under, in which case the
/// part of the path below that folder is carried across unchanged.
fn moved_path(path: &Path, old: &Path, new: &Path) -> Option<PathBuf> {
    if path == old {
        return Some(new.to_path_buf());
    }

    path.strip_prefix(old).ok().map(|rest| new.join(rest))
}

/// A name turned into a file stem: lowercase, with runs of anything but letters and digits becoming
/// a single underscore.
fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending_underscore = false;

    for c in name.chars() {
        if c.is_alphanumeric() {
            if pending_underscore && !out.is_empty() {
                out.push('_');
            }

            pending_underscore = false;
            out.extend(c.to_lowercase());
        } else {
            // Runs of punctuation and spaces collapse, and a trailing run is dropped entirely.
            pending_underscore = true;
        }
    }

    out
}

/// Order two tracks by `sort`, falling back on the path so the order is stable.
fn compare(left: &Song, right: &Song, sort: SortBy) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    /// Compare optional values, with the ones that have a value first.
    fn some_first<T: Ord>(left: Option<T>, right: Option<T>) -> Ordering {
        match (left, right) {
            (Some(left), Some(right)) => left.cmp(&right),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }

    /// Case-insensitive text comparison, for names.
    fn text(value: Option<&str>) -> Option<String> {
        value.map(str::to_lowercase)
    }

    /// Disc then track, for keeping a record in its running order.
    fn position(song: &Song) -> (u32, u32) {
        (song.disc_number().unwrap_or(1), song.track_number().unwrap_or(0))
    }

    let ordering = match sort {
        SortBy::Manual => Ordering::Equal,
        SortBy::Title => some_first(text(left.title()), text(right.title())),
        SortBy::Artist => some_first(
            text(left.album_artist().or_else(|| left.artist())),
            text(right.album_artist().or_else(|| right.artist())),
        )
        .then_with(|| some_first(text(left.album()), text(right.album())))
        .then_with(|| position(left).cmp(&position(right))),
        SortBy::Album => some_first(text(left.album()), text(right.album()))
            .then_with(|| position(left).cmp(&position(right))),
        SortBy::Duration => some_first(left.duration(), right.duration()),
        SortBy::Year => some_first(left.year(), right.year())
            .then_with(|| some_first(text(left.album()), text(right.album())))
            .then_with(|| position(left).cmp(&position(right))),
        SortBy::FileName => some_first(
            left.path().file_name().map(|name| name.to_string_lossy().to_lowercase()),
            right.path().file_name().map(|name| name.to_string_lossy().to_lowercase()),
        ),
    };

    // Equal keys still need a definite order, or the listing shuffles about between draws.
    ordering.then_with(|| left.path().cmp(right.path()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_becomes_a_file_name() {
        assert_eq!(slug("Late Night"), "late_night");
        assert_eq!(slug("ROAD TRIP"), "road_trip");
        assert_eq!(slug("Sunday   Morning!"), "sunday_morning");
        assert_eq!(slug("  leading and trailing  "), "leading_and_trailing");
        assert_eq!(slug("90s / 00s"), "90s_00s");
        assert_eq!(slug("Björk & Co."), "björk_co");

        // Nothing usable in the name means nothing to save under.
        assert_eq!(slug("!!!"), "");
    }

    #[test]
    fn the_file_name_carries_the_extension() {
        let playlist = Playlist::new("Late Night");

        assert_eq!(playlist.file_stem(), "late_night");
        assert_eq!(playlist.file_name(), "late_night.toml");
    }

    #[test]
    fn the_sort_cycle_comes_back_round() {
        let mut sort = SortBy::Manual;

        for _ in 0..SortBy::ALL.len() {
            sort = sort.next();
        }

        assert_eq!(sort, SortBy::Manual);
    }
}
