//! The part of the player that makes sound.
//!
//! The daemon owns the queue, what has played, what is playing, and the audio device. Nothing else:
//! no interface, no library browsing, no tag writing. It answers commands over the socket in
//! [`crate::ipc`], and reports its state to whoever asks, so an interface can show what is happening
//! without being the thing that makes it happen.
//!
//! Both `ogma-cmd` and the terminal interface are clients of this.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::audio::AudioEngine;
use crate::config::Config;
use crate::ipc::{self, Command, Status};
use crate::library;
use crate::player::{PlaybackState, Player};
use crate::playlist::Playlist;
use crate::song::Song;
use crate::volume;

/// How often the daemon looks at the clock and the socket.
///
/// Fine enough that a seek or a track change feels immediate, coarse enough to leave the machine
/// alone while a song plays.
const TICK: Duration = Duration::from_millis(20);

/// The queue, the audio device, and the loop that drives them.
pub struct Daemon {
    player: Player,
    /// The device, when one could be opened. Without it the daemon still tracks state, which keeps
    /// an interface usable on a machine with no sound.
    audio: Option<AudioEngine>,
    /// Why there is no sound, if there is none.
    audio_error: Option<String>,
    /// Where a song named on the command line is looked for.
    library_root: Option<std::path::PathBuf>,
    running: bool,
}

impl std::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Daemon")
            .field("queued", &self.player.queue().len())
            .field("state", &self.player.state())
            .field("audio", &self.audio.is_some())
            .finish()
    }
}

impl Default for Daemon {
    fn default() -> Self {
        Self::new()
    }
}

impl Daemon {
    /// Open the audio device and take the volume and library from the config.
    pub fn new() -> Self {
        let config = Config::load();

        let (audio, audio_error) = match AudioEngine::new(*config.get_master_volume()) {
            Ok(engine) => (Some(engine), None),
            Err(err) => (None, Some(err.to_string())),
        };

        Daemon {
            player: Player::with_config(&config),
            audio,
            audio_error,
            library_root: config.default_folder().map(Path::to_path_buf),
            running: true,
        }
    }

    /// Listen on the socket and play until asked to finish.
    ///
    /// Fails only when the socket cannot be opened, which usually means another daemon has it.
    pub fn run(&mut self) -> std::io::Result<()> {
        let server = ipc::Server::start()?;

        if let Some(err) = &self.audio_error {
            // Worth saying once: the daemon will otherwise look like it is working and be silent.
            eprintln!("ogma-daemon: {err}");
        }

        eprintln!("ogma-daemon: listening on {}", server.path().display());

        while self.running {
            for request in server.pending().collect::<Vec<_>>() {
                let reply = self.apply(request.command.clone());
                request.answer(reply);
            }

            self.tick();
            std::thread::sleep(TICK);
        }

        Ok(())
    }

    /// Follow the audio: move the clock, and move on when a song ends.
    pub fn tick(&mut self) {
        let Some(audio) = &self.audio else {
            return;
        };

        if audio.loaded().is_some() && self.player.is_playing() {
            self.player.set_position(audio.position());
        }

        if audio.finished() {
            self.player.skip();
            self.sync();
        }
    }

    /// What the daemon is doing.
    pub fn status(&self) -> Status {
        Status {
            state: match self.player.state() {
                PlaybackState::Playing => "playing",
                PlaybackState::Paused => "paused",
                PlaybackState::Stopped => "stopped",
            }
            .to_string(),
            position_ms: self.player.position().as_millis() as u64,
            volume: self.player.volume(),
            current: self.player.current().map(|song| song.path().to_path_buf()),
            queue: self.player.queue().iter().map(|song| song.path().to_path_buf()).collect(),
            history: self.player.history().iter().map(|song| song.path().to_path_buf()).collect(),
        }
    }

    /// The queue and what is playing, for anything driving the daemon in-process.
    pub fn player(&self) -> &Player {
        &self.player
    }

    /// Whether the loop is still meant to be running.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Carry out one command, and say what came of it.
    ///
    /// The reply is what a client prints, so it says what happened rather than only that the command
    /// was understood.
    pub fn apply(&mut self, command: Command) -> String {
        let reply = self.act(command);

        // Every command may have changed what should be playing; the device is brought into line once,
        // here, rather than in each arm.
        self.sync();

        reply
    }

