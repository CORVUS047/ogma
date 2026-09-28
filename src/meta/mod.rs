//! Reading everything a file can tell us about the music inside it.
//!
//! [`probe`] opens a file, pulls out every tag, picture and technical parameter that the container
//! states, and returns it as a codec-specific struct behind the [`AudioFile`] trait. The trait
//! exposes one accessor per kind of metadata; anything the format cannot express, or that this
//! particular file left out, comes back as `None`.
//!
//! ```no_run
//! # fn main() -> Result<(), ogma::meta::MetaError> {
//! let file = ogma::meta::probe("track.flac")?;
//! println!("{:?} - {:?} [{}]", file.artist(), file.title(), file.codec_long());
//! # Ok(())
//! # }
//! ```
//!
//! Accessors default to reading the shared [`RawMeta`] body, so a codec struct only overrides what
//! is genuinely codec-specific: MP3 reports no bit depth, FLAC reports that it is lossless.

mod artwork;
mod codecs;
mod raw;
mod sym_map;

pub use artwork::{Artwork, ArtworkKind};
pub use codecs::{
    AacFile, AdpcmFile, AlacFile, FlacFile, Mp1File, Mp2File, Mp3File, OpusFile, PcmFile,
    UnknownFile, VorbisFile,
};
pub use raw::{ContainerTag, RawMeta, Technical, codec_registry};

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lofty::tag::{ItemKey, TagType};
use symphonia::core::audio::sample::SampleFormat;

/// Why a file could not be read.
#[derive(Debug)]
pub enum MetaError {
    /// Neither the demuxer nor the tag reader could make sense of the file.
    Unreadable {
        path: PathBuf,
        /// Symphonia's complaint, if it got that far.
        symphonia: Option<String>,
        /// Lofty's complaint, if it got that far.
        lofty: Option<String>,
    },
}

impl fmt::Display for MetaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetaError::Unreadable { path, symphonia, lofty } => {
                write!(f, "cannot read {}", path.display())?;

                if let Some(err) = symphonia {
                    write!(f, ": demuxer: {err}")?;
                }
                if let Some(err) = lofty {
                    write!(f, ": tags: {err}")?;
                }

                Ok(())
            }
        }
    }
}

impl std::error::Error for MetaError {}

/// The leading four-digit year of a date string such as `2024-06-01`.
fn leading_year(date: &str) -> Option<u32> {
    let head: String = date.trim().chars().take_while(char::is_ascii_digit).collect();

    (head.len() == 4).then(|| head.parse().ok())?
}

/// Read all available metadata from `path`.
///
/// The returned value is a codec-specific struct ([`FlacFile`], [`Mp3File`], ...) chosen from the
/// codec found inside the container rather than from the file extension. Files whose codec this
/// build cannot identify come back as [`UnknownFile`], still carrying whatever tags were readable.
pub fn probe(path: impl AsRef<Path>) -> Result<Box<dyn AudioFile>, MetaError> {
    raw::load(path.as_ref())
}

/// Everything one audio file can report.
///
/// Implementors are named after their codec. Each accessor returns `None` when the metadata is
/// absent from the file, or when the format has no way to express it at all.
pub trait AudioFile: fmt::Debug + Send + Sync {
    // ---------------------------------------------------------------- identity of the implementor

    /// The shared metadata body every accessor below reads from.
    fn raw(&self) -> &RawMeta;

    /// Short codec name, e.g. `mp3`.
    fn codec(&self) -> &str;

    /// Descriptive codec name, e.g. `MPEG Audio Layer 3`.
    fn codec_long(&self) -> &str;

    /// Whether the codec reconstructs the original samples exactly. `None` when the codec is
    /// unknown to this build.
    fn is_lossless(&self) -> Option<bool>;

    // ------------------------------------------------------------------------------------- titles

    fn title(&self) -> Option<&str> {
        self.raw().text(ItemKey::TrackTitle)
    }

    fn title_sort(&self) -> Option<&str> {
        self.raw().text(ItemKey::TrackTitleSortOrder)
    }

