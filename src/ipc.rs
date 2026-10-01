//! Talking to a running player.
//!
//! The player listens on a Unix socket; `ogma-cmd` connects, writes one command as a line of text,
//! and reads one line back. Text rather than anything cleverer, so the whole thing can be driven with
//! `nc` or `socat` when something needs looking at:
//!
//! ```text
//! $ printf 'volume -10\n' | nc -U "${XDG_RUNTIME_DIR}/ogma.sock"
//! ok: volume 45% (-14.0 dB)
//! ```
//!
//! The socket lives in the runtime directory, which is cleared on reboot, so a socket left behind by
//! a crash cannot outlive the session for long. A stale one is detected and replaced at start-up.
//!
//! One command per connection, with the exception of `attach`: an interface sends that and then holds
//! the connection open for as long as it is running, so the daemon can count how many interfaces are
//! driving it and never stop while one of them still needs it. See [`attach`] and [`Command::Leave`].

use serde::{Deserialize, Serialize};

use crate::player::Repeat;
use crate::song::{Song, StreamInfo};

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// What version of this protocol the build speaks.
///
/// Bumped whenever a command or a reply changes shape in a way the other side could misread —
/// a field removed from the status, an argument that means something new. An interface and a
/// daemon from different builds can otherwise sit there misunderstanding each other quietly, which
/// is worse than refusing to talk: the interface asks the daemon what it speaks before it trusts
/// anything else it says. See [`Command::Version`].
pub const PROTOCOL_VERSION: u32 = 1;

/// Name of the socket inside the runtime directory.
const SOCKET_NAME: &str = "ogma.sock";

/// Environment variable that moves the socket, for tests and for anyone running two players.
const SOCKET_ENV: &str = "OGMA_SOCKET";

/// How long a client waits for the player to answer.
///
/// The player answers from its event loop, which wakes ten times a second, so this is generous.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// A track the daemon plays from a URL rather than from disk, and what to call it.
///
/// A stream carries no tags to read, so whatever found it — the YouTube search, for now — says what
/// it is, and that travels with the URL. Without this the queue would list raw URLs, since the
/// daemon has nothing else to go on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StreamTrack {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl StreamTrack {
    /// Everything known about `song`, for a song that is streamed.
    pub fn of(song: &Song) -> Option<Self> {
        let info = song.stream_info()?;

        Some(StreamTrack {
            url: song.uri(),
            title: info.title.clone(),
            artist: info.artist.clone(),
            duration_ms: info.duration.map(|length| length.as_millis() as u64),
        })
    }

    /// The song this describes, ready to be queued.
    pub fn song(&self) -> Song {
        Song::stream(
            &self.url,
            StreamInfo {
                title: self.title.clone(),
                artist: self.artist.clone(),
                duration: self.duration_ms.map(Duration::from_millis),
            },
        )
    }

    /// The tab-separated fields that carry this on one line.
    ///
    /// Tabs again, as for a queue of paths: neither a URL nor a title can contain one, so nothing
    /// needs escaping. Empty fields stand for what is not known.
    fn to_fields(&self) -> String {
        let seconds = match self.duration_ms {
            Some(millis) => (millis / 1000).to_string(),
            None => String::new(),
        };

        format!(
            "{}\t{}\t{}\t{seconds}",
            self.url,
            self.title.as_deref().unwrap_or_default(),
            self.artist.as_deref().unwrap_or_default(),
        )
    }

    /// Read back what [`StreamTrack::to_fields`] wrote.
    ///
    /// Only the URL is required: a caller with nothing else to say — `ogma-cmd play_stream <url>`
    /// from a script, say — sends that alone.
    fn parse_fields(what: &str, argument: &str) -> Result<Self, String> {
        let mut fields = argument.split('\t');

        let url = fields.next().unwrap_or_default().trim();

        if url.is_empty() {
            return Err(format!("{what} needs a url"));
        }

        let text = |field: Option<&str>| -> Option<String> {
            field
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };

        let title = text(fields.next());
        let artist = text(fields.next());
        let duration_ms = text(fields.next())
            .and_then(|seconds| seconds.parse::<u64>().ok())
            .map(|seconds| seconds * 1000);

        Ok(StreamTrack { url: url.to_string(), title, artist, duration_ms })
    }
}

