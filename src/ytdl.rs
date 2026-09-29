//! Searching YouTube, and playing or keeping what it finds.
//!
//! Everything here shells out to [yt-dlp](https://github.com/yt-dlp/yt-dlp), which is the part that
//! knows how YouTube works; this module only builds its arguments and reads its output. yt-dlp is
//! not bundled and not required: with it missing, the rest of the player carries on exactly as
//! before and the search screen says what to install.
//!
//! Two things can be done with a result:
//!
//! * **stream** it, which spawns yt-dlp writing the audio to a pipe that [`crate::audio`] decodes.
//!   Nothing is written to disk, and nothing is left behind when the song ends.
//! * **download** it, which writes an audio file into the download folder, tags and cover art
//!   included, where it becomes an ordinary part of the library.
//!
//! Nothing here runs unless the user asks for it: no search is made, and no process is started,
//! until a query is typed or a track is chosen.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::song::{Song, StreamInfo};

/// The program that does the work. Overridable for anyone keeping it somewhere unusual.
const BINARY_ENV: &str = "OGMA_YTDLP";

/// What to run when the environment names nothing else.
const DEFAULT_BINARY: &str = "yt-dlp";

/// How many results a search asks for when the caller has no opinion.
pub const DEFAULT_RESULTS: usize = 20;

/// Most results a search will ask for, so a mistyped count cannot ask YouTube for thousands.
pub const MAX_RESULTS: usize = 50;

/// Formats preferred when streaming, best first.
///
/// WebM comes first because it is what the decoder can read straight from a pipe: its container
/// puts what a reader needs at the front, where an MP4's index may sit at the very end of a file
/// that is still arriving.
const STREAM_FORMATS: &str = "bestaudio[ext=webm]/bestaudio[acodec=opus]/bestaudio/best";

/// Marks the progress lines yt-dlp prints, so they are told apart from the path it prints at the end.
const PROGRESS_MARK: &str = "[ogma]";

/// How the audio is kept when a track is downloaded.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// Whatever YouTube served, unconverted. The fastest, and the only one that loses nothing that
    /// was there to begin with — YouTube's own audio is already lossy, so re-encoding it only ever
    /// takes more away.
    #[default]
    Original,
    Opus,
    Mp3,
    M4a,
    Flac,
}

impl Format {
    /// Every choice, in the order the config screen cycles through them.
    pub const ALL: [Format; 5] = [Self::Original, Self::Opus, Self::Mp3, Self::M4a, Self::Flac];

    /// A name for the interface.
    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "as served",
            Self::Opus => "opus",
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
            Self::Flac => "flac",
        }
    }

    /// What this choice means, spelled out.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Original => "keep YouTube's own audio, no re-encoding",
            Self::Opus => "convert to opus, small and lossy",
            Self::Mp3 => "convert to mp3, for players that want it",
            Self::M4a => "convert to m4a/AAC",
            Self::Flac => "convert to flac, lossless container over lossy audio",
        }
    }

    /// The next choice in the cycle.
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|choice| *choice == self).unwrap_or(0);

        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    /// What `--audio-format` this asks for, or `None` when nothing is to be converted.
    fn audio_format(self) -> Option<&'static str> {
        match self {
            Self::Original => None,
            Self::Opus => Some("opus"),
            Self::Mp3 => Some("mp3"),
            Self::M4a => Some("m4a"),
            Self::Flac => Some("flac"),
        }
    }
}

/// One search result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Track {
    /// YouTube's id for the video, which is what makes the result unique.
    pub id: String,
    /// The page the track lives on, which is what yt-dlp is handed later.
    pub url: String,
    pub title: String,
    /// Who uploaded it, which for music is usually the artist or their label's channel.
    pub uploader: Option<String>,
    pub duration: Option<Duration>,
}

impl Track {
    /// `m:ss`, or `--:--` when the length is not known.
    pub fn display_duration(&self) -> String {
        match self.duration {
            Some(duration) => {
                let total = duration.as_secs();

                format!("{}:{:02}", total / 60, total % 60)
            }
            None => "--:--".to_string(),
        }
    }

    /// The track as something the player can be handed, playing from the network rather than a file.
    pub fn song(&self) -> Song {
        Song::stream(
            &self.url,
            StreamInfo {
                title: Some(self.title.clone()),
                artist: self.uploader.clone(),
                duration: self.duration,
            },
        )
    }
}

