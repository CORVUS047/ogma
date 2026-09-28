//! A track on disk, and the metadata it reports.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::meta::{self, Artwork, AudioFile, MetaError};

/// One track, identified by its path.
///
/// Constructing a `Song` touches nothing: the file is read the first time metadata is asked for,
/// and the result — success or failure — is cached from then on. Walking a large library therefore
/// costs nothing until something needs to display a track.
///
/// Two songs are equal when they point at the same path; the cached metadata plays no part.
#[derive(Debug, Clone)]
pub struct Song {
    path: PathBuf,
    /// Shared so that cloning a song into a queue does not mean reading the file twice.
    meta: Arc<OnceLock<Result<Box<dyn AudioFile>, MetaError>>>,
}

impl Song {
    /// Point at a file without reading it.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Song { path: path.into(), meta: Arc::new(OnceLock::new()) }
    }

    /// Point at a file and read it now, failing if it cannot be read.
    ///
    /// Use this when a bad file should be rejected up front; use [`Song::new`] when it should
    /// simply display as unknown.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, MetaError> {
        let song = Self::new(path);

        match song.meta() {
            Some(_) => Ok(song),
            // The probe failed, and the cache now holds that failure.
            None => Err(song.take_error()),
        }
    }

    /// Consume the cached failure. Only called when the cache is known to hold one.
    fn take_error(self) -> MetaError {
        match Arc::try_unwrap(self.meta).map(OnceLock::into_inner) {
            Ok(Some(Err(err))) => err,
            // The song was just constructed here, so neither branch is reachable in practice.
            _ => MetaError::Unreadable {
                path: self.path,
                symphonia: None,
                lofty: None,
            },
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Forget what was read from the file, so the next access reads it again.
    ///
    /// Needed when something has changed the file underneath us — filling in missing artwork, for
    /// instance. Clones made before this keep their own cache.
    pub fn refresh(&mut self) {
        self.meta = Arc::new(OnceLock::new());
    }

    /// Everything the file reports, reading it on first use.
    ///
    /// `None` means the file could not be read at all; every accessor below then reports `None` too.
    pub fn meta(&self) -> Option<&dyn AudioFile> {
        self.meta_result().as_deref().ok()
    }

    /// Why the file could not be read, if it could not.
    pub fn error(&self) -> Option<&MetaError> {
        self.meta_result().as_ref().err()
    }

    /// Whether the file could be read.
    pub fn is_readable(&self) -> bool {
        self.meta().is_some()
    }

    fn meta_result(&self) -> &Result<Box<dyn AudioFile>, MetaError> {
        self.meta.get_or_init(|| meta::probe(&self.path))
    }

    // ------------------------------------------------------------------------------------- tags

    pub fn title(&self) -> Option<&str> {
        self.meta()?.title()
    }

    pub fn artist(&self) -> Option<&str> {
        self.meta()?.artist()
    }

    pub fn album(&self) -> Option<&str> {
        self.meta()?.album()
    }

    pub fn album_artist(&self) -> Option<&str> {
        self.meta()?.album_artist()
    }

    pub fn composer(&self) -> Option<&str> {
        self.meta()?.composer()
    }

    pub fn genre(&self) -> Option<&str> {
        self.meta()?.genre()
    }

    pub fn year(&self) -> Option<u32> {
        self.meta()?.year()
    }

    pub fn track_number(&self) -> Option<u32> {
        self.meta()?.track_number()
    }

    pub fn track_total(&self) -> Option<u32> {
        self.meta()?.track_total()
    }

    pub fn disc_number(&self) -> Option<u32> {
        self.meta()?.disc_number()
    }

    pub fn disc_total(&self) -> Option<u32> {
        self.meta()?.disc_total()
    }

    pub fn comment(&self) -> Option<&str> {
        self.meta()?.comment()
    }

    pub fn bpm(&self) -> Option<f32> {
        self.meta()?.bpm()
    }

    /// Embedded images, front cover first where one is labelled.
    pub fn front_cover(&self) -> Option<&Artwork> {
        self.meta()?.front_cover()
    }

    /// Track gain in dB, for levelling playback across a library.
    pub fn replay_gain(&self) -> Option<f32> {
        self.meta()?.replay_gain_track_gain()
    }

    // -------------------------------------------------------------------------------- the stream

    pub fn duration(&self) -> Option<Duration> {
        self.meta()?.duration()
    }

    /// Short codec name, e.g. `flac`.
    pub fn codec(&self) -> Option<&str> {
        Some(self.meta()?.codec())
    }

    pub fn sample_rate(&self) -> Option<u32> {
        self.meta()?.sample_rate()
    }

    pub fn bits_per_sample(&self) -> Option<u32> {
        self.meta()?.bits_per_sample()
    }

    pub fn channel_count(&self) -> Option<usize> {
        self.meta()?.channel_count()
    }

    /// Bitrate of the audio stream in kbps.
    pub fn bitrate(&self) -> Option<u32> {
        self.meta()?.audio_bitrate()
    }

    pub fn is_lossless(&self) -> Option<bool> {
        self.meta()?.is_lossless()
    }

    // ------------------------------------------------------------------------------ for display

    /// The title, or the file name when the file carries no title.
    pub fn display_title(&self) -> String {
        if let Some(title) = self.title() {
            return title.to_string();
        }

        self.path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// The artist, falling back to the album artist, then to a placeholder.
    pub fn display_artist(&self) -> &str {
        self.artist()
            .or_else(|| self.album_artist())
            .unwrap_or("Unknown Artist")
    }

    /// The album, or a placeholder.
    pub fn display_album(&self) -> &str {
        self.album().unwrap_or("Unknown Album")
    }

    /// `m:ss`, or `--:--` when the length is unknown.
    pub fn display_duration(&self) -> String {
        match self.duration() {
            Some(duration) => {
                let total = duration.as_secs();

                format!("{}:{:02}", total / 60, total % 60)
            }
            None => "--:--".to_string(),
        }
    }
}

/// Songs are the same song when they are the same file.
impl PartialEq for Song {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

impl Eq for Song {}

impl std::hash::Hash for Song {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.path.hash(state);
    }
}

impl From<PathBuf> for Song {
    fn from(path: PathBuf) -> Self {
        Song::new(path)
    }
}

impl From<&Path> for Song {
    fn from(path: &Path) -> Self {
        Song::new(path)
    }
}