    fn subtitle(&self) -> Option<&str> {
        self.raw().text(ItemKey::TrackSubtitle)
    }

    fn album(&self) -> Option<&str> {
        self.raw().text(ItemKey::AlbumTitle)
    }

    fn album_sort(&self) -> Option<&str> {
        self.raw().text(ItemKey::AlbumTitleSortOrder)
    }

    /// Subtitle of the set the album belongs to, e.g. a box-set disc name.
    fn set_subtitle(&self) -> Option<&str> {
        self.raw().text(ItemKey::SetSubtitle)
    }

    /// Grouping, as iTunes and ID3v2's `TIT1` call it.
    fn content_group(&self) -> Option<&str> {
        self.raw().text(ItemKey::ContentGroup)
    }

    fn work(&self) -> Option<&str> {
        self.raw().text(ItemKey::Work)
    }

    fn movement(&self) -> Option<&str> {
        self.raw().text(ItemKey::Movement)
    }

    fn movement_number(&self) -> Option<u32> {
        self.raw().number(ItemKey::MovementNumber)
    }

    fn movement_total(&self) -> Option<u32> {
        self.raw().number(ItemKey::MovementTotal)
    }

    fn original_album(&self) -> Option<&str> {
        self.raw().text(ItemKey::OriginalAlbumTitle)
    }

    fn show_name(&self) -> Option<&str> {
        self.raw().text(ItemKey::ShowName)
    }

    fn show_name_sort(&self) -> Option<&str> {
        self.raw().text(ItemKey::ShowNameSortOrder)
    }

    // ------------------------------------------------------------------------------------- people

    fn artist(&self) -> Option<&str> {
        self.raw().text(ItemKey::TrackArtist)
    }

    /// Every credited track artist, for files that list them separately.
    fn artists(&self) -> Option<Vec<&str>> {
        match self.raw().texts(ItemKey::TrackArtists) {
            Some(artists) => Some(artists),
            None => self.raw().texts(ItemKey::TrackArtist),
        }
    }

    fn artist_sort(&self) -> Option<&str> {
        self.raw().text(ItemKey::TrackArtistSortOrder)
    }

    fn album_artist(&self) -> Option<&str> {
        self.raw().text(ItemKey::AlbumArtist)
    }

    fn album_artists(&self) -> Option<Vec<&str>> {
        match self.raw().texts(ItemKey::AlbumArtists) {
            Some(artists) => Some(artists),
            None => self.raw().texts(ItemKey::AlbumArtist),
        }
    }

    fn album_artist_sort(&self) -> Option<&str> {
        self.raw().text(ItemKey::AlbumArtistSortOrder)
    }

    fn composer(&self) -> Option<&str> {
        self.raw().text(ItemKey::Composer)
    }

    fn composer_sort(&self) -> Option<&str> {
        self.raw().text(ItemKey::ComposerSortOrder)
    }

    fn conductor(&self) -> Option<&str> {
        self.raw().text(ItemKey::Conductor)
    }

    fn arranger(&self) -> Option<&str> {
        self.raw().text(ItemKey::Arranger)
    }

    fn engineer(&self) -> Option<&str> {
        self.raw().text(ItemKey::Engineer)
    }

    fn producer(&self) -> Option<&str> {
        self.raw().text(ItemKey::Producer)
    }

    fn mix_dj(&self) -> Option<&str> {
        self.raw().text(ItemKey::MixDj)
    }

    fn mix_engineer(&self) -> Option<&str> {
        self.raw().text(ItemKey::MixEngineer)
    }

    fn performer(&self) -> Option<&str> {
        self.raw().text(ItemKey::Performer)
    }

    fn remixer(&self) -> Option<&str> {
        self.raw().text(ItemKey::Remixer)
    }

    fn lyricist(&self) -> Option<&str> {
        self.raw().text(ItemKey::Lyricist)
    }

    fn writer(&self) -> Option<&str> {
        self.raw().text(ItemKey::Writer)
    }