/// What a download is doing, as it does it.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Work has begun on a track: the `index`th of `total`.
    Started { title: String, index: usize, total: usize },
    /// How far the current track has got, 0.0 to 100.0.
    Progress { percent: f32 },
    /// A track is on disk, at `path`.
    Finished { title: String, path: PathBuf },
    /// A track could not be fetched, and the rest carry on.
    Failed { title: String, error: String },
    /// Every track has been tried.
    Done,
}

/// The program to run, which the environment may name.
pub fn binary() -> String {
    std::env::var(BINARY_ENV).unwrap_or_else(|_| DEFAULT_BINARY.to_string())
}

/// Whether yt-dlp can be run at all.
///
/// Asked before a search rather than letting the failure surface as an empty result: "yt-dlp is not
/// installed" is a different thing to tell someone than "nothing matched".
pub fn available() -> bool {
    Command::new(binary())
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// What to tell the user when yt-dlp is missing, or `None` when it is there.
pub fn missing() -> Option<String> {
    if available() {
        return None;
    }

    Some(format!("{} not found — install it to search YouTube", binary()))
}

/// Ask YouTube for `query`, returning at most `limit` tracks.
///
/// Blocks until yt-dlp answers, which takes a second or two; [`search_in_background`] is what the
/// interface uses so that the screen keeps drawing meanwhile.
pub fn search(query: &str, limit: usize) -> Result<Vec<Track>, String> {
    let query = query.trim();

    if query.is_empty() {
        return Ok(Vec::new());
    }

    let limit = limit.clamp(1, MAX_RESULTS);

    // `--flat-playlist` keeps this to one request for the search page: without it yt-dlp resolves
    // every hit in turn, which is a dozen requests and several seconds for a list nobody may pick
    // from. What comes back is enough to list: title, uploader, length.
    let output = Command::new(binary())
        .args([
            "--flat-playlist",
            "--dump-json",
            "--no-warnings",
            "--ignore-config",
            "--no-playlist",
        ])
        .arg(format!("ytsearch{limit}:{query}"))
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("cannot run {}: {err}", binary()))?;

    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        let first = message.lines().next().unwrap_or("search failed").trim();

        return Err(first.to_string());
    }

    Ok(parse_search(&String::from_utf8_lossy(&output.stdout)))
}

/// Search on a thread of its own, so the interface can keep drawing while YouTube is asked.
///
/// The answer arrives once; dropping the receiver abandons it.
pub fn search_in_background(query: String, limit: usize) -> Receiver<Result<Vec<Track>, String>> {
    let (sender, receiver) = mpsc::channel();

    let spawned = std::thread::Builder::new()
        .name("ogma-ytdl-search".to_string())
        .spawn(move || {
            // A closed channel means the screen moved on, which is not an error.
            let _ = sender.send(search(&query, limit));
        });

    if spawned.is_err() {
        // Nothing will ever arrive on the receiver, and the caller reports the silence as a failure
        // rather than waiting forever.
        return receiver;
    }

    receiver
}

/// Read the JSON lines `yt-dlp --dump-json` writes, one object per result.
///
/// Anything unparseable is skipped rather than failing the search: one odd result should not lose
/// the other nineteen. Results with no id are dropped, since there would be nothing to play.
pub fn parse_search(output: &str) -> Vec<Track> {
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .filter_map(|value| track_from_json(&value))
        .collect()
}

/// One search result, from the object yt-dlp printed for it.
fn track_from_json(value: &Value) -> Option<Track> {
    let id = value.get("id")?.as_str()?.trim().to_string();

    if id.is_empty() {
        return None;
    }

    let text = |key: &str| -> Option<String> {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(clean)
            .filter(|found| !found.is_empty())
    };

    // The search page gives a watch URL; building one from the id is the fallback for a result that
    // does not carry it.
    let url = text("url")
        .filter(|url| url.starts_with("http"))
        .or_else(|| text("webpage_url"))
        .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={id}"));

    // Live streams report no duration, and so do some results; they list as `--:--` rather than
    // being dropped, since they still play.
    let duration = value
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|seconds| *seconds > 0.0)
        .map(Duration::from_secs_f64);

    Some(Track {
        title: text("title").unwrap_or_else(|| id.clone()),
        // The uploader is the nearest thing a YouTube result has to an artist. `channel` is what the
        // search page fills in more often; `uploader` is what a resolved video carries.
        uploader: text("uploader").or_else(|| text("channel")),
        duration,
        id,
        url,
    })
}

