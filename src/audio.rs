//! Turning songs into sound.
//!
//! Three parties share the work:
//!
//! * the **decode thread** opens the file, decodes packets, maps them onto the device's channel
//!   count and sample rate, and pushes interleaved `f32` frames into a ring buffer;
//! * the **output callback**, owned by the audio device, pops those frames, applies the volume and
//!   hands them to the hardware — it allocates nothing and takes no locks, as a realtime callback
//!   must not;
//! * the **UI thread** sends commands and reads the shared counters to know where playback is.
//!
//! Position is measured from what the callback has consumed rather than what has been decoded, so
//! the clock follows the sound rather than running ahead of it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use symphonia::core::audio::Channels;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::units::Time;

use crate::{meta, volume, ytdl};

/// Roughly how much audio the ring buffer holds. Enough to ride out a slow disk or a busy CPU,
/// short enough that a seek does not have much to throw away.
const BUFFER_SECONDS: f32 = 0.5;

/// Frames per resampler chunk.
const RESAMPLE_CHUNK: usize = 1024;

/// How long the decode thread waits for the callback to acknowledge a flush before giving up.
const FLUSH_TIMEOUT: Duration = Duration::from_millis(200);

/// Why the engine could not be started or could not play something.
#[derive(Debug)]
pub enum AudioError {
    /// The machine reported no audio output.
    NoOutputDevice,
    /// The device's configuration could not be read or used.
    Device(String),
    /// The sample format the device wants is not one this build writes.
    UnsupportedSampleFormat(SampleFormat),
    /// The engine's own thread has gone away.
    EngineGone,
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::NoOutputDevice => write!(f, "no audio output device"),
            AudioError::Device(err) => write!(f, "audio device: {err}"),
            AudioError::UnsupportedSampleFormat(format) => {
                write!(f, "unsupported sample format: {format}")
            }
            AudioError::EngineGone => write!(f, "the audio engine stopped"),
        }
    }
}

impl std::error::Error for AudioError {}

/// What the UI thread asks the decode thread to do.
#[derive(Debug)]
enum Command {
    /// Open this file and start feeding it to the device.
    Load { path: PathBuf, start_at: Duration },
    /// Jump within the open file.
    Seek(Duration),
    /// Close the open file and fall silent.
    Unload,
    /// Finish the thread.
    Quit,
}

/// State shared between the three parties. Every field is atomic; nothing here ever blocks.
#[derive(Debug)]
struct Shared {
    /// Frames the callback has handed to the device since the last load or seek.
    frames_played: AtomicU64,
    /// Where the current song was started or seeked to, in milliseconds.
    base_ms: AtomicU64,
    /// Fader position, 0.0 to 1.0, as the bits of an `f32`. The gain applied is the curved form of
    /// this, not the value itself.
    volume: AtomicU32,
    /// Whether the callback should consume the ring, or emit silence.
    playing: AtomicBool,
    /// Set by the decode thread and cleared by the callback, to drop stale audio on a seek.
    flush: AtomicBool,
    /// Set when the open file has been decoded and played to its end.
    finished: AtomicBool,
    /// Sample rate the device runs at.
    device_rate: u32,
    /// Channels the device expects.
    device_channels: u16,
}

impl Shared {
    /// The gain to multiply samples by, from the fader position through the loudness curve.
    fn gain(&self) -> f32 {
        volume::gain(f32::from_bits(self.volume.load(Ordering::Relaxed)))
    }

    /// How far into the song the device has actually got.
    fn position(&self) -> Duration {
        let base = Duration::from_millis(self.base_ms.load(Ordering::Relaxed));
        let played = self.frames_played.load(Ordering::Relaxed);

        base + Duration::from_secs_f64(played as f64 / f64::from(self.device_rate))
    }

    /// Start the clock again from `at`.
    fn rebase(&self, at: Duration) {
        self.base_ms.store(at.as_millis() as u64, Ordering::Relaxed);
        self.frames_played.store(0, Ordering::Relaxed);
    }
}

/// An open audio device and the thread that feeds it.
///
/// Dropping the engine stops the stream and joins the decode thread.
pub struct AudioEngine {
    commands: Sender<Command>,
    shared: Arc<Shared>,
    /// Kept alive for as long as the engine is: dropping it closes the device.
    _stream: cpal::Stream,
    decoder: Option<std::thread::JoinHandle<()>>,
    /// What was last handed to [`AudioEngine::load`], so callers can tell whether a song is loaded.
    loaded: Option<PathBuf>,
}