/// What `ogma-cmd` can ask for.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Start playing, or resume from a pause.
    Play,
    /// Hold playback where it is.
    Pause,
    /// Pause if playing, play if not.
    PlayPause,
    /// On to the next song in the queue.
    Next,
    /// Back to the song that played before.
    Previous,
    /// Rearrange the queue.
    Shuffle,
    /// What happens when a song runs out: `None` moves to the next mode, `Some` sets one.
    Repeat(Option<Repeat>),
    /// Replace the queue with a playlist.
    LoadPlaylist(String),
    /// Add a playlist to the end of the queue.
    AddPlaylist(String),
    /// Move the volume by this many percentage points, up or down.
    Volume(i32),
    /// Find a song in the library by name and play it.
    PlaySong(String),

    // --- what the interface needs on top of what a keybinding does ---
    /// Report everything about what is playing, as JSON.
    Status,
    /// Stop playing entirely: no current song, no queue, no history.
    Clear,
    /// Halt playback and rewind, keeping the queue.
    Stop,
    /// Add one file to the end of the queue.
    QueueAdd(PathBuf),
    /// Add every track at or below a folder to the end of the queue.
    QueueAddFolder(PathBuf),
    /// Take the queue entry at this position out.
    QueueRemove(usize),
    /// Empty the queue and the history, leaving a song that is playing alone.
    QueueClear,
    /// Start this file now, sending whatever was playing to the history.
    PlayFile(PathBuf),
    /// Add a track played from the network to the end of the queue.
    QueueStream(StreamTrack),
    /// Start a track played from the network now.
    PlayStream(StreamTrack),
    /// Start the queue entry at this position now.
    PlayIndex(usize),
    /// Move the position by this many seconds, forwards or back.
    Seek(i64),
    /// Replace the queue with these files, in this order.
    SetQueue(Vec<PathBuf>),
    /// Finish: the daemon exits.
    Quit,

    // --- interfaces arriving and leaving ---
    /// An interface saying it is here, and whether it is what started this daemon.
    ///
    /// Sent over a connection the interface then holds open, so an interface that crashes stops
    /// being counted without having to say anything.
    Attach { id: String, spawned: bool },
    /// An interface saying it has gone, and what it would like done if it was the last one here.
    Leave { id: String, on_leave: OnLeave },
    /// How many interfaces are attached.
    Interfaces,
    /// What version of this protocol the daemon speaks.
    Version,
}

/// What a leaving interface would like done with the daemon.
///
/// Only ever acted on once no interface is left: a daemon another interface is still driving is not
/// the leaving one's to stop, whatever its config says.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum OnLeave {
    /// Leave it playing.
    #[default]
    Keep,
    /// Stop it, once nobody else is attached.
    Stop,
    /// Stop it only if an interface is what started it, and nobody else is attached.
    ///
    /// Which interface started it does not matter: an interface that inherits a daemon another one
    /// spawned is the one that has to close it, or nothing would.
    StopIfSpawned,
}

impl OnLeave {
    /// The word the protocol carries.
    pub fn word(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Stop => "stop",
            Self::StopIfSpawned => "stop_if_spawned",
        }
    }

    /// Read one from the protocol. Nothing at all means keep: the safe answer.
    pub fn parse(word: &str) -> Result<Self, String> {
        match word.trim().to_lowercase().replace('-', "_").as_str() {
            "" | "keep" => Ok(Self::Keep),
            "stop" => Ok(Self::Stop),
            "stop_if_spawned" => Ok(Self::StopIfSpawned),
            other => Err(format!("leave takes keep, stop or stop_if_spawned, not {other:?}")),
        }
    }
}

