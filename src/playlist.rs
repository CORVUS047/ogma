//! Named collections of tracks, kept as TOML beside the config.
//!
//! A playlist holds its tracks in the order they were added and a sort setting on top of that. The
//! sort is not a view: [`Playlist::ordered`] is what the interface lists *and* what replaces the
//! queue, so what is seen and what plays are the same sequence. Switching back to
//! [`SortBy::Manual`] restores the order the tracks were added in, which is why that order is the
//! one stored.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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

/// A named list of tracks.
#[derive(Clone, Debug, PartialEq)]
pub struct Playlist {
    name: String,
    /// Tracks in the order they were added; [`Playlist::ordered`] applies the sort.
    songs: Vec<Song>,
    sort: SortBy,
    descending: bool,
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
        }
    }

    /// A playlist called `name` holding `songs`.
    pub fn with_songs(name: impl Into<String>, songs: impl IntoIterator<Item = Song>) -> Self {
        Playlist { songs: songs.into_iter().collect(), ..Self::new(name) }
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

    /// Add a track to the end.
    pub fn add(&mut self, song: Song) {
        self.songs.push(song);
    }

    /// Add several tracks to the end, skipping any the playlist already holds.
    ///
    /// Returns how many were added.
    pub fn add_all(&mut self, songs: impl IntoIterator<Item = Song>) -> usize {
        let before = self.songs.len();

        for song in songs {
            if !self.songs.contains(&song) {
                self.songs.push(song);
            }
        }

        self.songs.len() - before
    }

    /// Remove `song` wherever it sits in the playlist, returning whether it was there.
    ///
    /// Tracks are matched rather than counted, so a caller holding a row from a sorted or narrowed
    /// listing does not have to work out which of the added-order positions it is.
    pub fn remove_song(&mut self, song: &Song) -> bool {
        match self.songs.iter().position(|candidate| candidate == song) {
            Some(position) => {
                self.songs.remove(position);

                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.songs.clear();
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
        };

        std::fs::write(path, toml::to_string_pretty(&on_disk)?)?;

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

        Ok(Playlist {
            name: on_disk.name,
            songs,
            sort: on_disk.sort,
            descending: on_disk.descending,
        })
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
