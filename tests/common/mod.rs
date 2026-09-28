//! Fixtures shared by the integration tests.

#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};

/// Write a mono 16-bit WAV carrying RIFF INFO tags, and return its path.
pub fn write_tagged_wav(dir: &Path) -> PathBuf {
    const SAMPLE_RATE: u32 = 44_100;
    const FRAMES: u32 = SAMPLE_RATE / 2; // half a second

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(body.len() + 9);
        out.extend_from_slice(id);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        // Chunks are word aligned.
        if body.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    /// An INFO sub-chunk, whose value is a NUL-terminated string.
    fn info(id: &[u8; 4], value: &str) -> Vec<u8> {
        let mut body = value.as_bytes().to_vec();
        body.push(0);
        chunk(id, &body)
    }

    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&1u16.to_le_bytes()); // mono
    fmt.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    fmt.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // bytes per second
    fmt.extend_from_slice(&2u16.to_le_bytes()); // block align
    fmt.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    let mut list = b"INFO".to_vec();
    list.extend(info(b"INAM", "Test Title"));
    list.extend(info(b"IART", "Test Artist"));
    list.extend(info(b"IPRD", "Test Album"));
    list.extend(info(b"ICRD", "2024-06-01"));
    list.extend(info(b"IGNR", "Ambient"));
    list.extend(info(b"ITRK", "3"));

    // A quiet sine, so the samples are not all identical.
    let samples: Vec<u8> = (0..FRAMES)
        .flat_map(|frame| {
            let phase = frame as f32 / SAMPLE_RATE as f32 * 440.0 * std::f32::consts::TAU;
            ((phase.sin() * 8000.0) as i16).to_le_bytes()
        })
        .collect();

    let mut body = b"WAVE".to_vec();
    body.extend(chunk(b"fmt ", &fmt));
    body.extend(chunk(b"LIST", &list));
    body.extend(chunk(b"data", &samples));

    let path = dir.join("tagged.wav");
    let mut file = std::fs::File::create(&path).expect("create wav");
    file.write_all(&chunk(b"RIFF", &body)).expect("write wav");

    path
}

/// A scratch directory, emptied first so each run starts clean.
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ogma-tests-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");

    dir
}

/// A tagged WAV under its own scratch directory, named after the caller's test.
pub fn tagged_wav(name: &str) -> PathBuf {
    write_tagged_wav(&scratch_dir(name))
}

/// A small PNG gradient, for exercising the artwork path.
pub fn png_gradient(size: u32) -> Vec<u8> {
    fn chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        let mut payload = kind.to_vec();
        payload.extend_from_slice(body);
        out.extend_from_slice(&payload);
        out.extend_from_slice(&crc32(&payload).to_be_bytes());
        out
    }

    // PNG needs a CRC per chunk, and pulling in a crate for six lines is not worth it.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;

        for byte in bytes {
            crc ^= u32::from(*byte);

            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }

        !crc
    }

    // One filter byte per row, then RGB triples.
    let mut raw = Vec::new();
    for y in 0..size {
        raw.push(0);
        for x in 0..size {
            raw.push((x * 255 / size) as u8);
            raw.push((y * 255 / size) as u8);
            raw.push(128);
        }
    }

    let mut header = Vec::new();
    header.extend_from_slice(&size.to_be_bytes());
    header.extend_from_slice(&size.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB, no interlacing

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(chunk(b"IHDR", &header));
    png.extend(chunk(b"IDAT", &deflate_stored(&raw)));
    png.extend(chunk(b"IEND", b""));

    png
}

/// A zlib stream of stored (uncompressed) deflate blocks, which every PNG decoder accepts.
fn deflate_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01]; // zlib header, no preset dictionary

    for (index, block) in data.chunks(65_535).enumerate() {
        let last = u8::from((index + 1) * 65_535 >= data.len());
        let len = block.len() as u16;

        out.push(last);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }

    // Adler-32 of the uncompressed data.
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    out.extend_from_slice(&((b << 16) | a).to_be_bytes());

    out
}

/// A FLAC with an embedded cover, written by re-tagging a copy of the WAV fixture through Lofty.
///
/// Returns the path, or `None` when the picture could not be attached.
pub fn wav_with_cover(dir: &Path) -> Option<PathBuf> {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::probe::Probe;
    use lofty::tag::{Tag, TagType};

    let path = write_tagged_wav(dir);

    let mut tagged = Probe::open(&path).ok()?.read().ok()?;
    let tag_type = tagged.primary_tag_type();

    if tagged.tag(tag_type).is_none() {
        tagged.insert_tag(Tag::new(TagType::Id3v2));
    }

    let tag = tagged.primary_tag_mut()?;
    tag.push_picture(
        Picture::unchecked(png_gradient(64))
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Png)
            .build(),
    );

    tagged.save_to_path(&path, WriteOptions::default()).ok()?;

    Some(path)
}

/// What a purpose-built WAV fixture should say about itself.
#[derive(Clone, Debug)]
pub struct WavSpec<'a> {
    pub name: &'a str,
    pub title: Option<&'a str>,
    pub artist: Option<&'a str>,
    pub album: Option<&'a str>,
    pub track: Option<u32>,
    pub year: Option<&'a str>,
    /// Length of the audio, which is what a sort by length has to work with.
    pub millis: u64,
}

impl<'a> Default for WavSpec<'a> {
    fn default() -> Self {
        WavSpec {
            name: "track.wav",
            title: None,
            artist: None,
            album: None,
            track: None,
            year: None,
            millis: 500,
        }
    }
}

/// Write a WAV carrying exactly the tags `spec` names, for testing sorts.
pub fn write_wav(dir: &Path, spec: &WavSpec<'_>) -> PathBuf {
    const RATE: u32 = 44_100;

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(body.len() + 9);
        out.extend_from_slice(id);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        if body.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn info(id: &[u8; 4], value: &str) -> Vec<u8> {
        let mut body = value.as_bytes().to_vec();
        body.push(0);
        chunk(id, &body)
    }

    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&RATE.to_le_bytes());
    fmt.extend_from_slice(&(RATE * 2).to_le_bytes());
    fmt.extend_from_slice(&2u16.to_le_bytes());
    fmt.extend_from_slice(&16u16.to_le_bytes());

    let mut list = b"INFO".to_vec();
    if let Some(title) = spec.title {
        list.extend(info(b"INAM", title));
    }
    if let Some(artist) = spec.artist {
        list.extend(info(b"IART", artist));
    }
    if let Some(album) = spec.album {
        list.extend(info(b"IPRD", album));
    }
    if let Some(track) = spec.track {
        list.extend(info(b"ITRK", &track.to_string()));
    }
    if let Some(year) = spec.year {
        list.extend(info(b"ICRD", year));
    }

    let frames = (u64::from(RATE) * spec.millis / 1000) as u32;
    let samples: Vec<u8> = (0..frames)
        .flat_map(|frame| {
            let phase = frame as f32 / RATE as f32 * 330.0 * std::f32::consts::TAU;
            ((phase.sin() * 6000.0) as i16).to_le_bytes()
        })
        .collect();

    let mut body = b"WAVE".to_vec();
    body.extend(chunk(b"fmt ", &fmt));
    if list.len() > 4 {
        body.extend(chunk(b"LIST", &list));
    }
    body.extend(chunk(b"data", &samples));

    let path = dir.join(spec.name);
    std::fs::write(&path, chunk(b"RIFF", &body)).expect("write wav");

    path
}