impl Command {
    /// Read a command from a line of text, as the socket carries it.
    pub fn parse(line: &str) -> Result<Self, String> {
        let line = line.trim();
        let (name, argument) = match line.split_once(char::is_whitespace) {
            Some((name, rest)) => (name, rest.trim()),
            None => (line, ""),
        };

        // Hyphens and underscores both read naturally on a command line.
        let name = name.to_lowercase().replace('-', "_");

        let needs_name = |what: &str| -> Result<String, String> {
            if argument.is_empty() {
                Err(format!("{what} needs a name"))
            } else {
                Ok(argument.to_string())
            }
        };

        /// A count or position, which must be a plain number.
        fn number(what: &str, argument: &str) -> Result<usize, String> {
            argument
                .parse::<usize>()
                .map_err(|_| format!("{what} takes a position, not {argument:?}"))
        }

        /// An interface's id and whatever followed it.
        fn split_id(what: &str, argument: &str) -> Result<(String, String), String> {
            if argument.is_empty() {
                return Err(format!("{what} needs an id"));
            }

            match argument.split_once(char::is_whitespace) {
                Some((id, rest)) => Ok((id.to_string(), rest.trim().to_string())),
                None => Ok((argument.to_string(), String::new())),
            }
        }

        match name.as_str() {
            "play" => Ok(Command::Play),
            "pause" => Ok(Command::Pause),
            "play_pause" => Ok(Command::PlayPause),
            "next" => Ok(Command::Next),
            "previous" | "prev" => Ok(Command::Previous),
            "shuffle" => Ok(Command::Shuffle),
            // No mode named cycles, which is what a keybinding wants; a named one sets it, which is
            // what a script wants.
            "repeat" => match argument {
                "" | "cycle" | "next" => Ok(Command::Repeat(None)),
                word => Repeat::parse(word).map(|mode| Command::Repeat(Some(mode))),
            },
            "load_playlist" => Ok(Command::LoadPlaylist(needs_name("load_playlist")?)),
            "add_playlist" => Ok(Command::AddPlaylist(needs_name("add_playlist")?)),
            "play_song" => Ok(Command::PlaySong(needs_name("play_song")?)),
            "status" => Ok(Command::Status),
            "stop" => Ok(Command::Stop),
            "clear" => Ok(Command::Clear),
            "queue_clear" => Ok(Command::QueueClear),
            "quit" => Ok(Command::Quit),
            "close" => Ok(Command::Quit),
            "queue_add" => Ok(Command::QueueAdd(PathBuf::from(needs_name("queue_add")?))),
            "queue_add_folder" => {
                Ok(Command::QueueAddFolder(PathBuf::from(needs_name("queue_add_folder")?)))
            }
            "play_file" => Ok(Command::PlayFile(PathBuf::from(needs_name("play_file")?))),
            "play_stream" => {
                StreamTrack::parse_fields("play_stream", argument).map(Command::PlayStream)
            }
            "queue_stream" => {
                StreamTrack::parse_fields("queue_stream", argument).map(Command::QueueStream)
            }
            "set_queue" => {
                // Paths are separated by tabs, which cannot appear in a path on any system this runs
                // on, so no escaping is needed.
                let paths: Vec<PathBuf> = argument
                    .split('\t')
                    .filter(|part| !part.is_empty())
                    .map(PathBuf::from)
                    .collect();

                Ok(Command::SetQueue(paths))
            }
            "queue_remove" => number("queue_remove", argument).map(Command::QueueRemove),
            "play_index" => number("play_index", argument).map(Command::PlayIndex),
            "seek" => {
                let value = argument.strip_prefix('+').unwrap_or(argument);

                value
                    .parse::<i64>()
                    .map(Command::Seek)
                    .map_err(|_| format!("seek takes a number of seconds, not {argument:?}"))
            }
            "volume" => {
                // A leading `+` is natural for a relative change, and `parse` does not take it.
                let value = argument.strip_prefix('+').unwrap_or(argument);

                value
                    .parse::<i32>()
                    .map(Command::Volume)
                    .map_err(|_| format!("volume takes a number of points, not {argument:?}"))
            }
            "attach" => {
                let (id, rest) = split_id("attach", argument)?;

                match rest.as_str() {
                    "" => Ok(Command::Attach { id, spawned: false }),
                    "spawned" => Ok(Command::Attach { id, spawned: true }),
                    other => Err(format!("attach takes spawned, not {other:?}")),
                }
            }
            // `detach` reads better from a command line, and means the same thing.
            "leave" | "detach" => {
                let (id, rest) = split_id("leave", argument)?;

                Ok(Command::Leave { id, on_leave: OnLeave::parse(&rest)? })
            }
            "interfaces" => Ok(Command::Interfaces),
            "version" | "protocol" => Ok(Command::Version),
            "" => Err("no command".to_string()),
            other => Err(format!("unknown command {other:?}")),
        }
    }

