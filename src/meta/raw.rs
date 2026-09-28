//! Everything read off disk for one file, and the code that reads it.
//!
//! Two sources are combined:
//!
//! * **Lofty** parses the tags. Its `ItemKey` normalizes ID3v2 frames, MP4 atoms, Vorbis comments,
//!   APE items and RIFF/AIFF chunks onto one key space, including the awkward ones (TXXX, iTunes
//!   freeform atoms, ReplayGain, MusicBrainz ids, sort orders).
//! * **Symphonia** demuxes the container and reports the codec actually inside it (an `.m4a` may be
//!   AAC or ALAC), exact frame counts, and encoder delay/padding. It also supplies the tags for
//!   containers Lofty does not handle at all (MKV, CAF).
//!
//! Either source is allowed to fail on its own; only a file both reject is an error.

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use lofty::file::{AudioFile as _, FileType, TaggedFileExt as _};
use lofty::properties::FileProperties;
use lofty::tag::{ItemKey, Tag, TagType};
use symphonia::core::audio::Channels;
use symphonia::core::audio::sample::SampleFormat;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::{AudioCodecId, AudioCodecParameters};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::TrackType;
use symphonia::core::units::{Duration as SymDuration, TimeBase};
use symphonia::core::io::MediaSourceStream;
use symphonia_adapter_libopus::OpusDecoder;

use super::artwork::Artwork;
use super::codecs;
use super::sym_map;
use super::{AudioFile, MetaError};

/// The codec registry used throughout the player: everything Symphonia was built with, plus the
/// libopus adapter, which is not part of Symphonia's own default registry.
pub fn codec_registry() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();

    REGISTRY.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        registry.register_audio_decoder::<OpusDecoder>();
        registry
    })
}

/// Technical facts about the stream, as opposed to the tags describing the music.
///
/// Every field is optional because containers vary in what they state: a raw MP3 stream knows its
/// sample rate but not its length, while a FLAC header knows both.
#[derive(Clone, Debug, Default)]
pub struct Technical {
    /// Short container name, e.g. `ogg`.
    pub container: Option<&'static str>,
    /// Descriptive container name, e.g. `OGG`.
    pub container_long: Option<&'static str>,
    /// Symphonia's id for the codec found inside the container.
    pub codec_id: Option<AudioCodecId>,
    /// Short codec name as reported by the codec registry.
    pub codec_registered: Option<&'static str>,
    /// Descriptive codec name as reported by the codec registry.
    pub codec_registered_long: Option<&'static str>,
    /// Codec-specific profile (AAC-LC, MPEG layer, ...), rendered for display.
    pub profile: Option<String>,
    pub sample_rate: Option<u32>,
    /// Bits per decoded sample. Meaningful for PCM and the lossless codecs.
    pub bits_per_sample: Option<u32>,
    /// Bits per *coded* sample, when it differs from the decoded width.
    pub bits_per_coded_sample: Option<u32>,
    /// Decoded sample format, rendered for display.
    pub sample_format: Option<SampleFormat>,
    pub channel_count: Option<usize>,
    /// Channel layout, rendered for display.
    pub channel_layout: Option<String>,
    /// Playable frames, excluding encoder delay and padding.
    pub num_frames: Option<u64>,
    pub duration: Option<Duration>,
    /// Leading frames the encoder inserted, to be discarded on playback.
    pub encoder_delay: Option<u32>,
    /// Trailing padding frames the encoder inserted, to be discarded on playback.
    pub encoder_padding: Option<u32>,
    /// Language declared on the audio track by the container.
    pub track_language: Option<String>,
    /// How many tracks of any type the container holds.
    pub track_count: usize,
    /// How many chapters the container declares, when it declares any.
    pub chapter_count: Option<usize>,
}

/// One tag exactly as the container stated it, for keys no normalized accessor covers.
#[derive(Clone, Debug)]
pub struct ContainerTag {
    pub key: String,
    pub value: String,
}

