//! Identifying a track by its audio rather than by where it sits.
//!
//! A playlist stores paths, and a path is only true until something moves the file. What does not
//! change under a move is the audio itself, so each track a playlist holds is stored alongside a
//! hash of its first [`PAYLOAD_BYTES`] of encoded audio. A file that has gone missing can then be
//! looked for by that hash rather than by name.
//!
//! The hash covers the *packets a demuxer reads*, not the bytes of the file. That distinction is
//! the whole point: ogma writes artwork and genres back into files itself (see
//! [`crate::autofill`]), and any outside tag editor does the same. Tag blocks live outside the
//! audio packets, so rewriting them leaves this hash alone — while a hash of the file as a whole
//! would be invalidated by ogma's own background fill, which is the common case rather than a rare
//! one.
//!
//! Only a prefix is read so that fingerprinting a track costs a small read rather than a pass over
//! a whole file: a playlist of several hundred tracks is fingerprinted while it is being added to.

use std::path::Path;

use symphonia::core::formats::TrackType;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use twox_hash::XxHash3_128;

/// How much encoded audio goes into a fingerprint.
///
/// Enough that two different recordings do not share it — a megabyte is seconds of audio in any
/// format ogma plays — and little enough to read without being noticed.
pub const PAYLOAD_BYTES: usize = 1 << 20;

/// The hash of `path`'s first [`PAYLOAD_BYTES`] of audio, or `None` when it has none to read.
///
/// A file that cannot be opened, is not audio, or holds no packets has no fingerprint, which the
/// callers treat as "cannot be followed" rather than as an error: a track with no fingerprint
/// behaves exactly as tracks did before fingerprints existed.
pub fn of(path: &Path) -> Option<u128> {
    // A URL has no file to read, and reading one would mean fetching the audio over the network.
    if crate::ytdl::is_stream(&path.to_string_lossy()) {
        return None;
    }

    let file = std::fs::File::open(path).ok()?;
    let mut hint = Hint::new();

    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(extension);
    }

    let stream = MediaSourceStream::new(Box::new(file), Default::default());

    let mut reader = symphonia::default::get_probe()
        .probe(&hint, stream, Default::default(), Default::default())
        .ok()?;

    let track_id = reader.first_track_known_codec(TrackType::Audio)?.id;

    let mut hasher = XxHash3_128::new();
    let mut hashed = 0usize;

    while hashed < PAYLOAD_BYTES {
        let packet = match reader.next_packet() {
            Ok(Some(packet)) => packet,
            // The end of the audio, or a file too damaged to read further: what was read is still
            // a fingerprint, as long as there was some of it.
            Ok(None) | Err(_) => break,
        };

        // A container can carry more than one track; only the audio being played identifies it.
        if packet.track_id != track_id {
            continue;
        }

        let wanted = PAYLOAD_BYTES - hashed;
        let data = &packet.data[..packet.data.len().min(wanted)];

        hasher.write(data);
        hashed += data.len();
    }

    if hashed == 0 {
        return None;
    }

    // The length goes in last so that a short track is not the fingerprint of the longer one it is
    // a prefix of — an encode cut short, say, beside the whole of it.
    hasher.write(&(hashed as u64).to_le_bytes());

    Some(hasher.finish_128())
}

/// A fingerprint as it is written into a playlist file: lowercase hex, 32 digits.
pub fn to_hex(print: u128) -> String {
    format!("{print:032x}")
}

/// Read back what [`to_hex`] wrote, ignoring anything that is not a fingerprint.
pub fn from_hex(text: &str) -> Option<u128> {
    u128::from_str_radix(text.trim(), 16).ok()
}