    /// The line of text that carries this command.
    pub fn to_line(&self) -> String {
        match self {
            Command::Play => "play".to_string(),
            Command::Pause => "pause".to_string(),
            Command::PlayPause => "play_pause".to_string(),
            Command::Next => "next".to_string(),
            Command::Previous => "previous".to_string(),
            Command::Shuffle => "shuffle".to_string(),
            Command::Repeat(mode) => match mode {
                Some(mode) => format!("repeat {}", mode.word()),
                None => "repeat".to_string(),
            },
            Command::LoadPlaylist(name) => format!("load_playlist {name}"),
            Command::AddPlaylist(name) => format!("add_playlist {name}"),
            Command::PlaySong(name) => format!("play_song {name}"),
            Command::Volume(points) => format!("volume {points}"),
            Command::Status => "status".to_string(),
            Command::Stop => "stop".to_string(),
            Command::Clear => "clear".to_string(),
            Command::QueueClear => "queue_clear".to_string(),
            Command::Quit => "quit".to_string(),
            Command::QueueAdd(path) => format!("queue_add {}", path.display()),
            Command::QueueAddFolder(path) => format!("queue_add_folder {}", path.display()),
            Command::PlayFile(path) => format!("play_file {}", path.display()),
            Command::PlayStream(track) => format!("play_stream {}", track.to_fields()),
            Command::QueueStream(track) => format!("queue_stream {}", track.to_fields()),
            Command::QueueRemove(index) => format!("queue_remove {index}"),
            Command::PlayIndex(index) => format!("play_index {index}"),
            Command::Seek(seconds) => format!("seek {seconds}"),
            Command::SetQueue(paths) => {
                let joined: Vec<String> =
                    paths.iter().map(|path| path.display().to_string()).collect();

                format!("set_queue {}", joined.join("\t"))
            }
            Command::Attach { id, spawned } => {
                if *spawned {
                    format!("attach {id} spawned")
                } else {
                    format!("attach {id}")
                }
            }
            Command::Leave { id, on_leave } => format!("leave {id} {}", on_leave.word()),
            Command::Interfaces => "interfaces".to_string(),
            Command::Version => "version".to_string(),
        }
    }

    /// Every command's name and what it takes, for the tool's own help.
    pub fn usage() -> &'static [(&'static str, &'static str)] {
        &[
            ("play", "start or resume playback"),
            ("pause", "hold playback where it is"),
            ("play_pause", "pause if playing, play if not"),
            ("next", "on to the next song in the queue"),
            ("previous", "back to the song that played before"),
            ("shuffle", "rearrange the queue"),
            ("repeat [off|queue|song]", "cycle what repeats, or set it"),
            ("load_playlist <name>", "replace the queue with a playlist"),
            ("add_playlist <name>", "add a playlist to the end of the queue"),
            ("play_song <name>", "find a song in the library and play it"),
            ("play_stream <url>", "play a track from the network, e.g. a YouTube URL"),
            ("queue_stream <url>", "add a track from the network to the queue"),
            ("volume <±points>", "move the volume, e.g. 10 or -5"),
            ("seek <±seconds>", "move the position, e.g. 30 or -10"),
            ("queue_clear", "empty the queue and the history, leaving a playing song alone"),
            ("stop", "halt playback and rewind, keeping the queue"),
            ("clear", "stop and forget the queue and history"),
            ("status", "report what is playing, as JSON"),
            ("interfaces", "how many interfaces are attached"),
            ("version", "what version of the socket protocol the daemon speaks"),
            ("quit/close", "kill the daemon, whoever is attached"),
        ]
    }
}

