//! Bridge from Symphonia's `StandardTag` to Lofty's `ItemKey`.
//!
//! Lofty is the primary tag source, but it cannot parse every container Symphonia can demux (MKV
//! and CAF most notably). For those files the tags Symphonia found during the probe are normalized
//! through this table so the same accessors keep working.

use lofty::tag::ItemKey;
use symphonia::core::meta::{RawValue, StandardTag};

/// Map a Symphonia standard tag onto the equivalent Lofty key, if there is one.
pub(crate) fn item_key_of(std: &StandardTag) -> Option<ItemKey> {
    use StandardTag as S;

    let key = match std {
        S::AcoustIdFingerprint(_) => ItemKey::AcoustIdFingerprint,
        S::AcoustIdId(_) => ItemKey::AcoustId,
        S::Album(_) => ItemKey::AlbumTitle,
        S::AlbumArtist(_) => ItemKey::AlbumArtist,
        S::Arranger(_) => ItemKey::Arranger,
        S::Artist(_) => ItemKey::TrackArtist,
        S::Bpm(_) => ItemKey::Bpm,
        S::Comment(_) => ItemKey::Comment,
        S::CompilationFlag(_) => ItemKey::FlagCompilation,
        S::Composer(_) => ItemKey::Composer,
        S::Conductor(_) => ItemKey::Conductor,
        S::Copyright(_) => ItemKey::CopyrightMessage,
        S::Description(_) => ItemKey::Description,
        S::Director(_) => ItemKey::Director,
        S::DiscNumber(_) => ItemKey::DiscNumber,
        S::DiscTotal(_) => ItemKey::DiscTotal,
        S::EncodedBy(_) => ItemKey::EncodedBy,
        S::Encoder(_) => ItemKey::EncoderSoftware,
        S::EncoderSettings(_) => ItemKey::EncoderSettings,
        S::EncodingDate(_) => ItemKey::EncodingTime,
        S::Engineer(_) => ItemKey::Engineer,
        S::Genre(_) => ItemKey::Genre,
        S::Grouping(_) => ItemKey::ContentGroup,
        S::IdentBarcode(_) => ItemKey::Barcode,
        S::IdentCatalogNumber(_) => ItemKey::CatalogNumber,
        S::IdentIsrc(_) => ItemKey::Isrc,
        S::InitialKey(_) => ItemKey::InitialKey,
        S::InternetRadioName(_) => ItemKey::InternetRadioStationName,
        S::InternetRadioOwner(_) => ItemKey::InternetRadioStationOwner,
        S::Label(_) => ItemKey::Label,
        S::Language(_) => ItemKey::Language,
        S::License(_) => ItemKey::License,
        S::Lyricist(_) => ItemKey::Lyricist,
        S::Lyrics(_) => ItemKey::Lyrics,
        S::MediaFormat(_) => ItemKey::OriginalMediaType,
        S::MixDj(_) => ItemKey::MixDj,
        S::MixEngineer(_) => ItemKey::MixEngineer,
        S::Mood(_) => ItemKey::Mood,
        S::MovementName(_) => ItemKey::Movement,
        S::MovementNumber(_) => ItemKey::MovementNumber,
        S::MovementTotal(_) => ItemKey::MovementTotal,
        S::MusicBrainzAlbumArtistId(_) => ItemKey::MusicBrainzReleaseArtistId,
        S::MusicBrainzAlbumId(_) => ItemKey::MusicBrainzReleaseId,
        S::MusicBrainzArtistId(_) => ItemKey::MusicBrainzArtistId,
        S::MusicBrainzRecordingId(_) => ItemKey::MusicBrainzRecordingId,
        S::MusicBrainzReleaseGroupId(_) => ItemKey::MusicBrainzReleaseGroupId,
        S::MusicBrainzReleaseTrackId(_) => ItemKey::MusicBrainzTrackId,
        S::MusicBrainzReleaseType(_) => ItemKey::MusicBrainzReleaseType,
        S::MusicBrainzTrackId(_) => ItemKey::MusicBrainzTrackId,
        S::MusicBrainzWorkId(_) => ItemKey::MusicBrainzWorkId,
        S::OriginalAlbum(_) => ItemKey::OriginalAlbumTitle,
        S::OriginalArtist(_) => ItemKey::OriginalArtist,
        S::OriginalFile(_) => ItemKey::OriginalFileName,
        S::OriginalLyricist(_) => ItemKey::OriginalLyricist,
        S::OriginalReleaseDate(_) => ItemKey::OriginalReleaseDate,
        S::OriginalReleaseYear(_) => ItemKey::OriginalReleaseDate,
        S::Owner(_) => ItemKey::FileOwner,
        S::Performer(_) => ItemKey::Performer,
        S::PodcastCategory(_) => ItemKey::PodcastSeriesCategory,
        S::PodcastDescription(_) => ItemKey::PodcastDescription,
        S::PodcastFlag(_) => ItemKey::FlagPodcast,
        S::PodcastKeywords(_) => ItemKey::PodcastKeywords,
        S::Producer(_) => ItemKey::Producer,
        S::RecordingDate(_) => ItemKey::RecordingDate,
        S::RecordingYear(_) => ItemKey::Year,
        S::ReleaseCountry(_) => ItemKey::ReleaseCountry,
        S::ReleaseDate(_) => ItemKey::ReleaseDate,
        S::ReleaseYear(_) => ItemKey::Year,
        S::Remixer(_) => ItemKey::Remixer,
        S::ReplayGainAlbumGain(_) => ItemKey::ReplayGainAlbumGain,
        S::ReplayGainAlbumPeak(_) => ItemKey::ReplayGainAlbumPeak,
        S::ReplayGainTrackGain(_) => ItemKey::ReplayGainTrackGain,
        S::ReplayGainTrackPeak(_) => ItemKey::ReplayGainTrackPeak,
        S::Script(_) => ItemKey::Script,
        S::SortAlbum(_) => ItemKey::AlbumTitleSortOrder,
        S::SortAlbumArtist(_) => ItemKey::AlbumArtistSortOrder,
        S::SortArtist(_) => ItemKey::TrackArtistSortOrder,
        S::SortComposer(_) => ItemKey::ComposerSortOrder,
        S::SortTrackTitle(_) => ItemKey::TrackTitleSortOrder,
        S::TaggingDate(_) => ItemKey::TaggingTime,
        S::TrackNumber(_) => ItemKey::TrackNumber,
        S::TrackSubtitle(_) => ItemKey::TrackSubtitle,
        S::TrackTitle(_) => ItemKey::TrackTitle,
        S::TrackTotal(_) => ItemKey::TrackTotal,
        S::Url(_) => ItemKey::AudioFileUrl,
        S::UrlArtist(_) => ItemKey::TrackArtistUrl,
        S::UrlCopyright(_) => ItemKey::CopyrightUrl,
        S::UrlInternetRadio(_) => ItemKey::RadioStationUrl,
        S::UrlPayment(_) => ItemKey::PaymentUrl,
        S::UrlPodcast(_) => ItemKey::PodcastUrl,
        S::UrlPurchase(_) => ItemKey::CommercialInformationUrl,
        S::UrlSource(_) => ItemKey::AudioSourceUrl,
        S::Work(_) => ItemKey::Work,
        S::Writer(_) => ItemKey::Writer,
        _ => return None,
    };

    Some(key)
}

/// Render a raw tag value as the flat string the accessors expect. Binary values are dropped, since
/// there is no sensible textual form for them.
pub(crate) fn value_to_string(value: &RawValue) -> Option<String> {
    let text = match value {
        RawValue::Binary(_) => return None,
        RawValue::Boolean(b) => u8::from(*b).to_string(),
        RawValue::Flag => "1".to_string(),
        RawValue::Float(f) => f.to_string(),
        RawValue::SignedInt(i) => i.to_string(),
        RawValue::String(s) => s.to_string(),
        RawValue::StringList(list) => list.join("; "),
        RawValue::UnsignedInt(u) => u.to_string(),
        _ => return None,
    };

    Some(text)
}
