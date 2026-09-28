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

use serde::{Deserialize, Serialize};

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// Name of the socket inside the runtime directory.
const SOCKET_NAME: &str = "ogma.sock";

/// Environment variable that moves the socket, for tests and for anyone running two players.
const SOCKET_ENV: &str = "OGMA_SOCKET";

/// How long a client waits for the player to answer.
///
/// The player answers from its event loop, which wakes ten times a second, so this is generous.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// Empty the queue, leaving what is playing alone.
    QueueClear,
    /// Start this file now, sending whatever was playing to the history.
    PlayFile(PathBuf),
    /// Start the queue entry at this position now.
    PlayIndex(usize),
    /// Move the position by this many seconds, forwards or back.
    Seek(i64),
    /// Replace the queue with these files, in this order.
    SetQueue(Vec<PathBuf>),
    /// Finish: the daemon exits.
    Quit,
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

        match name.as_str() {
            "play" => Ok(Command::Play),
            "pause" => Ok(Command::Pause),
            "play_pause" => Ok(Command::PlayPause),
            "next" => Ok(Command::Next),
            "previous" | "prev" => Ok(Command::Previous),
            "shuffle" => Ok(Command::Shuffle),
            "load_playlist" => Ok(Command::LoadPlaylist(needs_name("load_playlist")?)),
            "add_playlist" => Ok(Command::AddPlaylist(needs_name("add_playlist")?)),
            "play_song" => Ok(Command::PlaySong(needs_name("play_song")?)),
            "status" => Ok(Command::Status),
            "stop" => Ok(Command::Stop),
            "clear" => Ok(Command::Clear),
            "queue_clear" => Ok(Command::QueueClear),
            "quit" => Ok(Command::Quit),
            "queue_add" => Ok(Command::QueueAdd(PathBuf::from(needs_name("queue_add")?))),
            "queue_add_folder" => {
                Ok(Command::QueueAddFolder(PathBuf::from(needs_name("queue_add_folder")?)))
            }
            "play_file" => Ok(Command::PlayFile(PathBuf::from(needs_name("play_file")?))),
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
            Command::QueueRemove(index) => format!("queue_remove {index}"),
            Command::PlayIndex(index) => format!("play_index {index}"),
            Command::Seek(seconds) => format!("seek {seconds}"),
            Command::SetQueue(paths) => {
                let joined: Vec<String> =
                    paths.iter().map(|path| path.display().to_string()).collect();

                format!("set_queue {}", joined.join("\t"))
            }
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
            ("load_playlist <name>", "replace the queue with a playlist"),
            ("add_playlist <name>", "add a playlist to the end of the queue"),
            ("play_song <name>", "find a song in the library and play it"),
            ("volume <±points>", "move the volume, e.g. 10 or -5"),
            ("seek <±seconds>", "move the position, e.g. 30 or -10"),
            ("stop", "halt playback and rewind, keeping the queue"),
            ("clear", "stop and forget the queue and history"),
            ("status", "report what is playing, as JSON"),
            ("quit", "ask the daemon to finish"),
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

    let reply = match Command::parse(&line) {
        Ok(command) => {
            let (answer, answered) = mpsc::channel();

            if sender.send(Request { command, reply: answer }).is_err() {
                "error: the player is not accepting commands".to_string()
            } else {
                // The player answers from its event loop, so this waits rather than assuming.
                match answered.recv_timeout(REPLY_TIMEOUT) {
                    Ok(reply) => reply,
                    Err(_) => "error: the player did not answer".to_string(),
                }
            }
        }
        Err(err) => format!("error: {err}"),
    };

    let _ = writeln!(writer, "{reply}");
    let _ = writer.flush();
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
        ] {
            let line = command.to_line();

            assert_eq!(Command::parse(&line).expect("a command"), command, "via {line:?}");
        }
    }
}
