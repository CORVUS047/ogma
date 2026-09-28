//! One struct per codec.
//!
//! The tags in a file do not depend on its codec, so every struct here shares [`RawMeta`] and the
//! default accessors on [`AudioFile`]. What each struct contributes is the codec's own character:
//! its name, whether it is lossless, and the fields that are meaningless for it. Lossy codecs, for
//! instance, report no bit depth even when a container claims one, because there is no such thing
//! as a bit depth for an MP3 stream.

use lofty::file::FileType;
use symphonia::core::codecs::audio::AudioCodecId;
use symphonia::core::codecs::audio::well_known::*;

use super::AudioFile;
use super::raw::RawMeta;

/// Short and long names for a file Symphonia could not demux, from what Lofty made of it.
///
/// These name the file rather than its codec, which is as precise as it gets without a demuxer: an
/// MP4 could hold either AAC or ALAC, and Lofty does not say which.
pub(crate) fn file_type_names(file_type: FileType) -> (&'static str, &'static str) {
    match file_type {
        FileType::Aac => ("aac", "Advanced Audio Coding"),
        FileType::Aiff => ("aiff", "Audio Interchange File Format"),
        FileType::Ape => ("ape", "Monkey's Audio"),
        FileType::Flac => ("flac", "Free Lossless Audio Codec"),
        FileType::Mpeg => ("mpeg", "MPEG-1/2 Audio"),
        FileType::Mp4 => ("mp4", "MPEG-4 Audio"),
        FileType::Mpc => ("mpc", "Musepack"),
        FileType::Opus => ("opus", "Opus"),
        FileType::Vorbis => ("vorbis", "Vorbis"),
        FileType::Speex => ("speex", "Speex"),
        FileType::Wav => ("wav", "Waveform Audio File Format"),
        FileType::WavPack => ("wavpack", "WavPack"),
        FileType::Custom(name) => (name, name),
        _ => ("unknown", "Unknown format"),
    }
}

/// Build the struct matching the codec that was found inside the container.
pub(crate) fn for_codec(raw: RawMeta) -> Box<dyn AudioFile> {
    let Some(codec) = raw.tech().codec_id else {
        return Box::new(UnknownFile::new(raw));
    };

    match codec {
        CODEC_ID_MP1 => Box::new(Mp1File(raw)),
        CODEC_ID_MP2 => Box::new(Mp2File(raw)),
        CODEC_ID_MP3 => Box::new(Mp3File(raw)),
        CODEC_ID_AAC => Box::new(AacFile(raw)),
        CODEC_ID_ALAC => Box::new(AlacFile(raw)),
        CODEC_ID_FLAC => Box::new(FlacFile(raw)),
        CODEC_ID_VORBIS => Box::new(VorbisFile(raw)),
        CODEC_ID_OPUS => Box::new(OpusFile(raw)),
        other if is_pcm(other) => Box::new(PcmFile(raw)),
        other if is_adpcm(other) => Box::new(AdpcmFile(raw)),
        _ => Box::new(UnknownFile::new(raw)),
    }
}

/// PCM codec ids occupy the `0x1xx` block.
fn is_pcm(codec: AudioCodecId) -> bool {
    (CODEC_ID_PCM_S32LE..CODEC_ID_ADPCM_G722).contains(&codec)
}

/// ADPCM codec ids occupy the `0x2xx` block.
fn is_adpcm(codec: AudioCodecId) -> bool {
    (CODEC_ID_ADPCM_G722..CODEC_ID_VORBIS).contains(&codec)
}

/// Overrides for lossy codecs: a lossy stream has no sample width, whatever headers a container
/// wrapping it happens to carry.
macro_rules! no_sample_width {
    () => {
        fn bits_per_sample(&self) -> Option<u32> {
            None
        }

        fn bits_per_coded_sample(&self) -> Option<u32> {
            None
        }
    };
}

