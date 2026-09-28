//! Playback against the real audio device.
//!
//! These tests need an output device. Where there is none — a headless machine, a sandbox without
//! sound — they report that and pass, rather than failing for something the code cannot control.
//!
//! Engines are created one at a time: two tests opening the device at once is a property of the
//! machine, not of the code under test.

mod common;

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ogma::audio::AudioEngine;

/// Serializes device access across tests in this binary.
static DEVICE: Mutex<()> = Mutex::new(());

fn device() -> MutexGuard<'static, ()> {
    DEVICE.lock().unwrap_or_else(|err| err.into_inner())
}

/// An engine at a low volume, or `None` when the machine has no output.
fn engine() -> Option<AudioEngine> {
    match AudioEngine::new(0.05) {
        Ok(engine) => Some(engine),
        Err(err) => {
            eprintln!("skipping: no audio device ({err})");
            None
        }
    }
}

/// A twelve-second tone, long enough to seek around in.
fn tone(name: &str) -> std::path::PathBuf {
    let dir = common::scratch_dir(name);
    let path = dir.join("tone.wav");

    // 12 seconds of 16-bit mono at 44.1 kHz, which also exercises resampling to a 48 kHz device.
    const RATE: u32 = 44_100;
    let frames = RATE * 12;

    let samples: Vec<u8> = (0..frames)
        .flat_map(|frame| {
            let phase = frame as f32 / RATE as f32 * 220.0 * std::f32::consts::TAU;
            ((phase.sin() * 6000.0) as i16).to_le_bytes()
        })
        .collect();

    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(&samples);

    std::fs::write(&path, wav).expect("write tone");

    path
}

#[test]
fn the_clock_follows_the_sound() {
    let _guard = device();
    let Some(mut engine) = engine() else { return };

    let path = tone("audio-clock");
    engine.load(&path, Duration::ZERO).expect("load");

    let start = Instant::now();
    std::thread::sleep(Duration::from_millis(800));
    let played = engine.position();
    let elapsed = start.elapsed();

    // The device consumes audio in real time, give or take its own buffering.
    assert!(
        played > Duration::from_millis(300),
        "playback should have advanced, got {played:?}"
    );
    assert!(
        played < elapsed + Duration::from_millis(300),
        "the clock must not run ahead of the sound: {played:?} vs {elapsed:?}"
    );
}

#[test]
fn pausing_freezes_the_clock_and_resuming_carries_on() {
    let _guard = device();
    let Some(mut engine) = engine() else { return };

    engine.load(&tone("audio-pause"), Duration::ZERO).expect("load");
    std::thread::sleep(Duration::from_millis(400));

    engine.pause();
    // Let any audio already handed to the device drain before sampling the clock.
    std::thread::sleep(Duration::from_millis(150));
    let paused_at = engine.position();

    std::thread::sleep(Duration::from_millis(500));
    let still = engine.position();

    assert_eq!(paused_at, still, "a paused player does not advance");

    engine.resume();
    std::thread::sleep(Duration::from_millis(400));

    assert!(engine.position() > still, "resuming carries on from where it paused");
}

#[test]
fn seeking_moves_playback() {
    let _guard = device();
    let Some(mut engine) = engine() else { return };

    engine.load(&tone("audio-seek"), Duration::ZERO).expect("load");
    std::thread::sleep(Duration::from_millis(300));

    engine.seek(Duration::from_secs(6)).expect("seek");
    let landed = engine.position();
    assert!(
        landed >= Duration::from_secs(6) && landed < Duration::from_millis(6_300),
        "the clock reports the new position at once, got {landed:?}"
    );

    std::thread::sleep(Duration::from_millis(400));
    assert!(engine.position() > landed, "and playback continues from there");

    // Seeking back works the same way.
    engine.seek(Duration::from_secs(1)).expect("seek back");
    assert!(engine.position() < Duration::from_millis(1_300));
}

#[test]
fn a_song_that_runs_out_reports_itself_finished() {
    let _guard = device();
    let Some(mut engine) = engine() else { return };

    // Half a second long, so this does not sit waiting.
    let dir = common::scratch_dir("audio-finish");
    engine.load(&common::write_tagged_wav(&dir), Duration::ZERO).expect("load");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !engine.finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(engine.finished(), "the end of the file should be reported");
    // And the clock stopped at about the length of the file.
    assert!(
        engine.position() >= Duration::from_millis(400),
        "got {:?}",
        engine.position()
    );
}

#[test]
fn a_file_that_cannot_be_decoded_is_reported_rather_than_hanging() {
    let _guard = device();
    let Some(mut engine) = engine() else { return };

    let dir = common::scratch_dir("audio-garbage");
    let path = dir.join("garbage.wav");
    std::fs::write(&path, b"this is not audio").expect("write garbage");

    engine.load(&path, Duration::ZERO).expect("load is accepted");

    let deadline = Instant::now() + Duration::from_secs(3);
    while !engine.finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }

    assert!(engine.finished(), "an unplayable file must not stall the queue");
}

#[test]
fn loading_another_song_replaces_the_one_playing() {
    let _guard = device();
    let Some(mut engine) = engine() else { return };

    let first = tone("audio-swap-first");
    let second = tone("audio-swap-second");

    engine.load(&first, Duration::ZERO).expect("load first");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(engine.loaded(), Some(first.as_path()));

    engine.load(&second, Duration::from_secs(4)).expect("load second");
    assert_eq!(engine.loaded(), Some(second.as_path()));

    // The new song starts where it was asked to, not where the old one had reached.
    let landed = engine.position();
    assert!(landed >= Duration::from_secs(4), "got {landed:?}");

    std::thread::sleep(Duration::from_millis(300));
    assert!(engine.position() > landed);

    engine.unload().expect("unload");
    assert_eq!(engine.loaded(), None);
}