/// What the daemon is doing, as the interface needs to see it.
///
/// Sent as JSON in answer to [`Command::Status`]. Paths rather than tags: the daemon's business is
/// playing audio, and whoever is displaying it can read the tags itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// `playing`, `paused` or `stopped`.
    pub state: String,
    /// How far into the current song playback has reached, in milliseconds.
    pub position_ms: u64,
    /// Fader position, 0.0 to 1.0.
    pub volume: f32,
    /// The file playing now, if any.
    pub current: Option<PathBuf>,
    /// What is queued, next first.
    pub queue: Vec<PathBuf>,
    /// What has played, most recent last.
    pub history: Vec<PathBuf>,
    /// What happens when a song runs out: `off`, `queue` or `song`.
    ///
    /// Defaulted, so a status from a daemon that predates repeat still reads.
    #[serde(default)]
    pub repeat: String,
    /// What the entries above that are URLs rather than files are, so an interface can name them.
    ///
    /// Empty for a daemon playing only local files, and left out of the JSON entirely then.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streams: Vec<StreamTrack>,
}

impl Status {
    /// Read a status back from the JSON the daemon sent.
    pub fn parse(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|err| format!("cannot read status: {err}"))
    }

    /// The JSON the daemon sends.
    pub fn to_json(&self) -> String {
        // A status that cannot be rendered would be a bug in this struct, not a runtime condition.
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// One command from a client, and where its answer goes.
#[derive(Debug)]
pub struct Request {
    pub command: Command,
    reply: Sender<String>,
}

impl Request {
    /// Answer the client. A client that has gone away is not an error worth reporting.
    pub fn answer(self, reply: impl Into<String>) {
        let _ = self.reply.send(reply.into());
    }
}

/// What a daemon says when asked its protocol version.
pub fn version_reply() -> String {
    format!("ok: ipc {PROTOCOL_VERSION}")
}

/// Read a version out of what [`version_reply`] wrote.
///
/// `None` for anything else, which includes the refusal a daemon too old to know the command
/// gives: not knowing how to say which version it speaks is itself an answer.
pub fn parse_version(reply: &str) -> Option<u32> {
    reply.trim().strip_prefix("ok: ipc")?.trim().parse().ok()
}

/// Ask the daemon listening on the usual socket what it speaks.
///
/// `Err` means nobody answered; `Ok(None)` means something answered but not with a version.
pub fn protocol_version() -> Result<Option<u32>, String> {
    protocol_version_of(&socket_path())
}

/// [`protocol_version`], against a daemon listening somewhere else.
pub fn protocol_version_of(path: &Path) -> Result<Option<u32>, String> {
    send_to(path, &Command::Version).map(|reply| parse_version(&reply))
}

/// The socket the player listens on.
///
/// `$OGMA_SOCKET` wins, then the runtime directory, then a name in the temporary directory that
/// includes the user id so two people on one machine do not collide.
pub fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os(SOCKET_ENV) {
        return PathBuf::from(path);
    }

    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join(SOCKET_NAME);
    }

    // Safe enough as a last resort: the name is per-user, and the socket is replaced if stale.
    std::env::temp_dir().join(format!("ogma-{}.sock", users_id()))
}

/// The user id, for naming a socket that is not shared.
fn users_id() -> String {
    std::env::var("UID")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "user".to_string())
}

/// The player's end: a socket, a thread accepting connections, and the commands they carry.
#[derive(Debug)]
pub struct Server {
    requests: Receiver<Request>,
    path: PathBuf,
}

impl Server {
    /// Start listening, replacing a socket left behind by a player that is no longer running.
    ///
    /// Fails when another player is already listening, which is the right outcome: two players
    /// answering the same commands would be worse than one.
    pub fn start() -> std::io::Result<Self> {
        Self::start_at(socket_path())
    }

