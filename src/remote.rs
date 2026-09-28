//! Driving the daemon from an interface.
//!
//! The queue and what is playing live in the daemon, so the interface keeps a [`Player`] that mirrors
//! it and sends commands for anything that should change. Each change is applied to the mirror as
//! well, so the screen answers a key press at once rather than a tick later; the next status from the
//! daemon is what the mirror is then reconciled to, so an optimistic guess cannot persist.

use std::path::Path;
use std::time::Duration;

use crate::ipc::{self, Command, Status};
use crate::player::{Controls, PlaybackState, Player};
use crate::playlist::Playlist;
use crate::song::Song;

/// A connection to the daemon, and the mirror it keeps up to date.
#[derive(Debug)]
pub struct Remote {
    /// What the daemon last said it was doing, for the screens to draw.
    mirror: Player,
    /// Whether the last exchange with the daemon worked.
    connected: bool,
    /// Why the daemon could not be reached, if it could not.
    error: Option<String>,
}

impl Default for Remote {
    fn default() -> Self {
        Self::new()
    }
}

impl Remote {
    pub fn new() -> Self {
        Remote { mirror: Player::new(), connected: false, error: None }
    }

    /// What the daemon is doing, as far as the last status said.
    pub fn player(&self) -> &Player {
        &self.mirror
    }

    /// Whether the daemon answered last time it was asked.
    pub fn connected(&self) -> bool {
        self.connected
    }

    /// Why the daemon could not be reached, if it could not.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Ask the daemon what it is doing and bring the mirror into line.
    pub fn refresh(&mut self) {
        let reply = match ipc::send(&Command::Status) {
            Ok(reply) => reply,
            Err(err) => {
                self.connected = false;
                self.error = Some(err);
                return;
            }
        };

        match Status::parse(&reply) {
            Ok(status) => {
                self.connected = true;
                self.error = None;
                self.take(&status);
            }
            Err(err) => {
                self.connected = false;
                self.error = Some(err);
            }
        }
    }

    /// Put a status into the mirror.
    fn take(&mut self, status: &Status) {
        let state = match status.state.as_str() {
            "playing" => PlaybackState::Playing,
            "paused" => PlaybackState::Paused,
            _ => PlaybackState::Stopped,
        };

        self.mirror.mirror(
            state,
            Duration::from_millis(status.position_ms),
            status.volume,
            status.current.as_deref().map(Song::new),
            status.queue.iter().map(Song::new).collect(),
            status.history.iter().map(Song::new).collect(),
        );
    }

    /// Send a command, noting whether the daemon was there to take it.
    fn send(&mut self, command: Command) {
        match ipc::send(&command) {
            Ok(_) => {
                self.connected = true;
                self.error = None;
            }
            Err(err) => {
                self.connected = false;
                self.error = Some(err);
            }
        }
    }

    /// Ask the daemon to finish.
    pub fn quit_daemon(&mut self) {
        self.send(Command::Quit);
    }
}

/// Everything the panes ask for goes to the daemon, and to the mirror so the screen keeps up.
impl Controls for Remote {
    fn queue_len(&self) -> usize {
        self.mirror.queue().len()
    }

    fn play(&mut self) {
        Controls::play(&mut self.mirror);
        self.send(Command::Play);
    }

    fn pause(&mut self) {
        Controls::pause(&mut self.mirror);
        self.send(Command::Pause);
    }

    fn toggle_pause(&mut self) {
        Controls::toggle_pause(&mut self.mirror);
        self.send(Command::PlayPause);
    }

    fn stop(&mut self) {
        Controls::stop(&mut self.mirror);
        self.send(Command::Stop);
    }

    fn skip(&mut self) {
        Controls::skip(&mut self.mirror);
        self.send(Command::Next);
    }

    fn previous(&mut self) {
        Controls::previous(&mut self.mirror);
        self.send(Command::Previous);
    }

    fn shuffle_queue(&mut self) {
        // The daemon does the shuffling: two shuffles would disagree, and its one is the one that
        // plays. The mirror catches up on the next status.
        self.send(Command::Shuffle);
    }

    fn change_volume(&mut self, change: f32) {
        Controls::change_volume(&mut self.mirror, change);

        // Sent as whole points, which is what the protocol carries.
        self.send(Command::Volume((change * 100.0).round() as i32));
    }

    fn seek(&mut self, change: Duration, forwards: bool) {
        Controls::seek(&mut self.mirror, change, forwards);

        let seconds = change.as_secs() as i64;
        self.send(Command::Seek(if forwards { seconds } else { -seconds }));
    }

    fn add_queue(&mut self, song: Song) {
        let path = song.path().to_path_buf();
        Controls::add_queue(&mut self.mirror, song);
        self.send(Command::QueueAdd(path));
    }

    fn add_queue_all(&mut self, songs: Vec<Song>) {
        let paths: Vec<std::path::PathBuf> =
            songs.iter().map(|song| song.path().to_path_buf()).collect();

        Controls::add_queue_all(&mut self.mirror, songs);

        // One command for the lot: a folder of a thousand tracks should not be a thousand exchanges.
        let mut queue: Vec<std::path::PathBuf> =
            self.mirror.queue().iter().map(|song| song.path().to_path_buf()).collect();

        // The mirror already holds the new entries, so the whole queue is what gets sent.
        if paths.is_empty() {
            return;
        }

        queue.dedup();
        self.send(Command::SetQueue(queue));
    }

    fn remove_queue(&mut self, index: usize) {
        Controls::remove_queue(&mut self.mirror, index);
        self.send(Command::QueueRemove(index));
    }

    fn play_now(&mut self, song: Song) {
        let path = song.path().to_path_buf();
        Controls::play_now(&mut self.mirror, song);
        self.send(Command::PlayFile(path));
    }

    fn play_queued(&mut self, index: usize) {
        Controls::play_queued(&mut self.mirror, index);
        self.send(Command::PlayIndex(index));
    }

    fn replace_queue_with_playlist(&mut self, playlist: &Playlist) {
        Controls::replace_queue_with_playlist(&mut self.mirror, playlist);

        let paths: Vec<std::path::PathBuf> =
            playlist.ordered().iter().map(|song| song.path().to_path_buf()).collect();

        self.send(Command::SetQueue(paths));
    }
}

/// Where the daemon is expected to be listening, for anything that wants to say so.
pub fn socket_path() -> std::path::PathBuf {
    ipc::socket_path()
}

/// Whether a path is a file the daemon could play, for callers checking before they ask.
pub fn is_playable(path: &Path) -> bool {
    crate::meta::has_audio_extension(path)
}
