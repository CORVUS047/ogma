//! Looking at a folder by what its tags say rather than by how it is laid out.
//!
//! A library is arranged as folders, which is one answer to "where is that track" and a poor answer
//! to "what have I got by them". This groups everything below a folder under one tag at a time —
//! artist, album, genre, year — so the listing can be walked by name instead of by path.
//!
//! Reading the tags of a whole folder is the expensive part: the scan itself only looks at names,
//! but every file has to be opened for its tags. That happens on a thread of its own, and the songs
//! come back with their metadata already read, since a [`Song`] shares what it has read with every
//! copy of itself.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use crate::library;
use crate::song::Song;

/// What the tracks are grouped under.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Facet {
    #[default]
    Artist,
    Album,
    Genre,
    Year,
}

impl Facet {
    /// Every grouping, in the order the pane cycles through them.
    pub const ALL: [Facet; 4] = [Self::Artist, Self::Album, Self::Genre, Self::Year];

    /// A name for the interface.
    pub fn label(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::Album => "album",
            Self::Genre => "genre",
            Self::Year => "year",
        }
    }

    /// The next grouping in the cycle.
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|facet| *facet == self).unwrap_or(0);

        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    /// What `song` is filed under here.
    ///
    /// A track whose tags do not say gets the unknown group rather than being left out: a listing
    /// that quietly drops tracks is worse than one with an untidy group in it.
    pub fn value_of(self, song: &Song) -> String {
        let text = match self {
            // The album artist names a compilation better than the track artist does, and is the
            // same thing for everything else.
            Self::Artist => song.album_artist().or_else(|| song.artist()).map(str::to_string),
            Self::Album => song.album().map(str::to_string),
            Self::Genre => song.genre().map(str::to_string),
            Self::Year => song.year().map(|year| year.to_string()),
        };

        match text.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()) {
            Some(value) => value,
            None => self.unknown().to_string(),
        }
    }

    /// What the tracks that do not say are filed under.
    pub fn unknown(self) -> &'static str {
        match self {
            Self::Artist => "Unknown Artist",
            Self::Album => "Unknown Album",
            Self::Genre => "Unknown Genre",
            Self::Year => "Unknown Year",
        }
    }
}

/// One group, and the tracks in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub value: String,
    pub songs: Vec<Song>,
}

/// Group `songs` under `facet`, in the order the groups should be listed.
///
/// Groups come out sorted by name, case-insensitively, with the unknown group last however it is
/// spelled: it is not a name, and sorting it among them puts it in the middle of the alphabet.
/// Tracks keep the order they arrived in, which for a library scan is by path, so an album's tracks
/// stay in their own order.
pub fn group(songs: &[Song], facet: Facet) -> Vec<Group> {
    let mut groups: BTreeMap<String, Vec<Song>> = BTreeMap::new();

    for song in songs {
        groups.entry(facet.value_of(song)).or_default().push(song.clone());
    }

    let mut grouped: Vec<Group> = groups
        .into_iter()
        .map(|(value, songs)| Group { value, songs })
        .collect();

    grouped.sort_by(|left, right| {
        let unknown = facet.unknown();

        // Sorted as they read, and the unknown group after everything that has a name.
        (left.value == unknown)
            .cmp(&(right.value == unknown))
            .then_with(|| left.value.to_lowercase().cmp(&right.value.to_lowercase()))
    });

    grouped
}

/// Read the tags of everything below `root`, on a thread of its own.
///
/// The whole lot arrives in one message once it is done: a listing grouped by artist cannot be
/// drawn until every artist is known, so there is nothing useful to show halfway. Dropping the
/// receiver abandons the work.
pub fn index_in_background(root: PathBuf) -> Receiver<Vec<Song>> {
    let (sender, receiver) = mpsc::channel();

    let spawned = std::thread::Builder::new()
        .name("ogma-browse".to_string())
        .spawn(move || {
            let songs = index(&root);

            // A closed channel means the pane has moved on, which is not an error.
            let _ = sender.send(songs);
        });

    if spawned.is_err() {
        // Nothing will arrive, and the pane reports the silence rather than waiting for ever.
        return receiver;
    }

    receiver
}

/// Every track below `root`, with its tags already read.
///
/// The reading is the point: a [`Song`] reads its file the first time it is asked about, and doing
/// that here means the grouping — and every frame drawn afterwards — asks a song that already
/// knows the answer.
pub fn index(root: &std::path::Path) -> Vec<Song> {
    let songs = library::scan(root);

    for song in &songs {
        let _ = song.meta();
    }

    songs
}