    fn director(&self) -> Option<&str> {
        self.raw().text(ItemKey::Director)
    }

    fn original_artist(&self) -> Option<&str> {
        self.raw().text(ItemKey::OriginalArtist)
    }

    fn original_lyricist(&self) -> Option<&str> {
        self.raw().text(ItemKey::OriginalLyricist)
    }

    // --------------------------------------------------------------------- position in the release

    fn track_number(&self) -> Option<u32> {
        self.raw().number(ItemKey::TrackNumber)
    }

    fn track_total(&self) -> Option<u32> {
        self.raw().number(ItemKey::TrackTotal)
    }

    fn disc_number(&self) -> Option<u32> {
        self.raw().number(ItemKey::DiscNumber)
    }

    fn disc_total(&self) -> Option<u32> {
        self.raw().number(ItemKey::DiscTotal)
    }

    // -------------------------------------------------------------------------------------- dates

    /// Release year. Taken from a dedicated year field where one exists, otherwise from the leading
    /// year of whichever date the file does carry, since most taggers write only a full date.
    fn year(&self) -> Option<u32> {
        if let Some(year) = self.raw().number(ItemKey::Year) {
            return Some(year);
        }

        [
            ItemKey::RecordingDate,
            ItemKey::ReleaseDate,
            ItemKey::OriginalReleaseDate,
        ]
        .into_iter()
        .filter_map(|key| self.raw().text(key))
        .find_map(leading_year)
    }

    fn recording_date(&self) -> Option<&str> {
        self.raw().text(ItemKey::RecordingDate)
    }

    fn release_date(&self) -> Option<&str> {
        self.raw().text(ItemKey::ReleaseDate)
    }

    fn original_release_date(&self) -> Option<&str> {
        self.raw().text(ItemKey::OriginalReleaseDate)
    }

    fn tagging_date(&self) -> Option<&str> {
        self.raw().text(ItemKey::TaggingTime)
    }

    fn encoding_date(&self) -> Option<&str> {
        self.raw().text(ItemKey::EncodingTime)
    }

    /// Duration in milliseconds as *claimed by the tag*. Prefer [`AudioFile::duration`], which is
    /// derived from the stream itself.
    fn tagged_length_ms(&self) -> Option<u32> {
        self.raw().number(ItemKey::Length)
    }

    // ----------------------------------------------------------------------------- classification

    fn genre(&self) -> Option<&str> {
        self.raw().text(ItemKey::Genre)
    }

    fn genres(&self) -> Option<Vec<&str>> {
        self.raw().texts(ItemKey::Genre)
    }

    fn mood(&self) -> Option<&str> {
        self.raw().text(ItemKey::Mood)
    }

    fn initial_key(&self) -> Option<&str> {
        self.raw().text(ItemKey::InitialKey)
    }

    fn color(&self) -> Option<&str> {
        self.raw().text(ItemKey::Color)
    }

    fn bpm(&self) -> Option<f32> {
        self.raw().float(ItemKey::Bpm)
    }

    /// Rounded BPM, for the formats that store only an integer.
    fn integer_bpm(&self) -> Option<u32> {
        self.raw().number(ItemKey::IntegerBpm)
    }

    fn is_compilation(&self) -> Option<bool> {
        self.raw().flag(ItemKey::FlagCompilation)
    }

    fn parental_advisory(&self) -> Option<u32> {
        self.raw().number(ItemKey::ParentalAdvisory)
    }

    fn language(&self) -> Option<&str> {
        self.raw().text(ItemKey::Language)
    }

    fn script(&self) -> Option<&str> {
        self.raw().text(ItemKey::Script)
    }

    /// Star rating, 1-5, from a popularimeter frame.
    fn rating(&self) -> Option<u8> {
        self.raw().rating()
    }

    /// Play count from a popularimeter frame. ID3v2 only.
    fn play_count(&self) -> Option<u64> {
        self.raw().play_count()
    }

    // --------------------------------------------------------------------------- text and lyrics

