//! Embedded artwork, normalized across the two metadata sources.

use lofty::picture::{Picture, PictureType};
use symphonia::core::meta::{StandardVisualKey, Visual};

/// What an embedded image depicts. Mirrors the ID3v2 APIC picture type set, which both Lofty and
/// Symphonia key their own enums off of.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ArtworkKind {
    Other,
    FileIcon,
    OtherFileIcon,
    FrontCover,
    BackCover,
    Leaflet,
    Media,
    LeadArtist,
    Artist,
    Conductor,
    Band,
    Composer,
    Lyricist,
    RecordingLocation,
    DuringRecording,
    DuringPerformance,
    ScreenCapture,
    BrightFish,
    Illustration,
    BandLogo,
    PublisherLogo,
}

/// One embedded image plus whatever the container said about it.
#[derive(Clone, Debug)]
pub struct Artwork {
    /// What the image depicts, if the container said.
    pub kind: Option<ArtworkKind>,
    /// Media type of `data`, e.g. `image/jpeg`.
    pub mime: Option<String>,
    /// Free-form description.
    pub description: Option<String>,
    /// Pixel width, as claimed by the metadata (not by the image itself).
    pub width: Option<u32>,
    /// Pixel height, as claimed by the metadata (not by the image itself).
    pub height: Option<u32>,
    /// The encoded image bytes.
    pub data: Vec<u8>,
}

impl Artwork {
    /// Build from a Lofty picture. Lofty always knows a picture type, never the dimensions.
    pub(crate) fn from_lofty(pic: &Picture) -> Self {
        let kind = match pic.pic_type() {
            PictureType::Other => ArtworkKind::Other,
            PictureType::Icon => ArtworkKind::FileIcon,
            PictureType::OtherIcon => ArtworkKind::OtherFileIcon,
            PictureType::CoverFront => ArtworkKind::FrontCover,
            PictureType::CoverBack => ArtworkKind::BackCover,
            PictureType::Leaflet => ArtworkKind::Leaflet,
            PictureType::Media => ArtworkKind::Media,
            PictureType::LeadArtist => ArtworkKind::LeadArtist,
            PictureType::Artist => ArtworkKind::Artist,
            PictureType::Conductor => ArtworkKind::Conductor,
            PictureType::Band => ArtworkKind::Band,
            PictureType::Composer => ArtworkKind::Composer,
            PictureType::Lyricist => ArtworkKind::Lyricist,
            PictureType::RecordingLocation => ArtworkKind::RecordingLocation,
            PictureType::DuringRecording => ArtworkKind::DuringRecording,
            PictureType::DuringPerformance => ArtworkKind::DuringPerformance,
            PictureType::ScreenCapture => ArtworkKind::ScreenCapture,
            PictureType::BrightFish => ArtworkKind::BrightFish,
            PictureType::Illustration => ArtworkKind::Illustration,
            PictureType::BandLogo => ArtworkKind::BandLogo,
            PictureType::PublisherLogo => ArtworkKind::PublisherLogo,
            _ => ArtworkKind::Other,
        };

        Artwork {
            kind: Some(kind),
            mime: pic.mime_type().map(|m| m.to_string()),
            description: pic.description().map(str::to_owned),
            width: None,
            height: None,
            data: pic.data().to_vec(),
        }
    }

    /// Build from a Symphonia visual. Used for containers Lofty cannot parse (MKV, CAF).
    pub(crate) fn from_symphonia(visual: &Visual) -> Self {
        let kind = visual.usage.map(|usage| match usage {
            StandardVisualKey::FileIcon => ArtworkKind::FileIcon,
            StandardVisualKey::OtherIcon => ArtworkKind::OtherFileIcon,
            StandardVisualKey::FrontCover => ArtworkKind::FrontCover,
            StandardVisualKey::BackCover => ArtworkKind::BackCover,
            StandardVisualKey::Leaflet => ArtworkKind::Leaflet,
            StandardVisualKey::Media => ArtworkKind::Media,
            StandardVisualKey::LeadArtistPerformerSoloist => ArtworkKind::LeadArtist,
            StandardVisualKey::ArtistPerformer => ArtworkKind::Artist,
            StandardVisualKey::Conductor => ArtworkKind::Conductor,
            StandardVisualKey::BandOrchestra => ArtworkKind::Band,
            StandardVisualKey::Composer => ArtworkKind::Composer,
            StandardVisualKey::Lyricist => ArtworkKind::Lyricist,
            StandardVisualKey::RecordingLocation => ArtworkKind::RecordingLocation,
            StandardVisualKey::RecordingSession => ArtworkKind::DuringRecording,
            StandardVisualKey::Performance => ArtworkKind::DuringPerformance,
            StandardVisualKey::ScreenCapture => ArtworkKind::ScreenCapture,
            StandardVisualKey::Illustration => ArtworkKind::Illustration,
            StandardVisualKey::BandArtistLogo => ArtworkKind::BandLogo,
            StandardVisualKey::PublisherStudioLogo => ArtworkKind::PublisherLogo,
            _ => ArtworkKind::Other,
        });

        Artwork {
            kind,
            mime: visual.media_type.clone(),
            description: None,
            width: visual.dimensions.map(|d| d.width),
            height: visual.dimensions.map(|d| d.height),
            data: visual.data.to_vec(),
        }
    }
}