impl std::fmt::Debug for AudioEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioEngine")
            .field("loaded", &self.loaded)
            .field("device_rate", &self.shared.device_rate)
            .field("device_channels", &self.shared.device_channels)
            .finish()
    }
}

impl AudioEngine {
    /// Open the default output device and start the decode thread.
    ///
    /// `volume` is a fader position, 0.0 to 1.0.
    pub fn new(volume: f32) -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(AudioError::NoOutputDevice)?;

        let supported = device
            .default_output_config()
            .map_err(|err| AudioError::Device(err.to_string()))?;

        let format = supported.sample_format();
        let config: StreamConfig = supported.into();

        let shared = Arc::new(Shared {
            frames_played: AtomicU64::new(0),
            base_ms: AtomicU64::new(0),
            volume: AtomicU32::new(volume.clamp(0.0, 1.0).to_bits()),
            playing: AtomicBool::new(false),
            flush: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            device_rate: config.sample_rate,
            device_channels: config.channels,
        });

        // Half a second of audio, rounded to whole frames.
        let capacity = (config.sample_rate as f32 * BUFFER_SECONDS) as usize
            * usize::from(config.channels);
        let (producer, consumer) = rtrb::RingBuffer::<f32>::new(capacity);

        let stream = build_stream(&device, &config, format, consumer, Arc::clone(&shared))?;
        stream.play().map_err(|err| AudioError::Device(err.to_string()))?;

        let (commands, orders) = mpsc::channel();
        let decoder = {
            let shared = Arc::clone(&shared);

            std::thread::Builder::new()
                .name("ogma-decode".to_string())
                .spawn(move || decode_loop(orders, producer, shared))
                .map_err(|err| AudioError::Device(err.to_string()))?
        };