    fn comment(&self) -> Option<&str> {
        self.raw().text(ItemKey::Comment)
    }

    fn description(&self) -> Option<&str> {
        self.raw().text(ItemKey::Description)
    }

    /// Timed lyrics, where the format supports them.
    fn lyrics(&self) -> Option<&str> {
        self.raw().text(ItemKey::Lyrics)
    }

    fn unsynced_lyrics(&self) -> Option<&str> {
        self.raw().text(ItemKey::UnsyncLyrics)
    }

    fn copyright(&self) -> Option<&str> {
        self.raw().text(ItemKey::CopyrightMessage)
    }

    fn license(&self) -> Option<&str> {
        self.raw().text(ItemKey::License)
    }

    // ------------------------------------------------------------------- release and identifiers

    fn publisher(&self) -> Option<&str> {
        self.raw().text(ItemKey::Publisher)
    }

    fn label(&self) -> Option<&str> {
        self.raw().text(ItemKey::Label)
    }

    fn catalog_number(&self) -> Option<&str> {
        self.raw().text(ItemKey::CatalogNumber)
    }

    fn barcode(&self) -> Option<&str> {
        self.raw().text(ItemKey::Barcode)
    }

    fn isrc(&self) -> Option<&str> {
        self.raw().text(ItemKey::Isrc)
    }

    fn release_country(&self) -> Option<&str> {
        self.raw().text(ItemKey::ReleaseCountry)
    }

    fn original_media_type(&self) -> Option<&str> {
        self.raw().text(ItemKey::OriginalMediaType)
    }

    fn acoust_id(&self) -> Option<&str> {
        self.raw().text(ItemKey::AcoustId)
    }

    fn acoust_id_fingerprint(&self) -> Option<&str> {
        self.raw().text(ItemKey::AcoustIdFingerprint)
    }

    fn musicbrainz_recording_id(&self) -> Option<&str> {
        self.raw().text(ItemKey::MusicBrainzRecordingId)
    }

    fn musicbrainz_track_id(&self) -> Option<&str> {
        self.raw().text(ItemKey::MusicBrainzTrackId)
    }

    fn musicbrainz_release_id(&self) -> Option<&str> {
        self.raw().text(ItemKey::MusicBrainzReleaseId)
    }

    fn musicbrainz_release_group_id(&self) -> Option<&str> {
        self.raw().text(ItemKey::MusicBrainzReleaseGroupId)
    }

    fn musicbrainz_artist_id(&self) -> Option<&str> {
        self.raw().text(ItemKey::MusicBrainzArtistId)
    }

    fn musicbrainz_release_artist_id(&self) -> Option<&str> {
        self.raw().text(ItemKey::MusicBrainzReleaseArtistId)
    }

    fn musicbrainz_work_id(&self) -> Option<&str> {
        self.raw().text(ItemKey::MusicBrainzWorkId)
    }

    fn musicbrainz_release_type(&self) -> Option<&str> {
        self.raw().text(ItemKey::MusicBrainzReleaseType)
    }

    // --------------------------------------------------------------------------------- ReplayGain

    /// Track gain in dB.
    fn replay_gain_track_gain(&self) -> Option<f32> {
        self.raw().float(ItemKey::ReplayGainTrackGain)
    }

    /// Track peak amplitude, where 1.0 is full scale.
    fn replay_gain_track_peak(&self) -> Option<f32> {
        self.raw().float(ItemKey::ReplayGainTrackPeak)
    }

    /// Album gain in dB.
    fn replay_gain_album_gain(&self) -> Option<f32> {
        self.raw().float(ItemKey::ReplayGainAlbumGain)
    }

    /// Album peak amplitude, where 1.0 is full scale.
    fn replay_gain_album_peak(&self) -> Option<f32> {
        self.raw().float(ItemKey::ReplayGainAlbumPeak)
    }

    /// EBU R128 track gain, in Q7.8 dB units as stored.
    fn r128_track_gain(&self) -> Option<i32> {
        self.raw().integer(ItemKey::R128TrackGain)
    }