/// Tidy a title or a channel name into something that can be drawn on one line.
///
/// A newline or a tab in a title would break the row it is drawn in, and the control characters
/// some titles carry are not worth rendering at all; each becomes a space, and the runs of spaces
/// that leaves collapse into one.
fn clean(text: &str) -> String {
    let spaced: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();

    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Audio arriving from yt-dlp's standard output, for the decoder to read.
///
/// Dropping this kills the process: a stream nobody is listening to must not keep downloading.
#[derive(Debug)]
pub struct Stream {
    child: Child,
    stdout: ChildStdout,
}

impl Stream {
    /// The track's title as yt-dlp knows it, which the caller may already have from the search.
    pub fn new(url: &str) -> Result<Self, String> {
        let mut child = Command::new(binary())
            .args(["--no-playlist", "--quiet", "--no-warnings", "--ignore-config", "-f"])
            .arg(STREAM_FORMATS)
            // `-o -` is what makes it write the audio to the pipe rather than to a file.
            .args(["-o", "-"])
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("cannot run {}: {err}", binary()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "yt-dlp gave no output to read".to_string())?;

        Ok(Stream { child, stdout })
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stdout.read(buf)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // The process has no reason to carry on once nothing is reading it, and would go on
        // downloading the whole track if left alone.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whether `uri` names something to be streamed rather than a file to be opened.
pub fn is_stream(uri: &str) -> bool {
    uri.starts_with("http://") || uri.starts_with("https://")
}

/// Fetch one track into `folder`, returning where it landed.
///
/// Blocks until yt-dlp is finished, which for a song is a few seconds; [`download_in_background`]
/// is what the interface uses.
pub fn download(track: &Track, folder: &Path, format: Format) -> Result<PathBuf, String> {
    fetch(track, folder, format, None)
}

/// Fetch every track in turn on a thread of its own, reporting progress as it goes.
///
/// One at a time rather than all at once: several downloads at a time is heavier on the network and
/// on YouTube, and the interface has one progress line to show anyway.
pub fn download_in_background(
    tracks: Vec<Track>,
    folder: PathBuf,
    format: Format,
) -> Receiver<Event> {
    let (sender, receiver) = mpsc::channel();

    let spawned = std::thread::Builder::new()
        .name("ogma-ytdl-download".to_string())
        .spawn(move || {
            let total = tracks.len();

            for (index, track) in tracks.iter().enumerate() {
                let started = Event::Started {
                    title: track.title.clone(),
                    index: index + 1,
                    total,
                };

                // A closed channel means the screen has gone; there is no one left to download for.
                if sender.send(started).is_err() {
                    return;
                }

                let event = match fetch(track, &folder, format, Some(&sender)) {
                    Ok(path) => Event::Finished { title: track.title.clone(), path },
                    Err(error) => Event::Failed { title: track.title.clone(), error },
                };

                if sender.send(event).is_err() {
                    return;
                }
            }

            let _ = sender.send(Event::Done);
        });

    if spawned.is_err() {
        // Nothing will arrive; the caller sees a receiver that ends at once.
        return receiver;
    }

    receiver
}

/// Fetch one track, sending progress to `sender` when there is one to send it to.
///
/// Cover art is asked for first and given up on if that is what failed: embedding a picture needs
/// more than yt-dlp itself — ffmpeg, and Mutagen for the Ogg formats — and a missing helper should
/// cost the track its cover, not the download.
fn fetch(
    track: &Track,
    folder: &Path,
    format: Format,
    sender: Option<&mpsc::Sender<Event>>,
) -> Result<PathBuf, String> {
    match run(track, folder, format, true, sender) {
        Err(err) if is_artwork_failure(&err) => run(track, folder, format, false, sender),
        other => other,
    }
}

/// Whether a failure was the cover art rather than the download.
fn is_artwork_failure(error: &str) -> bool {
    let error = error.to_lowercase();

    error.contains("thumbnail") || error.contains("mutagen") || error.contains("atomicparsley")
}

/// Run yt-dlp once, reading its output as it goes.
fn run(
    track: &Track,
    folder: &Path,
    format: Format,
    thumbnail: bool,
    sender: Option<&mpsc::Sender<Event>>,
) -> Result<PathBuf, String> {
    let mut child = download_command(track, folder, format, thumbnail)
        .spawn()
        .map_err(|err| format!("cannot run {}: {err}", binary()))?;

    let mut path = None;

    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            match read_line(&line) {
                Line::Progress(percent) => {
                    // Nobody to tell is not a reason to stop; nobody left listening is.
                    if let Some(sender) = sender
                        && sender.send(Event::Progress { percent }).is_err()
                    {
                        let _ = child.kill();
                        let _ = child.wait();

                        return Err("abandoned".to_string());
                    }
                }
                Line::Path(found) => path = Some(found),
                Line::Other => {}
            }
        }
    }

    finish(child, path)
}

/// Wait for yt-dlp and turn what it left behind into an answer.
fn finish(mut child: Child, path: Option<PathBuf>) -> Result<PathBuf, String> {
    let status = child
        .wait()
        .map_err(|err| format!("{} did not finish: {err}", binary()))?;

    let mut message = String::new();

    if let Some(stderr) = child.stderr.as_mut() {
        let _ = stderr.read_to_string(&mut message);
    }

    if !status.success() {
        let first = message
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("download failed")
            .trim();

        return Err(first.to_string());
    }

    // yt-dlp prints the final path only once the file is in place, so its absence means the file
    // never arrived however the exit code read.
    path.ok_or_else(|| "download produced no file".to_string())
}

/// The command a download runs, built but not started, so every attempt asks for the same thing.
fn download_command(track: &Track, folder: &Path, format: Format, thumbnail: bool) -> Command {
    let mut command = Command::new(binary());

    command
        .args(["--no-playlist", "--quiet", "--no-warnings", "--ignore-config"])
        // Audio only: the video is several times the size and nothing here can show it.
        .args(["-f", "bestaudio/best", "-x"])
        // Tags go into the file, so a downloaded track lists like any other rather than as an
        // unknown artist.
        .arg("--embed-metadata")
        // Progress on its own marked lines, and the finished path on a line of its own, so reading
        // the output is not guesswork.
        .args(["--newline", "--progress"])
        .args(["--progress-template", &format!("download:{PROGRESS_MARK} %(progress._percent_str)s")])
        .args(["--print", "after_move:filepath", "--no-simulate"])
        .arg("--paths")
        .arg(folder)
        .args(["-o", "%(artist,uploader,channel)s - %(track,title)s.%(ext)s"]);

    if thumbnail {
        command.arg("--embed-thumbnail");
    }

    if let Some(audio_format) = format.audio_format() {
        command.args(["--audio-format", audio_format, "--audio-quality", "0"]);
    }

    command
        .arg(&track.url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    command
}

/// What one line of yt-dlp's output says.
#[derive(Clone, Debug, PartialEq)]
enum Line {
    /// A percentage, from the progress template.
    Progress(f32),
    /// Where the finished file went.
    Path(PathBuf),
    /// Something else, which is nothing to act on.
    Other,
}

/// Read one line of output. Public to the module only; the tests drive it through [`parse_search`]
/// and through this.
fn read_line(line: &str) -> Line {
    let line = line.trim();

    if line.is_empty() {
        return Line::Other;
    }

    if let Some(rest) = line.strip_prefix(PROGRESS_MARK) {
        return match rest.trim().trim_end_matches('%').trim().parse::<f32>() {
            Ok(percent) => Line::Progress(percent.clamp(0.0, 100.0)),
            Err(_) => Line::Other,
        };
    }

    // Anything else on stdout is the path yt-dlp was asked to print, which is absolute.
    if line.starts_with('/') {
        return Line::Path(PathBuf::from(line));
    }

    Line::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_progress_line() {
        assert_eq!(read_line("[ogma]  12.3%"), Line::Progress(12.3));
        assert_eq!(read_line("[ogma] 100.0%"), Line::Progress(100.0));
    }

    #[test]
    fn reads_the_finished_path() {
        assert_eq!(read_line("/music/Artist - Song.opus"), Line::Path(PathBuf::from("/music/Artist - Song.opus")));
    }

    #[test]
    fn ignores_anything_else() {
        assert_eq!(read_line(""), Line::Other);
        assert_eq!(read_line("[youtube] Extracting URL"), Line::Other);
    }
}