    /// Start listening on `path`.
    pub fn start_at(path: PathBuf) -> std::io::Result<Self> {
        if path.exists() {
            // Something is there. If it answers, a player is running and this one must not take over.
            if UnixStream::connect(&path).is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!("a player is already listening on {}", path.display()),
                ));
            }

            // Nothing answered, so it is the remains of a player that is gone.
            std::fs::remove_file(&path)?;
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(&path)?;
        let (sender, requests) = mpsc::channel();

        std::thread::Builder::new()
            .name("ogma-ipc".to_string())
            .spawn(move || accept_loop(listener, sender))?;

        Ok(Server { requests, path })
    }

    /// The commands that have arrived and not yet been answered.
    ///
    /// Never blocks: the player calls this from its event loop.
    pub fn pending(&self) -> impl Iterator<Item = Request> + '_ {
        self.requests.try_iter()
    }

    /// Where this server is listening.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Leaving the socket behind would make the next player think one is already running until it
        // tried to connect. Tidier to take it with us.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Take connections until the listener fails or the player goes away.
fn accept_loop(listener: UnixListener, sender: Sender<Request>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            // A failed accept is not fatal; a closed listener ends the loop through `incoming`.
            continue;
        };

        // One command per connection, handled on its own thread so a slow client cannot hold up the
        // next one.
        let sender = sender.clone();
        let handled = std::thread::Builder::new()
            .name("ogma-ipc-client".to_string())
            .spawn(move || serve(stream, sender));

        if handled.is_err() {
            return;
        }
    }
}

/// Read one command, hand it to the player, write back what the player says.
///
/// `attach` is the exception to one command per connection: its connection is held open afterwards,
/// and when it ends — the interface closed, or was killed — the player is told the interface has gone.
fn serve(stream: UnixStream, sender: Sender<Request>) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    });
    let mut writer = stream;

    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }

    let (reply, held) = match Command::parse(&line) {
        Ok(command) => {
            // The id is kept before the command goes, so the departure can be reported under it.
            let held = match &command {
                Command::Attach { id, .. } => Some(id.clone()),
                _ => None,
            };

            (ask(&sender, command), held)
        }
        Err(err) => (format!("error: {err}"), None),
    };

    let _ = writeln!(writer, "{reply}");
    let _ = writer.flush();

    let Some(id) = held else {
        return;
    };

    // Nothing more is expected on this connection; what matters is when it ends. Anything that does
    // arrive is read and dropped, so a client writing to it cannot wedge itself.
    let mut ignored = String::new();
    while matches!(reader.read_line(&mut ignored), Ok(read) if read > 0) {
        ignored.clear();
    }

    // Keep, not the interface's own wish: an interface that vanished without saying so has not asked
    // for anything, and guessing that it wanted the music stopped would be the wrong guess.
    let _ = ask(&sender, Command::Leave { id, on_leave: OnLeave::Keep });
}

/// Put one command to the player and wait for its answer.
fn ask(sender: &Sender<Request>, command: Command) -> String {
    let (answer, answered) = mpsc::channel();

    if sender.send(Request { command, reply: answer }).is_err() {
        return "error: the player is not accepting commands".to_string();
    }

    // The player answers from its event loop, so this waits rather than assuming.
    match answered.recv_timeout(REPLY_TIMEOUT) {
        Ok(reply) => reply,
        Err(_) => "error: the player did not answer".to_string(),
    }
}

/// A held connection that tells a daemon an interface is here.
///
/// Nothing is ever sent over the connection again: it is held open because the daemon takes its
/// closing as the interface having gone. An interface that is killed, or panics, therefore stops
/// being counted at once rather than keeping a daemon alive forever.
#[derive(Debug)]
pub struct Attachment {
    /// What this interface is called, for the daemon to count it under.
    id: String,
    /// Where the daemon is listening, for saying goodbye over a connection of its own.
    path: PathBuf,
    /// The connection whose end means this interface is gone.
    _hold: UnixStream,
}

impl Attachment {
    /// What this interface is called.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Say this interface has gone, and what to do with the daemon if it was the last one.
    ///
    /// The daemon decides: it knows how many interfaces are left, and only it can be sure another one
    /// has not attached in the meantime. Its reply says what it did.
    pub fn leave(&self, on_leave: OnLeave) -> Result<String, String> {
        send_to(&self.path, &Command::Leave { id: self.id.clone(), on_leave })
    }
}

/// Names for attachments, so two interfaces in one process are still two interfaces.
static ATTACHMENTS: AtomicUsize = AtomicUsize::new(0);