        Ok(AudioEngine {
            commands,
            shared,
            _stream: stream,
            decoder: Some(decoder),
            loaded: None,
        })
    }

    /// The file currently open for playback.
    pub fn loaded(&self) -> Option<&Path> {
        self.loaded.as_deref()
    }

    /// Open `path` and play it from `start_at`.
    pub fn load(&mut self, path: &Path, start_at: Duration) -> Result<(), AudioError> {
        self.shared.finished.store(false, Ordering::Relaxed);
        self.shared.rebase(start_at);
        self.send(Command::Load { path: path.to_path_buf(), start_at })?;
        self.loaded = Some(path.to_path_buf());
        self.shared.playing.store(true, Ordering::Relaxed);

        Ok(())
    }

    /// Close whatever is open and fall silent.
    pub fn unload(&mut self) -> Result<(), AudioError> {
        self.shared.playing.store(false, Ordering::Relaxed);
        self.shared.finished.store(false, Ordering::Relaxed);
        self.shared.rebase(Duration::ZERO);
        self.loaded = None;

        self.send(Command::Unload)
    }

    /// Let the sound out.
    pub fn resume(&self) {
        self.shared.playing.store(true, Ordering::Relaxed);
    }

    /// Hold the sound without giving up the buffered audio.
    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
    }

    /// Whether what is open arrived over the network, and so cannot be moved within.
    pub fn is_streaming(&self) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|path| ytdl::is_stream(&path.to_string_lossy()))
    }

    /// Jump to `position` within the open file.
    ///
    /// A stream is read straight through as it arrives, with nothing to jump back to and no index
    /// to jump forward by, so this does nothing to one: the clock stays where the sound is rather
    /// than being moved to a position the audio never reached.
    pub fn seek(&self, position: Duration) -> Result<(), AudioError> {
        if self.is_streaming() {
            return Ok(());
        }

        self.shared.finished.store(false, Ordering::Relaxed);
        self.shared.rebase(position);

        self.send(Command::Seek(position))
    }

    /// Set the fader position, 0.0 to 1.0. The gain applied follows the loudness curve in
    /// [`crate::volume`].
    pub fn set_volume(&self, position: f32) {
        self.shared.volume.store(position.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// The gain currently applied to samples, for anything that needs the amplitude rather than the
    /// fader position.
    pub fn gain(&self) -> f32 {
        self.shared.gain()
    }

    /// How far into the open file the device has played.
    pub fn position(&self) -> Duration {
        self.shared.position()
    }

    /// Whether the open file has played to its end.
    pub fn finished(&self) -> bool {
        self.shared.finished.load(Ordering::Relaxed)
    }

    /// The rate the device runs at, which everything is resampled to.
    pub fn device_rate(&self) -> u32 {
        self.shared.device_rate
    }

    /// The channel count the device expects.
    pub fn device_channels(&self) -> u16 {
        self.shared.device_channels
    }

    fn send(&self, command: Command) -> Result<(), AudioError> {
        self.commands.send(command).map_err(|_| AudioError::EngineGone)
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.shared.playing.store(false, Ordering::Relaxed);
        let _ = self.commands.send(Command::Quit);

        if let Some(decoder) = self.decoder.take() {
            let _ = decoder.join();
        }
    }
}

/// Build the output stream for whichever sample format the device asked for.
fn build_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    consumer: rtrb::Consumer<f32>,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, AudioError> {
    // The callback is written once over `f32` and converted per sample for the other formats.
    macro_rules! stream {
        ($sample:ty) => {{
            let mut consumer = consumer;
            let shared = shared;

            device
                .build_output_stream(
                    config.clone(),
                    move |output: &mut [$sample], _| {
                        fill(output, &mut consumer, &shared);
                    },
                    |err| {
                        // Nothing useful can be done from the callback; the UI notices the silence.
                        let _ = err;
                    },
                    None,
                )
                .map_err(|err| AudioError::Device(err.to_string()))
        }};
    }

    match format {
        SampleFormat::F32 => stream!(f32),
        SampleFormat::I16 => stream!(i16),
        SampleFormat::U16 => stream!(u16),
        SampleFormat::F64 => stream!(f64),
        SampleFormat::I32 => stream!(i32),
        other => Err(AudioError::UnsupportedSampleFormat(other)),
    }
}

/// The realtime callback: pop frames, scale them, write them out.
fn fill<S>(output: &mut [S], consumer: &mut rtrb::Consumer<f32>, shared: &Shared)
where
    S: cpal::Sample + cpal::FromSample<f32>,
{
    let silence = S::from_sample(0.0f32);

    // A seek asked for everything buffered to be dropped.
    if shared.flush.swap(false, Ordering::Acquire) {
        while consumer.pop().is_ok() {}

        output.fill(silence);
        return;
    }

    if !shared.playing.load(Ordering::Relaxed) {
        output.fill(silence);
        return;
    }

    // Converted once per callback rather than per sample: a `powf` per sample would be waste.
    let gain = shared.gain();
    let mut written = 0;

    for slot in output.iter_mut() {
        match consumer.pop() {
            Ok(sample) => {
                *slot = S::from_sample(sample * gain);
                written += 1;
            }
            // Underrun, or nothing left to play: silence is better than a click.
            Err(_) => *slot = silence,
        }
    }

    let frames = written / usize::from(shared.device_channels).max(1);
    shared.frames_played.fetch_add(frames as u64, Ordering::Relaxed);
}

/// The decode thread: obey commands, and keep the ring buffer fed while something is open.
fn decode_loop(orders: Receiver<Command>, mut producer: rtrb::Producer<f32>, shared: Arc<Shared>) {
    let mut track: Option<OpenTrack> = None;

    loop {
        // Commands first: they decide what should be decoded at all.
        match orders.try_recv() {
            Ok(Command::Load { path, start_at }) => {
                flush_ring(&shared);

                track = match OpenTrack::open(&path, &shared) {
                    Ok(mut open) => {
                        if !start_at.is_zero() {
                            let _ = open.seek(start_at);
                        }

                        Some(open)
                    }
                    Err(_) => {
                        // An unplayable file reads as a finished one, so the queue moves on.
                        shared.finished.store(true, Ordering::Relaxed);
                        None
                    }
                };

                continue;
            }
            Ok(Command::Seek(position)) => {
                if let Some(open) = &mut track {
                    flush_ring(&shared);
                    let _ = open.seek(position);
                }

                continue;
            }
            Ok(Command::Unload) => {
                flush_ring(&shared);
                track = None;
                continue;
            }
            Ok(Command::Quit) | Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {}
        }

        let Some(open) = &mut track else {
            // Nothing to do; wake often enough to stay responsive to commands.
            std::thread::sleep(Duration::from_millis(10));
            continue;
        };

        match open.pump(&mut producer) {
            PumpResult::Pushed => {}
            PumpResult::Full => std::thread::sleep(Duration::from_millis(5)),
            PumpResult::Done => {
                // The file is decoded; the song is over once the device has played the remainder.
                if producer.slots() == producer.buffer().capacity() {
                    shared.finished.store(true, Ordering::Relaxed);
                    track = None;
                } else {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }
}

/// Ask the callback to drop what it holds, and wait for it to say it has.
fn flush_ring(shared: &Shared) {
    shared.flush.store(true, Ordering::Release);

    // The callback clears the flag once it has drained. If the device is not running, give up rather
    // than hang: whatever is left is at most a fraction of a second of stale audio.
    let deadline = std::time::Instant::now() + FLUSH_TIMEOUT;
    while shared.flush.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }

    shared.flush.store(false, Ordering::Release);
}

/// How far a single pump of the decoder got.
enum PumpResult {
    /// Samples were pushed.
    Pushed,
    /// The ring buffer has no room; try again shortly.
    Full,
    /// The file has been decoded to its end.
    Done,
}

/// One open file, with everything needed to turn it into device-ready frames.
struct OpenTrack {
    reader: Box<dyn FormatReader + 'static>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    /// Channel count of the file.
    source_channels: u16,
    device_channels: u16,
    /// Present only when the file's rate differs from the device's.
    resampler: Option<Fft<f32>>,
    /// Frames awaiting resampling, interleaved at the device's channel count.
    pending: Vec<f32>,
    /// Device-ready frames waiting for room in the ring buffer.
    ready: std::collections::VecDeque<f32>,
    /// Scratch space, reused so the decode loop does not allocate per packet.
    decoded: Vec<f32>,
    mapped: Vec<f32>,
    resampled: Vec<f32>,
}

/// Audio arriving from yt-dlp rather than from a file.
///
/// Reports itself as unseekable and of unknown length, which is what tells the demuxer to read
/// straight through rather than looking for an index it cannot reach. Seeking within a stream
/// therefore fails, and the daemon simply keeps playing where it is.
struct Piped {
    stream: ytdl::Stream,
}

impl std::io::Read for Piped {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stream.read(buf)
    }
}

impl std::io::Seek for Piped {
    fn seek(&mut self, _to: std::io::SeekFrom) -> std::io::Result<u64> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "a stream cannot be seeked",
        ))
    }
}