/// The shared body of every `AudioFile` implementation.
///
/// Holds the parsed tags, the artwork and the technical facts. Lookups are ordered: the file's
/// primary tag first, then its remaining tags, then anything Symphonia recovered from the container.
pub struct RawMeta {
    path: PathBuf,
    file_size: Option<u64>,
    /// What Lofty thinks the file is. The last word on naming files Symphonia could not demux.
    file_type: Option<FileType>,
    /// Lofty tags, primary tag first.
    tags: Vec<Tag>,
    /// Normalized tags recovered from the container by Symphonia, used when Lofty found nothing.
    container: HashMap<ItemKey, String>,
    /// Container tags that map onto no known key.
    container_extra: Vec<ContainerTag>,
    artwork: Vec<Artwork>,
    props: Option<FileProperties>,
    tech: Technical,
}

// Lofty's `Tag` does not implement `Debug`, so summarize the tags rather than dumping them.
impl fmt::Debug for RawMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawMeta")
            .field("path", &self.path)
            .field("file_size", &self.file_size)
            .field("tag_formats", &self.tag_formats())
            .field("container_tags", &self.container.len())
            .field("container_extra", &self.container_extra.len())
            .field("artwork", &self.artwork.len())
            .field("tech", &self.tech)
            .finish()
    }
}

impl RawMeta {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Size of the file on disk.
    pub fn file_size(&self) -> Option<u64> {
        self.file_size
    }

    /// What Lofty identified the file as, which is all there is to go on when the demuxer failed.
    pub fn file_type(&self) -> Option<FileType> {
        self.file_type
    }

    /// Duration as Lofty measured it. Zero means unknown, and is reported as `None`.
    pub fn tagged_duration(&self) -> Option<Duration> {
        self.props
            .as_ref()
            .map(FileProperties::duration)
            .filter(|duration| !duration.is_zero())
    }

    pub fn tech(&self) -> &Technical {
        &self.tech
    }

    pub fn artwork(&self) -> &[Artwork] {
        &self.artwork
    }

    /// Tag formats present in the file, e.g. `ID3v2`, `VorbisComments`.
    pub fn tag_formats(&self) -> Vec<TagType> {
        self.tags.iter().map(Tag::tag_type).collect()
    }

    /// Container tags that no normalized key covers.
    pub fn container_extra(&self) -> &[ContainerTag] {
        &self.container_extra
    }

    /// Bitrate of the audio stream, in kbps.
    pub fn audio_bitrate(&self) -> Option<u32> {
        self.props.as_ref().and_then(FileProperties::audio_bitrate)
    }

    /// Bitrate of the whole file including tags and container overhead, in kbps.
    pub fn overall_bitrate(&self) -> Option<u32> {
        self.props.as_ref().and_then(FileProperties::overall_bitrate)
    }

    /// First value for `key`, searching Lofty's tags then the container tags.
    pub(crate) fn text(&self, key: ItemKey) -> Option<&str> {
        self.tags
            .iter()
            .find_map(|tag| tag.get_string(key))
            .or_else(|| self.container.get(&key).map(String::as_str))
            .filter(|value| !value.is_empty())
    }

    /// Every value for `key` across all tags, deduplicated, in tag order.
    pub(crate) fn texts(&self, key: ItemKey) -> Option<Vec<&str>> {
        let mut values: Vec<&str> = Vec::new();

        for value in self.tags.iter().flat_map(|tag| tag.get_strings(key)) {
            if !value.is_empty() && !values.contains(&value) {
                values.push(value);
            }
        }

        if let Some(value) = self.container.get(&key).map(String::as_str)
            && !value.is_empty()
            && !values.contains(&value)
        {
            values.push(value);
        }

        (!values.is_empty()).then_some(values)
    }

    /// `text`, parsed as an unsigned integer. Handles the `3/12` form ID3v2 uses for track and disc
    /// numbers by taking the part before the slash.
    pub(crate) fn number(&self, key: ItemKey) -> Option<u32> {
        let text = self.text(key)?;
        let head = text.split('/').next().unwrap_or(text);
        head.trim().parse().ok()
    }