/// Attach to the daemon as an interface, saying whether this process is what started it.
pub fn attach(spawned: bool) -> Result<Attachment, String> {
    attach_to(&socket_path(), spawned)
}

/// [`attach`], to the daemon listening on `path`.
pub fn attach_to(path: &Path, spawned: bool) -> Result<Attachment, String> {
    let id = format!("{}-{}", std::process::id(), ATTACHMENTS.fetch_add(1, Ordering::Relaxed));

    let stream = UnixStream::connect(path)
        .map_err(|err| format!("no player listening on {} ({err})", path.display()))?;

    // Only while attaching: the connection is held open afterwards, where a read timeout would end it.
    stream.set_read_timeout(Some(REPLY_TIMEOUT)).ok();

    let mut writer = stream.try_clone().map_err(|err| err.to_string())?;
    let command = Command::Attach { id: id.clone(), spawned };
    writeln!(writer, "{}", command.to_line()).map_err(|err| err.to_string())?;
    writer.flush().map_err(|err| err.to_string())?;

    let mut reply = String::new();
    BufReader::new(stream.try_clone().map_err(|err| err.to_string())?)
        .read_line(&mut reply)
        .map_err(|err| format!("no answer from the player ({err})"))?;

    if reply.trim_start().starts_with("error") {
        return Err(reply.trim().to_string());
    }

    // The daemon has answered, so nothing else will be read from here; a timeout would only close a
    // connection whose whole job is to stay open.
    stream.set_read_timeout(None).ok();

    Ok(Attachment { id, path: path.to_path_buf(), _hold: stream })
}

/// Send one command to a running player and return what it says.
pub fn send(command: &Command) -> Result<String, String> {
    send_to(&socket_path(), command)
}