impl MediaSource for Piped {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

impl OpenTrack {
    fn open(path: &Path, shared: &Shared) -> Result<Self, SymphoniaError> {
        let uri = path.to_string_lossy();

        // A URL is played through yt-dlp, which writes the audio to a pipe; anything else is a file.
        let (source, mut hint): (Box<dyn MediaSource>, Hint) = if ytdl::is_stream(&uri) {
            let stream = ytdl::Stream::new(&uri)
                .map_err(|err| SymphoniaError::IoError(std::io::Error::other(err)))?;

            let mut hint = Hint::new();
            // What the stream format asks yt-dlp for, and what it nearly always serves. A wrong
            // hint only costs the probe a guess; the bytes themselves decide.
            hint.with_extension("webm");

            (Box::new(Piped { stream }), hint)
        } else {
            (Box::new(std::fs::File::open(path)?), Hint::new())
        };

        let mss = MediaSourceStream::new(source, Default::default());

        if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
            hint.with_extension(extension);
        }

        let reader = symphonia::default::get_probe().probe(
            &hint,
            mss,
            Default::default(),
            Default::default(),
        )?;

        let track = reader
            .first_track_known_codec(TrackType::Audio)
            .ok_or(SymphoniaError::Unsupported("no playable audio track"))?;

        let track_id = track.id;
        let Some(CodecParameters::Audio(params)) = &track.codec_params else {
            return Err(SymphoniaError::Unsupported("track has no audio parameters"));
        };

        let source_rate = params.sample_rate.unwrap_or(shared.device_rate);
        let source_channels = params
            .channels
            .as_ref()
            .map(|channels| Channels::count(channels) as u16)
            .unwrap_or(2);