    fn act(&mut self, command: Command) -> String {
        match command {
            Command::Status => self.status().to_json(),

            Command::Play => {
                if self.player.play() {
                    self.playing_now()
                } else {
                    "error: nothing to play".to_string()
                }
            }
            Command::Pause => {
                self.player.pause();

                if self.player.is_paused() { "ok: paused".to_string() } else { "ok: not playing".to_string() }
            }
            Command::PlayPause => {
                self.player.toggle_pause();

                match (self.player.is_playing(), self.player.is_paused()) {
                    (true, _) => self.playing_now(),
                    (_, true) => "ok: paused".to_string(),
                    _ => "error: nothing to play".to_string(),
                }
            }
            Command::Stop => {
                self.player.stop();

                "ok: stopped".to_string()
            }
            Command::Clear => {
                self.player.clear();

                "ok: cleared".to_string()
            }
            Command::Next => match self.player.skip() {
                Some(song) => format!("ok: playing {}", song.display_title()),
                None => "ok: end of queue".to_string(),
            },
            Command::Previous => match self.player.previous() {
                Some(song) => format!("ok: playing {}", song.display_title()),
                None => "ok: restarted".to_string(),
            },
            Command::Shuffle => {
                self.player.shuffle_queue();

                format!("ok: shuffled {} queued", self.player.queue().len())
            }
            Command::Volume(points) => {
                self.player.change_volume(points as f32 / 100.0);
                let level = self.player.volume();

                format!(
                    "ok: volume {}% ({})",
                    volume::percent(level),
                    volume::display_decibels(level)
                )
            }
            Command::Seek(seconds) => {
                let position = self.player.position();

                let moved = if seconds.is_negative() {
                    position.saturating_sub(Duration::from_secs(seconds.unsigned_abs()))
                } else {
                    position + Duration::from_secs(seconds as u64)
                };

                self.player.set_position(moved);

                format!("ok: at {}", format_time(self.player.position()))
            }

            // --- the queue ---
            Command::QueueAdd(path) => {
                let song = Song::new(&path);
                let title = song.display_title();
                self.player.add_queue(song);

                format!("ok: queued {title}")
            }
            Command::QueueAddFolder(path) => {
                let songs = library::scan(&path);
                let count = songs.len();
                self.player.add_queue_all(songs);

                format!("ok: queued {count}")
            }
            Command::QueueRemove(index) => match self.player.remove_queue(index) {
                Some(song) => format!("ok: removed {}", song.display_title()),
                None => format!("error: nothing queued at {index}"),
            },
            Command::QueueClear => {
                self.player.clear_queue();

                "ok: queue cleared".to_string()
            }
            Command::SetQueue(paths) => {
                let count = paths.len();
                self.player.replace_queue(paths.into_iter().map(Song::new));

                format!("ok: queue of {count}")
            }
            Command::PlayFile(path) => {
                if !path.exists() {
                    return format!("error: no such file {}", path.display());
                }

                let song = Song::new(&path);
                self.player.play_now(song);

                self.playing_now()
            }
            Command::PlayIndex(index) => match self.player.remove_queue(index) {
                Some(song) => {
                    self.player.play_now(song);

                    self.playing_now()
                }
                None => format!("error: nothing queued at {index}"),
            },

            // --- playlists, by name ---
            Command::LoadPlaylist(name) => match find_playlist(&name) {
                Some(playlist) => {
                    self.player.replace_queue_with_playlist(&playlist);

                    format!(
                        "ok: queued {} from {} by {}",
                        playlist.len(),
                        playlist.name(),
                        playlist.sort().label()
                    )
                }
                None => format!("error: no playlist matching {name:?}"),
            },
            Command::AddPlaylist(name) => match find_playlist(&name) {
                Some(playlist) => {
                    let added = playlist.len();
                    self.player.add_queue_all(playlist.ordered_songs());

                    format!("ok: added {added} from {}", playlist.name())
                }
                None => format!("error: no playlist matching {name:?}"),
            },
            Command::PlaySong(name) => match self.find_song(&name) {
                Some(song) => {
                    let title = song.display_title();
                    let artist = song.display_artist().to_string();
                    self.player.play_now(song);

                    format!("ok: playing {artist} — {title}")
                }
                None => format!("error: no song matching {name:?}"),
            },

            Command::Quit => {
                self.running = false;

                "ok: finishing".to_string()
            }
        }
    }