    /// EBU R128 album gain, in Q7.8 dB units as stored.
    fn r128_album_gain(&self) -> Option<i32> {
        self.raw().integer(ItemKey::R128AlbumGain)
    }

    // ------------------------------------------------------------------------------------- encoder

    fn encoded_by(&self) -> Option<&str> {
        self.raw().text(ItemKey::EncodedBy)
    }

    fn encoder(&self) -> Option<&str> {
        self.raw().text(ItemKey::EncoderSoftware)
    }

    fn encoder_settings(&self) -> Option<&str> {
        self.raw().text(ItemKey::EncoderSettings)
    }

    fn file_owner(&self) -> Option<&str> {
        self.raw().text(ItemKey::FileOwner)
    }

    fn original_file_name(&self) -> Option<&str> {
        self.raw().text(ItemKey::OriginalFileName)
    }

    // ---------------------------------------------------------------------------- radio and podcast

    fn radio_station_name(&self) -> Option<&str> {
        self.raw().text(ItemKey::InternetRadioStationName)
    }

    fn radio_station_owner(&self) -> Option<&str> {
        self.raw().text(ItemKey::InternetRadioStationOwner)
    }

    fn is_podcast(&self) -> Option<bool> {
        self.raw().flag(ItemKey::FlagPodcast)
    }

    fn podcast_description(&self) -> Option<&str> {
        self.raw().text(ItemKey::PodcastDescription)
    }

    fn podcast_series_category(&self) -> Option<&str> {
        self.raw().text(ItemKey::PodcastSeriesCategory)
    }

    fn podcast_keywords(&self) -> Option<&str> {
        self.raw().text(ItemKey::PodcastKeywords)
    }

    fn podcast_id(&self) -> Option<&str> {
        self.raw().text(ItemKey::PodcastGlobalUniqueId)
    }

    fn podcast_url(&self) -> Option<&str> {
        self.raw().text(ItemKey::PodcastUrl)
    }

    // --------------------------------------------------------------------------------------- URLs

    fn audio_file_url(&self) -> Option<&str> {
        self.raw().text(ItemKey::AudioFileUrl)
    }

    fn audio_source_url(&self) -> Option<&str> {
        self.raw().text(ItemKey::AudioSourceUrl)
    }

    fn artist_url(&self) -> Option<&str> {
        self.raw().text(ItemKey::TrackArtistUrl)
    }

    fn publisher_url(&self) -> Option<&str> {
        self.raw().text(ItemKey::PublisherUrl)
    }

    fn copyright_url(&self) -> Option<&str> {
        self.raw().text(ItemKey::CopyrightUrl)
    }

    fn commercial_url(&self) -> Option<&str> {
        self.raw().text(ItemKey::CommercialInformationUrl)
    }

    fn radio_station_url(&self) -> Option<&str> {
        self.raw().text(ItemKey::RadioStationUrl)
    }

    fn payment_url(&self) -> Option<&str> {
        self.raw().text(ItemKey::PaymentUrl)
    }

    // ------------------------------------------------------------------------------------ artwork

    /// Every embedded image, in the order the file stores them.
    fn artwork(&self) -> &[Artwork] {
        self.raw().artwork()
    }

    /// The front cover, or the first image if none is labelled as such.
    fn front_cover(&self) -> Option<&Artwork> {
        let artwork = self.raw().artwork();

        artwork
            .iter()
            .find(|art| art.kind == Some(ArtworkKind::FrontCover))
            .or_else(|| artwork.first())
    }

    // ---------------------------------------------------------------------------------- the stream

    fn path(&self) -> &Path {
        self.raw().path()
    }

    fn file_size(&self) -> Option<u64> {
        self.raw().file_size()
    }

    /// Short container name, e.g. `ogg`. Falls back to the tag reader's identification when the
    /// demuxer could not read the file.
    fn container(&self) -> Option<&str> {
        self.raw()
            .tech()
            .container
            .or_else(|| self.raw().file_type().map(|ft| codecs::file_type_names(ft).0))
    }