        // The registry includes the Opus adapter, so Opus plays like anything else.
        let decoder = meta::codec_registry().make_audio_decoder(params, &Default::default())?;

        let resampler = (source_rate != shared.device_rate)
            .then(|| {
                Fft::<f32>::new(
                    source_rate as usize,
                    shared.device_rate as usize,
                    RESAMPLE_CHUNK,
                    usize::from(shared.device_channels),
                    FixedSync::Input,
                )
            })
            .transpose()
            .map_err(|_| SymphoniaError::Unsupported("cannot resample this sample rate"))?;

        Ok(OpenTrack {
            reader,
            decoder,
            track_id,
            source_channels,
            device_channels: shared.device_channels,
            resampler,
            pending: Vec::new(),
            ready: std::collections::VecDeque::new(),
            decoded: Vec::new(),
            mapped: Vec::new(),
            resampled: Vec::new(),
        })
    }

    /// Jump to `position`, discarding anything already decoded.
    fn seek(&mut self, position: Duration) -> Result<(), SymphoniaError> {
        let time = Time::try_from_secs_f64(position.as_secs_f64())
            .ok_or(SymphoniaError::Unsupported("seek position out of range"))?;

        self.reader.seek(
            SeekMode::Accurate,
            SeekTo::Time { time, track_id: Some(self.track_id) },
        )?;

        // The decoder's state belongs to the old position.
        self.decoder.reset();
        self.pending.clear();
        self.ready.clear();

        if let Some(resampler) = &mut self.resampler {
            resampler.reset();
        }

        Ok(())
    }

    /// Move one packet's worth of audio towards the device.
    fn pump(&mut self, producer: &mut rtrb::Producer<f32>) -> PumpResult {
        // Anything already converted goes out first.
        if !self.ready.is_empty() {
            return self.drain(producer);
        }

        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                // End of stream: push whatever the resampler still holds.
                Ok(None) => {
                    self.finish();
                    return if self.ready.is_empty() { PumpResult::Done } else { self.drain(producer) };
                }
                Err(SymphoniaError::IoError(err))
                    if err.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.finish();
                    return if self.ready.is_empty() { PumpResult::Done } else { self.drain(producer) };
                }
                // A damaged packet is skipped rather than ending the song.
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(_) => return PumpResult::Done,
            };

            if packet.track_id != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(buffer) => {
                    self.decoded.clear();
                    buffer.copy_to_vec_interleaved(&mut self.decoded);
                }
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(_) => return PumpResult::Done,
            }

            self.convert();

            if !self.ready.is_empty() {
                return self.drain(producer);
            }
        }
    }

    /// Map channels, then resample, leaving device-ready frames in `ready`.
    fn convert(&mut self) {
        map_channels(
            &self.decoded,
            self.source_channels,
            self.device_channels,
            &mut self.mapped,
        );

        let Some(resampler) = &mut self.resampler else {
            self.ready.extend(self.mapped.iter().copied());
            return;
        };

        self.pending.extend(self.mapped.iter().copied());

        let channels = usize::from(self.device_channels);

        // The resampler wants whole chunks of a fixed size.
        while self.pending.len() >= resampler.input_frames_next() * channels {
            let input_frames = resampler.input_frames_next();
            let output_frames = resampler.output_frames_next();

            self.resampled.clear();
            self.resampled.resize(output_frames * channels, 0.0);

            let input = &self.pending[..input_frames * channels];

            let Ok(adapter) = InterleavedSlice::new(input, channels, input_frames) else {
                self.pending.clear();
                return;
            };
            let Ok(mut out) = InterleavedSlice::new_mut(&mut self.resampled, channels, output_frames)
            else {
                self.pending.clear();
                return;
            };

            let indexing = Indexing::default();

            match resampler.process_into_buffer(&adapter, &mut out, Some(&indexing)) {
                Ok((used, produced)) => {
                    self.ready.extend(self.resampled[..produced * channels].iter().copied());
                    self.pending.drain(..used * channels);
                }
                Err(_) => {
                    // Give up on this chunk rather than looping on it.
                    self.pending.clear();
                    return;
                }
            }
        }
    }

    /// Flush the resampler's tail at the end of a file.
    fn finish(&mut self) {
        let channels = usize::from(self.device_channels);

        let Some(resampler) = &mut self.resampler else {
            self.ready.extend(self.pending.drain(..));
            return;
        };

        if self.pending.is_empty() {
            return;
        }

        let input_frames = resampler.input_frames_next();
        let partial = self.pending.len() / channels;
        let output_frames = resampler.output_frames_next();

        // The resampler always reads a full chunk, so the short tail is padded with silence and
        // `partial_len` tells it how much of that is real audio.
        self.pending.resize(input_frames * channels, 0.0);
        self.resampled.clear();
        self.resampled.resize(output_frames * channels, 0.0);

        let Ok(adapter) = InterleavedSlice::new(&self.pending, channels, input_frames) else {
            self.pending.clear();
            return;
        };
        let Ok(mut out) = InterleavedSlice::new_mut(&mut self.resampled, channels, output_frames)
        else {
            self.pending.clear();
            return;
        };

        let indexing = Indexing { partial_len: Some(partial), ..Indexing::default() };

        if let Ok((_, produced)) = resampler.process_into_buffer(&adapter, &mut out, Some(&indexing))
        {
            self.ready.extend(self.resampled[..produced * channels].iter().copied());
        }

        self.pending.clear();
    }

    /// Push as much of `ready` into the ring buffer as it will take.
    fn drain(&mut self, producer: &mut rtrb::Producer<f32>) -> PumpResult {
        let mut pushed = false;

        while let Some(sample) = self.ready.front().copied() {
            match producer.push(sample) {
                Ok(()) => {
                    self.ready.pop_front();
                    pushed = true;
                }
                Err(_) => return if pushed { PumpResult::Pushed } else { PumpResult::Full },
            }
        }

        PumpResult::Pushed
    }
}