    /// `text`, parsed as a float. Tolerates trailing units, so ReplayGain's `-7.23 dB` works.
    pub(crate) fn float(&self, key: ItemKey) -> Option<f32> {
        let text = self.text(key)?.trim();
        let end = text
            .char_indices()
            .take_while(|(i, c)| c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+')))
            .map(|(i, c)| i + c.len_utf8())
            .last()?;

        text[..end].parse().ok()
    }

    /// `text`, parsed as a signed integer.
    pub(crate) fn integer(&self, key: ItemKey) -> Option<i32> {
        self.text(key)?.trim().parse().ok()
    }

    /// `text`, parsed as a flag. Accepts the `1`/`0` and `true`/`false` spellings in the wild.
    pub(crate) fn flag(&self, key: ItemKey) -> Option<bool> {
        match self.text(key)?.trim() {
            "1" | "true" | "True" | "TRUE" | "yes" => Some(true),
            "0" | "false" | "False" | "FALSE" | "no" => Some(false),
            _ => None,
        }
    }

    /// Star rating, 1-5, from the first popularimeter in any tag.
    pub(crate) fn rating(&self) -> Option<u8> {
        self.tags
            .iter()
            .flat_map(|tag| tag.ratings())
            .map(|pop| pop.rating as u8)
            .next()
    }

    /// Play count from the first popularimeter that carries one (ID3v2 only).
    pub(crate) fn play_count(&self) -> Option<u64> {
        self.tags
            .iter()
            .flat_map(|tag| tag.ratings())
            .map(|pop| pop.play_counter)
            .next()
    }
}

/// Read every piece of metadata Symphonia and Lofty can see in `path`, and return it as the struct
/// for the codec the file actually contains.
///
/// The concrete type behind the returned `AudioFile` is chosen from the codec inside the container,
/// not from the file extension: an `.m4a` holding ALAC yields [`AlacFile`](super::AlacFile), one
/// holding AAC yields [`AacFile`](super::AacFile).
pub fn load(path: &Path) -> Result<Box<dyn AudioFile>, MetaError> {
    let file_size = std::fs::metadata(path).ok().map(|meta| meta.len());

    // Both readers are best-effort. Their errors are only fatal together.
    let sym = read_symphonia(path);
    let lofty = lofty::read_from_path(path);

    let (tags, props, file_type, mut artwork, lofty_error) = match lofty {
        Ok(tagged) => {
            let props = tagged.properties().clone();
            let file_type = tagged.file_type();

            // Primary tag first so lookups prefer the format's canonical tag over a secondary one
            // (an MP3 carrying both ID3v2 and APE, say).
            let primary = tagged.primary_tag_type();
            let mut tags: Vec<Tag> = Vec::with_capacity(tagged.tags().len());
            tags.extend(tagged.tags().iter().filter(|tag| tag.tag_type() == primary).cloned());
            tags.extend(tagged.tags().iter().filter(|tag| tag.tag_type() != primary).cloned());

            let artwork = tags
                .iter()
                .flat_map(|tag| tag.pictures())
                .map(Artwork::from_lofty)
                .collect();

            (tags, Some(props), Some(file_type), artwork, None)
        }
        Err(err) => (Vec::new(), None, None, Vec::new(), Some(err.to_string())),
    };

    let (tech, container, container_extra, sym_error) = match sym {
        Ok(probed) => {
            // Only trust Symphonia's artwork when Lofty found none, so pictures are not doubled up.
            if artwork.is_empty() {
                artwork = probed.artwork;
            }

            (probed.tech, probed.tags, probed.extra_tags, None)
        }
        Err(err) => (Technical::default(), HashMap::new(), Vec::new(), Some(err)),
    };

    if lofty_error.is_some() && sym_error.is_some() {
        return Err(MetaError::Unreadable {
            path: path.to_path_buf(),
            symphonia: sym_error,
            lofty: lofty_error,
        });
    }

    let raw = RawMeta {
        path: path.to_path_buf(),
        file_size,
        file_type,
        tags,
        container,
        container_extra,
        artwork,
        props,
        tech,
    };

    Ok(codecs::for_codec(raw))
}

/// What one Symphonia probe yielded.
struct Probed {
    tech: Technical,
    tags: HashMap<ItemKey, String>,
    extra_tags: Vec<ContainerTag>,
    artwork: Vec<Artwork>,
}

fn read_symphonia(path: &Path) -> Result<Probed, String> {
    let file = File::open(path).map_err(|err| err.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(extension);
    }

    let mut reader = symphonia::default::get_probe()
        .probe(&hint, mss, Default::default(), Default::default())
        .map_err(|err| err.to_string())?;

    let format = *reader.format_info();
    let media_info = *reader.media_info();
    let track_count = reader.tracks().len();
    let chapter_count = reader.chapters().map(|group| group.items.len());

    // Prefer an audio track whose codec is known; fall back to the first audio track so that files
    // using a codec Symphonia cannot decode still report their container facts.
    let track = reader
        .first_track_known_codec(TrackType::Audio)
        .or_else(|| reader.first_track(TrackType::Audio))
        .cloned();

    let mut tags = HashMap::new();
    let mut extra_tags = Vec::new();
    let mut artwork = Vec::new();

    // Walk every revision, oldest first, letting newer revisions overwrite older ones.
    let mut metadata = reader.metadata();
    loop {
        if let Some(revision) = metadata.current() {
            for tag in &revision.media.tags {
                let Some(value) = sym_map::value_to_string(&tag.raw.value) else {
                    continue;
                };

                match tag.std.as_ref().and_then(sym_map::item_key_of) {
                    Some(key) => {
                        tags.insert(key, value);
                    }
                    None => extra_tags.push(ContainerTag { key: tag.raw.key.clone(), value }),
                }
            }

            artwork.extend(revision.media.visuals.iter().map(Artwork::from_symphonia));
        }

        if metadata.pop().is_none() {
            break;
        }
    }

    let mut tech = Technical {
        container: Some(format.short_name),
        container_long: Some(format.long_name),
        track_count,
        chapter_count,
        ..Technical::default()
    };

    if let Some(track) = track {
        tech.track_language = track.language.clone();
        tech.num_frames = track.num_frames;
        tech.encoder_delay = track.delay;
        tech.encoder_padding = track.padding;

        if let Some(CodecParameters::Audio(params)) = &track.codec_params {
            apply_audio_params(&mut tech, params);
        }

        // Frame count over sample rate is the most accurate duration available; the container's own
        // duration in timebase units is the fallback.
        tech.duration = match (tech.num_frames, tech.sample_rate) {
            (Some(frames), Some(rate)) if rate > 0 => {
                Some(Duration::from_secs_f64(frames as f64 / f64::from(rate)))
            }
            _ => duration_from_timebase(track.time_base, track.duration),
        };
    }

    // Containers that state a duration for the media as a whole but not per track, MKV among them,
    // only answer here.
    if tech.duration.is_none() {
        tech.duration = duration_from_timebase(media_info.time_base, media_info.duration);
    }

    Ok(Probed { tech, tags, extra_tags, artwork })
}

fn apply_audio_params(tech: &mut Technical, params: &AudioCodecParameters) {
    tech.codec_id = Some(params.codec);
    tech.sample_rate = params.sample_rate;
    tech.bits_per_sample = params.bits_per_sample;
    tech.bits_per_coded_sample = params.bits_per_coded_sample;
    tech.sample_format = params.sample_format;
    tech.channel_count = params.channels.as_ref().map(Channels::count);
    tech.channel_layout = params.channels.as_ref().map(describe_channels);

    if let Some(registered) = codec_registry().get_audio_decoder(params.codec) {
        tech.codec_registered = Some(registered.codec.info.short_name);
        tech.codec_registered_long = Some(registered.codec.info.long_name);

        if let Some(profile) = params.profile {
            // The registry names the profiles a codec supports; fall back to the raw value.
            tech.profile = registered
                .codec
                .info
                .profiles
                .iter()
                .find(|info| info.profile == profile)
                .map(|info| info.short_name.to_string())
                .or_else(|| Some(format!("{profile:?}")));
        }
    }
}

fn duration_from_timebase(
    time_base: Option<TimeBase>,
    duration: Option<SymDuration>,
) -> Option<Duration> {
    let seconds = time_base?.calc_duration(duration?)?.as_secs_f64();

    (seconds > 0.0).then(|| Duration::from_secs_f64(seconds))
}

fn describe_channels(channels: &Channels) -> String {
    match channels {
        Channels::Positioned(positions) => format!("{positions:?}"),
        Channels::Discrete(count) => format!("{count} discrete"),
        Channels::Ambisonic(order) => format!("ambisonic order {order}"),
        Channels::Custom(labels) => format!("{labels:?}"),
        Channels::None => "none".to_string(),
        other => format!("{other:?}"),
    }
}