    /// What is playing, for the replies that report it.
    fn playing_now(&self) -> String {
        match self.player.current() {
            Some(song) => format!("ok: playing {}", song.display_title()),
            None => "ok: playing".to_string(),
        }
    }

    /// Make the device play whatever the queue says should be playing.
    fn sync(&mut self) {
        let Some(audio) = &mut self.audio else {
            return;
        };

        audio.set_volume(self.player.volume());

        match self.player.current() {
            Some(song) => {
                if audio.loaded() != Some(song.path()) {
                    let _ = audio.load(song.path(), self.player.position());
                } else if self.player.state() == PlaybackState::Stopped {
                    if audio.position() > Duration::from_millis(500) {
                        let _ = audio.seek(Duration::ZERO);
                    }
                } else {
                    // A position the device did not produce came from a seek.
                    let played = audio.position();
                    let wanted = self.player.position();
                    let drift = played.max(wanted) - played.min(wanted);

                    if drift > Duration::from_millis(750) {
                        let _ = audio.seek(wanted);
                    }
                }

                match self.player.state() {
                    PlaybackState::Playing => audio.resume(),
                    PlaybackState::Paused | PlaybackState::Stopped => audio.pause(),
                }
            }
            None => {
                if audio.loaded().is_some() {
                    let _ = audio.unload();
                }
            }
        }
    }

    /// A song matching `name`, looked for in the queue and history first, then across the library.
    fn find_song(&self, name: &str) -> Option<Song> {
        let wanted = name.trim().to_lowercase();

        let matches = |song: &Song| {
            song.title().is_some_and(|title| title.to_lowercase().contains(&wanted))
                || song
                    .path()
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.to_lowercase().contains(&wanted))
        };

        if let Some(song) = self
            .player
            .queue()
            .iter()
            .chain(self.player.history())
            .find(|song| matches(song))
        {
            return Some(song.clone());
        }

        library::scan(self.library_root.as_ref()?).into_iter().find(matches)
    }
}

/// The saved playlist whose name matches: exactly if one does, otherwise the first that contains it,
/// so a command line can be short.
fn find_playlist(name: &str) -> Option<Playlist> {
    let wanted = name.trim().to_lowercase();
    let playlists = Playlist::load_all();

    playlists
        .iter()
        .find(|playlist| playlist.name().to_lowercase() == wanted)
        .or_else(|| {
            playlists.iter().find(|playlist| playlist.name().to_lowercase().contains(&wanted))
        })
        .cloned()
}

/// `m:ss`, for the replies that mention a position.
fn format_time(position: Duration) -> String {
    let total = position.as_secs();

    format!("{}:{:02}", total / 60, total % 60)
}

/// Whether a daemon is listening.
pub fn is_running() -> bool {
    ipc::send(&Command::Status).is_ok()
}

/// Start a daemon and wait for it to answer.
///
/// Looks for `ogma-daemon` beside this executable first, so a build directory works without
/// installation, then on the path.
pub fn spawn() -> Result<(), String> {
    let exe = std::env::current_exe().ok();
    let directory = exe.as_ref().and_then(|exe| exe.parent());

    // Beside this executable first, then one directory up: a test or an example runs from a
    // subdirectory of the build output, where the daemon sits one level above.
    let mut nearby = directory.into_iter().flat_map(|dir| {
        [dir.join("ogma-daemon"), dir.join("..").join("ogma-daemon")]
    });

    let program = nearby
        .find(|path| path.exists())
        .unwrap_or_else(|| std::path::PathBuf::from("ogma-daemon"));

    std::process::Command::new(&program)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| format!("cannot start {}: {err}", program.display()))?;

    // Wait for it to be ready, so the first command does not arrive before the socket does.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if is_running() {
            return Ok(());
        }

        std::thread::sleep(Duration::from_millis(20));
    }

    Err("the daemon did not start answering".to_string())
}