/// Lay `source` out for a device with `dst_channels` channels.
///
/// Mono is copied to every channel, extra source channels are dropped, and missing ones are filled
/// with silence. A real downmix matrix belongs here eventually; this keeps every file audible.
fn map_channels(source: &[f32], src_channels: u16, dst_channels: u16, out: &mut Vec<f32>) {
    out.clear();

    let src = usize::from(src_channels).max(1);
    let dst = usize::from(dst_channels).max(1);

    if src == dst {
        out.extend_from_slice(source);
        return;
    }

    for frame in source.chunks(src) {
        if src == 1 {
            // Mono to anything: the same sample everywhere.
            out.extend(std::iter::repeat_n(frame[0], dst));
            continue;
        }

        for channel in 0..dst {
            out.push(frame.get(channel).copied().unwrap_or(0.0));
        }
    }
}

/// Whether a file can be opened and decoded, without playing it.
///
/// Useful for skipping past files the engine would only fail on.
pub fn is_playable(path: &Path) -> bool {
    meta::probe(path).is_ok_and(|file| {
        meta::codec_registry()
            .get_audio_decoder(match file.raw().tech().codec_id {
                Some(codec) => codec,
                None => return false,
            })
            .is_some()
    })
}

#[cfg(test)]
mod tests {
    use super::map_channels;

    #[test]
    fn matching_channel_counts_pass_straight_through() {
        let mut out = Vec::new();
        map_channels(&[0.1, 0.2, 0.3, 0.4], 2, 2, &mut out);

        assert_eq!(out, [0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn mono_is_copied_to_every_channel() {
        let mut out = Vec::new();
        map_channels(&[0.5, -0.5], 1, 2, &mut out);

        assert_eq!(out, [0.5, 0.5, -0.5, -0.5]);

        map_channels(&[0.25], 1, 4, &mut out);
        assert_eq!(out, [0.25; 4]);
    }

    #[test]
    fn extra_source_channels_are_dropped() {
        let mut out = Vec::new();
        // Two frames of 5.1, of which a stereo device takes the first two channels.
        map_channels(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0], 6, 2, &mut out);

        assert_eq!(out, [1.0, 2.0, 7.0, 8.0]);
    }

    #[test]
    fn missing_channels_are_filled_with_silence() {
        let mut out = Vec::new();
        map_channels(&[1.0, 2.0], 2, 4, &mut out);

        assert_eq!(out, [1.0, 2.0, 0.0, 0.0]);
    }
}