    /// Descriptive container name, e.g. `OGG`.
    fn container_long(&self) -> Option<&str> {
        self.raw()
            .tech()
            .container_long
            .or_else(|| self.raw().file_type().map(|ft| codecs::file_type_names(ft).1))
    }

    /// Codec profile, e.g. AAC's `LC`.
    fn codec_profile(&self) -> Option<&str> {
        self.raw().tech().profile.as_deref()
    }

    fn sample_rate(&self) -> Option<u32> {
        self.raw().tech().sample_rate
    }

    /// Bits per decoded sample. `None` for lossy codecs, which have no such notion.
    fn bits_per_sample(&self) -> Option<u32> {
        self.raw().tech().bits_per_sample
    }

    /// Bits per coded sample, when the encoded width differs from the decoded one.
    fn bits_per_coded_sample(&self) -> Option<u32> {
        self.raw().tech().bits_per_coded_sample
    }

    /// The sample format the decoder produces.
    fn sample_format(&self) -> Option<SampleFormat> {
        self.raw().tech().sample_format
    }

    fn channel_count(&self) -> Option<usize> {
        self.raw().tech().channel_count
    }

    /// Channel layout, rendered for display, e.g. `FRONT_LEFT | FRONT_RIGHT`.
    fn channel_layout(&self) -> Option<&str> {
        self.raw().tech().channel_layout.as_deref()
    }

    /// Playable duration of the stream, excluding encoder delay and padding where the container
    /// states them. Falls back to the tag reader's own measurement.
    fn duration(&self) -> Option<Duration> {
        self.raw().tech().duration.or_else(|| self.raw().tagged_duration())
    }

    /// Playable audio frames, excluding encoder delay and padding.
    fn frame_count(&self) -> Option<u64> {
        self.raw().tech().num_frames
    }

    /// Leading frames to discard for gapless playback.
    fn encoder_delay(&self) -> Option<u32> {
        self.raw().tech().encoder_delay
    }

    /// Trailing frames to discard for gapless playback.
    fn encoder_padding(&self) -> Option<u32> {
        self.raw().tech().encoder_padding
    }

    /// Bitrate of the audio stream in kbps.
    fn audio_bitrate(&self) -> Option<u32> {
        self.raw().audio_bitrate()
    }

    /// Bitrate of the whole file, tags and container overhead included, in kbps.
    fn overall_bitrate(&self) -> Option<u32> {
        self.raw().overall_bitrate()
    }

    /// Language the container declares on the audio track, as opposed to the tagged language.
    fn track_language(&self) -> Option<&str> {
        self.raw().tech().track_language.as_deref()
    }

    /// Tracks of any type in the container.
    fn track_count(&self) -> usize {
        self.raw().tech().track_count
    }

    /// Chapters the container declares, when it declares any.
    fn chapter_count(&self) -> Option<usize> {
        self.raw().tech().chapter_count
    }

    // ------------------------------------------------------------------------------ escape hatches

    /// Tag formats present in the file, e.g. ID3v2 alongside APEv2.
    fn tag_formats(&self) -> Vec<TagType> {
        self.raw().tag_formats()
    }

    /// Container tags that no accessor above covers, exactly as the file spells them.
    fn container_extra(&self) -> &[ContainerTag] {
        self.raw().container_extra()
    }
}

/// Extensions this build can read, lowercase and without the dot.
///
/// Extension matching is a cheap pre-filter for walking a library; [`probe`] is what actually
/// decides whether a file is readable.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aif", "aifc", "aiff", "alac", "ape", "caf", "flac", "m4a", "m4b", "mka", "mp1", "mp2",
    "mp3", "mp4", "mpc", "oga", "ogg", "opus", "spx", "wav", "wave", "wv",
];

/// Whether `path` has the extension of an audio file, ignoring case.
pub fn has_audio_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| AUDIO_EXTENSIONS.contains(&ext.as_str()))
}
