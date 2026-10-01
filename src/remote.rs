//! Driving the daemon from an interface.
//!
//! The queue and what is playing live in the daemon, so the interface keeps a [`Player`] that mirrors
//! it and sends commands for anything that should change. Each change is applied to the mirror as
//! well, so the screen answers a key press at once rather than a tick later; the next status from the
//! daemon is what the mirror is then reconciled to, so an optimistic guess cannot persist.

use std::path::Path;
use std::time::Duration;

use crate::ipc::{self, Attachment, Command, OnLeave, Status, StreamTrack};
use crate::player::{Controls, Mirrored, PlaybackState, Player, Repeat};
use crate::playlist::Playlist;
use crate::song::{Song, StreamInfo};

/// A connection to the daemon, and the mirror it keeps up to date.
#[derive(Debug)]
pub struct Remote {
    /// What the daemon last said it was doing, for the screens to draw.
    mirror: Player,
    /// Whether the last exchange with the daemon worked.
    connected: bool,
    /// Why the daemon could not be reached, if it could not.
    error: Option<String>,
    /// The held connection that has this interface counted among the daemon's.
    ///
    /// Held for as long as the interface runs: the daemon takes its closing as this interface having
    /// gone, which is what keeps a crashed interface from holding a daemon open forever.
    attachment: Option<Attachment>,
}

impl Default for Remote {
    fn default() -> Self {
        Self::new()
    }
}

impl Remote {
    pub fn new() -> Self {
        Remote { mirror: Player::new(), connected: false, error: None, attachment: None }
    }

    /// Tell the daemon this interface is here, so it knows not to stop while it is.
    ///
    /// `spawned` says whether this process is what started the daemon; the daemon remembers that
    /// rather than each interface, so whichever interface leaves last is the one that closes it.
    pub fn attach(&mut self, spawned: bool) {
        match ipc::attach(spawned) {
            Ok(attachment) => {
                self.attachment = Some(attachment);
                self.connected = true;
                self.error = None;
            }
            Err(err) => {
                self.connected = false;
                self.error = Some(err);
            }
        }
    }

    /// Whether the daemon is counting this interface.
    pub fn attached(&self) -> bool {
        self.attachment.is_some()
    }

    /// Say this interface has gone, asking for the daemon to stop if nothing else needs it.
    ///
    /// The daemon answers, since only it knows how many interfaces are left; `None` means this
    /// interface was never attached, so nobody was counting and the caller has to decide for itself.
    pub fn leave(&mut self, on_leave: OnLeave) -> Option<String> {
        let attachment = self.attachment.take()?;

        match attachment.leave(on_leave) {
            Ok(reply) => {
                self.connected = true;
                self.error = None;

                Some(reply)
            }
            Err(err) => {
                self.connected = false;
                self.error = Some(err);

                None
            }
        }
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

        // A word this build does not know reads as off rather than failing the whole status: the rest
        // of it still says what is playing.
        let repeat = Repeat::parse(&status.repeat).unwrap_or_default();

        // The daemon sends paths; the ones that are URLs come with what they are called, so the
        // screen lists a streamed track by its title rather than by its link.
        let named = |path: &Path| -> Song {
            let uri = path.to_string_lossy();

            match status.streams.iter().find(|track| track.url == uri) {
                Some(track) => track.song(),
                None if crate::ytdl::is_stream(&uri) => {
                    Song::stream(&uri, StreamInfo::default())
                }
                None => Song::new(path),
            }
        };

        self.mirror.mirror(Mirrored {
            state,
            position: Duration::from_millis(status.position_ms),
            volume: status.volume,
            current: status.current.as_deref().map(named),
            queue: status.queue.iter().map(|path| named(path)).collect(),
            history: status.history.iter().map(|path| named(path)).collect(),
            repeat,
        });
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

    /// Ask the daemon to finish, whoever else is attached.
    ///
    /// [`Remote::leave`] is what closing an interface uses; this is the blunt version, for a caller
    /// that means it.
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

    fn cycle_repeat(&mut self) -> Repeat {
        // Moved in the mirror too, so the screen says the new mode at once; the next status is what
        // settles it.
        let mode = Controls::cycle_repeat(&mut self.mirror);
        self.send(Command::Repeat(None));

        mode
    }

    fn set_repeat(&mut self, repeat: Repeat) {
        Controls::set_repeat(&mut self.mirror, repeat);
        self.send(Command::Repeat(Some(repeat)));
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
        // A stream is queued by URL and title together: the daemon has no file to read a name from.
        let command = match StreamTrack::of(&song) {
            Some(track) => Command::QueueStream(track),
            None => Command::QueueAdd(song.path().to_path_buf()),
        };

        Controls::add_queue(&mut self.mirror, song);
        self.send(command);
    }

    fn add_queue_all(&mut self, songs: Vec<Song>) {
        let paths: Vec<std::path::PathBuf> =
            songs.iter().map(|song| song.path().to_path_buf()).collect();

        // What the daemon cannot work out for itself goes first: a queue is sent as bare URIs, so a
        // stream it has never been told about would arrive nameless. Naming them here means the
        // queue that follows finds them already known.
        let streams: Vec<StreamTrack> = songs.iter().filter_map(StreamTrack::of).collect();

        Controls::add_queue_all(&mut self.mirror, songs);

        for track in streams {
            self.send(Command::QueueStream(track));
        }

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

    fn clear_queue(&mut self) {
        Controls::clear_queue(&mut self.mirror);
        self.send(Command::QueueClear);
    }

    fn play_now(&mut self, song: Song) {
        let command = match StreamTrack::of(&song) {
            Some(track) => Command::PlayStream(track),
            None => Command::PlayFile(song.path().to_path_buf()),
        };

        Controls::play_now(&mut self.mirror, song);
        self.send(command);
    }

    fn play_queued(&mut self, index: usize) {
        Controls::play_queued(&mut self.mirror, index);
        self.send(Command::PlayIndex(index));
    }

    fn replace_queue_with_playlist(&mut self, playlist: &Playlist) {
        let streams: Vec<StreamTrack> =
            playlist.ordered().iter().filter_map(|song| StreamTrack::of(song)).collect();

        Controls::replace_queue_with_playlist(&mut self.mirror, playlist);

        // As for a queue of songs: the daemon is told what the streams are before it is told to
        // play them, or it would have only their URLs to show.
        for track in streams {
            self.send(Command::QueueStream(track));
        }

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