/// Declare a codec struct: its names, whether it is lossless, and any accessors it overrides.
macro_rules! codec_file {
    (
        $(#[$meta:meta])*
        $name:ident, $short:literal, $long:literal, lossless: $lossless:literal,
        { $($overrides:tt)* }
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name(RawMeta);

        impl $name {
            /// The shared metadata body, for code that wants it without going through the trait.
            pub fn into_raw(self) -> RawMeta {
                self.0
            }
        }

        impl AudioFile for $name {
            fn raw(&self) -> &RawMeta {
                &self.0
            }

            fn codec(&self) -> &str {
                $short
            }

            fn codec_long(&self) -> &str {
                $long
            }

            fn is_lossless(&self) -> Option<bool> {
                Some($lossless)
            }

            $($overrides)*
        }
    };
}

codec_file!(
    /// MPEG-1/2 Audio Layer I.
    Mp1File, "mp1", "MPEG Audio Layer 1", lossless: false, { no_sample_width!(); }
);

codec_file!(
    /// MPEG-1/2 Audio Layer II.
    Mp2File, "mp2", "MPEG Audio Layer 2", lossless: false, { no_sample_width!(); }
);

codec_file!(
    /// MPEG-1/2 Audio Layer III.
    Mp3File, "mp3", "MPEG Audio Layer 3", lossless: false, { no_sample_width!(); }
);

codec_file!(
    /// Advanced Audio Coding, as found in MP4 and ADTS streams.
    AacFile, "aac", "Advanced Audio Coding", lossless: false, { no_sample_width!(); }
);

codec_file!(
    /// Xiph Vorbis, as found in OGG.
    VorbisFile, "vorbis", "Vorbis", lossless: false, { no_sample_width!(); }
);

codec_file!(
    /// Free Lossless Audio Codec.
    FlacFile, "flac", "Free Lossless Audio Codec", lossless: true, {}
);

codec_file!(
    /// Apple Lossless Audio Codec, as found in MP4 and CAF.
    AlacFile, "alac", "Apple Lossless Audio Codec", lossless: true, {}
);

codec_file!(
    /// Uncompressed PCM, as found in WAV, AIFF and CAF.
    PcmFile, "pcm", "Pulse Code Modulation", lossless: true, {}
);

codec_file!(
    /// ADPCM, the compressed sibling of PCM in RIFF containers.
    AdpcmFile, "adpcm", "Adaptive Differential Pulse Code Modulation", lossless: false, {}
);

/// Xiph Opus, as found in OGG.
///
/// Opus always decodes at 48 kHz regardless of the rate the material was captured at, and its
/// container header states a pre-skip that must be discarded rather than played.
#[derive(Debug)]
pub struct OpusFile(RawMeta);

impl OpusFile {
    pub fn into_raw(self) -> RawMeta {
        self.0
    }
}

impl AudioFile for OpusFile {
    fn raw(&self) -> &RawMeta {
        &self.0
    }

    fn codec(&self) -> &str {
        "opus"
    }

    fn codec_long(&self) -> &str {
        "Opus"
    }

    fn is_lossless(&self) -> Option<bool> {
        Some(false)
    }

    no_sample_width!();

    /// Opus decodes at 48 kHz, whatever the original material's rate was.
    fn sample_rate(&self) -> Option<u32> {
        self.0.tech().sample_rate.or(Some(48_000))
    }
}

/// A file whose codec this build cannot identify: a container Symphonia does not demux (WavPack,
/// Monkey's Audio, Musepack) or a codec it has no decoder for.
///
/// Tags and whatever technical facts were readable are still reported; the codec name comes from the
/// codec registry when the codec was at least identified.
#[derive(Debug)]
pub struct UnknownFile {
    raw: RawMeta,
    short: String,
    long: String,
}

impl UnknownFile {
    fn new(raw: RawMeta) -> Self {
        // The codec registry is the best source, then the tag reader's identification of the file,
        // and only then a shrug.
        let names = raw.file_type().map(file_type_names);

        let short = raw
            .tech()
            .codec_registered
            .or(names.map(|(short, _)| short))
            .unwrap_or("unknown")
            .to_string();

        let long = raw
            .tech()
            .codec_registered_long
            .or(names.map(|(_, long)| long))
            .unwrap_or("Unknown codec")
            .to_string();

        UnknownFile { raw, short, long }
    }

    pub fn into_raw(self) -> RawMeta {
        self.raw
    }
}

impl AudioFile for UnknownFile {
    fn raw(&self) -> &RawMeta {
        &self.raw
    }

    fn codec(&self) -> &str {
        &self.short
    }

    fn codec_long(&self) -> &str {
        &self.long
    }

    /// Unknown codec, so nothing can be claimed either way.
    fn is_lossless(&self) -> Option<bool> {
        None
    }
}