/// Send one command to the player listening on `path`.
pub fn send_to(path: &std::path::Path, command: &Command) -> Result<String, String> {
    let stream = UnixStream::connect(path).map_err(|err| {
        format!("no player listening on {} ({err})", path.display())
    })?;

    stream.set_read_timeout(Some(REPLY_TIMEOUT)).ok();

    let mut writer = stream.try_clone().map_err(|err| err.to_string())?;
    writeln!(writer, "{}", command.to_line()).map_err(|err| err.to_string())?;
    writer.flush().map_err(|err| err.to_string())?;

    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(|err| format!("no answer from the player ({err})"))?;

    Ok(reply.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_read_from_their_lines() {
        assert_eq!(Command::parse("play").expect("a command"), Command::Play);
        assert_eq!(Command::parse("  PAUSE  ").expect("a command"), Command::Pause);
        assert_eq!(Command::parse("play_pause").expect("a command"), Command::PlayPause);
        assert_eq!(Command::parse("play-pause").expect("a command"), Command::PlayPause);

        // Only that spelling: `toggle` on its own is not a command.
        assert!(Command::parse("toggle").is_err());
        assert_eq!(Command::parse("prev").expect("a command"), Command::Previous);
        assert_eq!(
            Command::parse("load-playlist Late Night").expect("a command"),
            Command::LoadPlaylist("Late Night".to_string())
        );
        assert_eq!(
            Command::parse("play_song Windowlicker").expect("a command"),
            Command::PlaySong("Windowlicker".to_string())
        );

        // Emptying the queue is not stopping: `clear` forgets the history and what is playing too.
        assert_eq!(Command::parse("queue_clear").expect("a command"), Command::QueueClear);
        assert_eq!(Command::parse("queue-clear").expect("a command"), Command::QueueClear);
        assert_eq!(Command::parse("clear").expect("a command"), Command::Clear);

        // And it is listed, so `ogma-cmd` with no arguments names it.
        assert!(Command::usage().iter().any(|(name, _)| *name == "queue_clear"));
    }

    #[test]
    fn a_volume_change_may_go_either_way() {
        assert_eq!(Command::parse("volume 10").expect("a command"), Command::Volume(10));
        assert_eq!(Command::parse("volume +10").expect("a command"), Command::Volume(10));
        assert_eq!(Command::parse("volume -5").expect("a command"), Command::Volume(-5));

        assert!(Command::parse("volume loud").is_err());
        assert!(Command::parse("volume").is_err());
    }

    #[test]
    fn an_interface_names_itself_and_says_what_it_wants_on_the_way_out() {
        assert_eq!(
            Command::parse("attach 4213-0").expect("a command"),
            Command::Attach { id: "4213-0".to_string(), spawned: false }
        );
        assert_eq!(
            Command::parse("attach 4213-0 spawned").expect("a command"),
            Command::Attach { id: "4213-0".to_string(), spawned: true }
        );

        // Nothing said about the daemon means leave it alone, which is the safe reading.
        assert_eq!(
            Command::parse("leave 4213-0").expect("a command"),
            Command::Leave { id: "4213-0".to_string(), on_leave: OnLeave::Keep }
        );
        assert_eq!(
            Command::parse("detach 4213-0 stop").expect("a command"),
            Command::Leave { id: "4213-0".to_string(), on_leave: OnLeave::Stop }
        );
        assert_eq!(
            Command::parse("leave 4213-0 stop-if-spawned").expect("a command"),
            Command::Leave { id: "4213-0".to_string(), on_leave: OnLeave::StopIfSpawned }
        );
        assert_eq!(Command::parse("interfaces").expect("a command"), Command::Interfaces);

        assert!(Command::parse("attach").unwrap_err().contains("needs an id"));
        assert!(Command::parse("leave").unwrap_err().contains("needs an id"));
        assert!(Command::parse("attach 1 sideways").unwrap_err().contains("spawned"));
        assert!(Command::parse("leave 1 sideways").unwrap_err().contains("keep, stop"));
    }

    #[test]
    fn repeat_is_cycled_by_a_bare_command_and_set_by_a_named_one() {
        assert_eq!(Command::parse("repeat").expect("a command"), Command::Repeat(None));
        assert_eq!(Command::parse("repeat cycle").expect("a command"), Command::Repeat(None));
        assert_eq!(
            Command::parse("repeat off").expect("a command"),
            Command::Repeat(Some(Repeat::Off))
        );
        assert_eq!(
            Command::parse("repeat QUEUE").expect("a command"),
            Command::Repeat(Some(Repeat::Queue))
        );

        // The names other players use read the same here.
        assert_eq!(
            Command::parse("repeat all").expect("a command"),
            Command::Repeat(Some(Repeat::Queue))
        );
        assert_eq!(
            Command::parse("repeat one").expect("a command"),
            Command::Repeat(Some(Repeat::Song))
        );

        assert!(Command::parse("repeat sideways").unwrap_err().contains("off, queue or song"));
    }

    #[test]
    fn what_cannot_be_read_says_why() {
        assert!(Command::parse("").unwrap_err().contains("no command"));
        assert!(Command::parse("dance").unwrap_err().contains("unknown"));
        assert!(Command::parse("load_playlist").unwrap_err().contains("needs a name"));
        assert!(Command::parse("add_playlist  ").unwrap_err().contains("needs a name"));
    }

    #[test]
    fn every_command_survives_the_round_trip() {
        for command in [
            Command::Play,
            Command::Pause,
            Command::PlayPause,
            Command::Next,
            Command::Previous,
            Command::Shuffle,
            Command::LoadPlaylist("Late Night".to_string()),
            Command::AddPlaylist("Road Trip".to_string()),
            Command::PlaySong("Xtal".to_string()),
            Command::Volume(-15),
            Command::Repeat(None),
            Command::Repeat(Some(Repeat::Off)),
            Command::Repeat(Some(Repeat::Queue)),
            Command::Repeat(Some(Repeat::Song)),
            Command::Attach { id: "4213-0".to_string(), spawned: false },
            Command::Attach { id: "4213-1".to_string(), spawned: true },
            Command::Leave { id: "4213-0".to_string(), on_leave: OnLeave::Keep },
            Command::Leave { id: "4213-0".to_string(), on_leave: OnLeave::Stop },
            Command::Leave { id: "4213-1".to_string(), on_leave: OnLeave::StopIfSpawned },
            Command::Interfaces,
            Command::QueueClear,
        ] {
            let line = command.to_line();

            assert_eq!(Command::parse(&line).expect("a command"), command, "via {line:?}");
        }
    }
}
