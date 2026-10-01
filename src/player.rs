//! Playback state: what is playing, what is queued, and what has played.
//!
//! This is the model only. Nothing here opens an audio device or decodes anything — the decode
//! thread and the output stream will drive this state rather than live inside it.

use std::time::Duration;

use rand::seq::SliceRandom;

use crate::config::Config;
use crate::playlist::Playlist;
use crate::song::Song;
use crate::volume;

/// What an interface can ask of whatever is playing.
///
/// The queue lives in the daemon, so the panes cannot simply change a [`Player`] and expect sound.
/// They call this instead: against a [`Player`] it changes that player, and against the interface's
/// remote controls it goes to the daemon. The panes then read the state back through a [`Player`]
/// that mirrors the daemon.
pub trait Controls {
    /// How many songs are queued, which is what a selection moves within.
    ///
    /// The panes need this to answer a key press, and cannot wait for the next frame to learn it.
    fn queue_len(&self) -> usize;

    fn play(&mut self);
    fn pause(&mut self);
    fn toggle_pause(&mut self);
    fn stop(&mut self);
    fn skip(&mut self);
    fn previous(&mut self);
    fn shuffle_queue(&mut self);
    /// Move to the next repeat mode, returning the one now in force.
    fn cycle_repeat(&mut self) -> Repeat;
    /// Repeat as `repeat` says, whatever the mode was before.
    fn set_repeat(&mut self, repeat: Repeat);
    fn change_volume(&mut self, change: f32);
    /// Move the position, forwards or back, without running past the start.
    fn seek(&mut self, change: std::time::Duration, forwards: bool);
    fn add_queue(&mut self, song: Song);
    fn add_queue_all(&mut self, songs: Vec<Song>);
    fn remove_queue(&mut self, index: usize);
    /// Empty the queue and the history, leaving a song that is playing alone.
    fn clear_queue(&mut self);
    fn play_now(&mut self, song: Song);
    /// Start the queue entry at `index`, sending what was playing to the history.
    fn play_queued(&mut self, index: usize);
    fn replace_queue_with_playlist(&mut self, playlist: &Playlist);
}

impl Controls for Player {
    fn queue_len(&self) -> usize {
        self.queue.len()
    }

    fn play(&mut self) {
        Player::play(self);
    }

    fn pause(&mut self) {
        Player::pause(self);
    }

    fn toggle_pause(&mut self) {
        Player::toggle_pause(self);
    }

    fn stop(&mut self) {
        Player::stop(self);
    }

    fn skip(&mut self) {
        Player::skip(self);
    }

    fn previous(&mut self) {
        Player::previous(self);
    }

    fn shuffle_queue(&mut self) {
        Player::shuffle_queue(self);
    }

    fn cycle_repeat(&mut self) -> Repeat {
        Player::cycle_repeat(self)
    }

    fn set_repeat(&mut self, repeat: Repeat) {
        Player::set_repeat(self, repeat);
    }

    fn change_volume(&mut self, change: f32) {
        Player::change_volume(self, change);
    }

    fn seek(&mut self, change: std::time::Duration, forwards: bool) {
        let position = self.position();

        let moved = if forwards { position + change } else { position.saturating_sub(change) };

        self.set_position(moved);
    }

    fn add_queue(&mut self, song: Song) {
        Player::add_queue(self, song);
    }

    fn add_queue_all(&mut self, songs: Vec<Song>) {
        Player::add_queue_all(self, songs);
    }

    fn remove_queue(&mut self, index: usize) {
        Player::remove_queue(self, index);
    }

    fn clear_queue(&mut self) {
        Player::clear_queue(self);
    }

    fn play_now(&mut self, song: Song) {
        Player::play_now(self, song);
    }

    fn play_queued(&mut self, index: usize) {
        if let Some(song) = Player::remove_queue(self, index) {
            Player::play_now(self, song);
        }
    }

    fn replace_queue_with_playlist(&mut self, playlist: &Playlist) {
        Player::replace_queue_with_playlist(self, playlist);
    }
}

/// Everything a player takes from elsewhere in one go, for [`Player::mirror`].
#[derive(Debug, Default, Clone)]
pub struct Mirrored {
    pub state: PlaybackState,
    pub position: Duration,
    pub volume: f32,
    /// The song playing now.
    pub current: Option<Song>,
    /// Songs waiting to play, next one first.
    pub queue: Vec<Song>,
    /// Songs that have already played, most recent last.
    pub history: Vec<Song>,
    pub repeat: Repeat,
}

/// Take the state of another player, as the interface does with what the daemon reports.
impl Player {
    /// Replace everything about this player with `state`.
    ///
    /// Used by the interface to mirror the daemon: the queue and what is playing belong to the
    /// daemon, and this is how they arrive.
    pub fn mirror(&mut self, state: Mirrored) {
        self.state = state.state;
        self.position = state.position;
        self.volume = state.volume;
        self.current = state.current;
        self.queue = state.queue;
        self.previous = state.history;
        self.repeat = state.repeat;
    }
}

/// Whether the player is producing sound.
///
/// Paused keeps the position, stopped resets it; both keep the queue.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum PlaybackState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// What happens when a song runs out.
///
/// Only the natural end of a song is affected. Pressing next always moves on, and pressing previous
/// always goes back: a repeat setting says what to do when nobody is asking for anything.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    /// Play the queue once and stop at its end.
    #[default]
    Off,
    /// Start the queue again once it has played out.
    Queue,
    /// Play the current song over until something says otherwise.
    Song,
}

impl Repeat {
    /// Every mode, in the order the interface cycles through them.
    pub const ALL: [Repeat; 3] = [Self::Off, Self::Queue, Self::Song];

    /// The word the protocol and the config carry.
    pub fn word(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Queue => "queue",
            Self::Song => "song",
        }
    }

    /// Read a mode from its word. `all` and `one` are taken from the players that use those names.
    pub fn parse(word: &str) -> Result<Self, String> {
        match word.trim().to_lowercase().as_str() {
            "off" | "none" => Ok(Self::Off),
            "queue" | "all" => Ok(Self::Queue),
            "song" | "one" | "track" => Ok(Self::Song),
            other => Err(format!("repeat takes off, queue or song, not {other:?}")),
        }
    }

    /// What the interface shows, when it is worth showing at all.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "repeat off",
            Self::Queue => "repeat queue",
            Self::Song => "repeat song",
        }
    }

    /// The next mode in the cycle.
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);

        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

/// What the player is playing, and what it will play next.
#[derive(Debug, Default, Clone)]
pub struct Player {
    /// Songs waiting to play, next one first.
    queue: Vec<Song>,
    /// The song playing now, if any.
    current: Option<Song>,
    /// How far into the current song playback has reached.
    position: Duration,
    /// Output level, 0.0 to 1.0.
    volume: f32,
    /// Songs that have already played, most recent last.
    previous: Vec<Song>,
    state: PlaybackState,
    /// What happens when a song runs out.
    repeat: Repeat,
}

impl Player {
    /// A player at half volume with nothing queued.
    pub fn new() -> Self {
        Player { volume: 0.5, ..Player::default() }
    }

    /// A player at the volume the config asks for.
    pub fn with_config(config: &Config) -> Self {
        Player { volume: *config.get_master_volume(), ..Player::default() }
    }

    // ------------------------------------------------------------------------------------ state

    /// The song playing now.
    pub fn current(&self) -> Option<&Song> {
        self.current.as_ref()
    }

    /// Songs waiting to play, next one first.
    pub fn queue(&self) -> &[Song] {
        &self.queue
    }

    /// Songs that have already played, most recent last.
    ///
    /// Named apart from [`Player::previous`], which is the skip-backwards command.
    pub fn history(&self) -> &[Song] {
        &self.previous
    }

    /// Whether the player is playing, paused or stopped.
    pub fn state(&self) -> PlaybackState {
        self.state
    }

    /// What happens when a song runs out.
    pub fn repeat(&self) -> Repeat {
        self.repeat
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    /// Move to the next repeat mode, returning the one now in force.
    pub fn cycle_repeat(&mut self) -> Repeat {
        self.repeat = self.repeat.next();

        self.repeat
    }

    pub fn is_playing(&self) -> bool {
        self.state == PlaybackState::Playing
    }

    pub fn is_paused(&self) -> bool {
        self.state == PlaybackState::Paused
    }

    /// How far into the current song playback has reached.
    pub fn position(&self) -> Duration {
        self.position
    }

    /// Fader position, 0.0 to 1.0.
    ///
    /// This is what the interface shows and what gets stored; the amplitude actually applied to the
    /// audio is [`Player::gain`].
    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// The amplitude the fader position corresponds to, through the loudness curve.
    pub fn gain(&self) -> f32 {
        volume::gain(self.volume)
    }

    /// Whether there is nothing playing and nothing queued.
    pub fn is_empty(&self) -> bool {
        self.current.is_none() && self.queue.is_empty()
    }

    /// How much of the current song is left, when its length is known.
    pub fn remaining(&self) -> Option<Duration> {
        let duration = self.current.as_ref()?.duration()?;

        Some(duration.saturating_sub(self.position))
    }

    /// How far through the current song playback is, 0.0 to 1.0, when its length is known.
    pub fn progress(&self) -> Option<f32> {
        let duration = self.current.as_ref()?.duration()?;

        if duration.is_zero() {
            return None;
        }

        Some((self.position.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0))
    }

    /// Combined length of the queue, when every song in it states a length.
    pub fn queue_duration(&self) -> Option<Duration> {
        self.queue.iter().try_fold(Duration::ZERO, |total, song| {
            Some(total + song.duration()?)
        })
    }

    // ----------------------------------------------------------------------------------- volume

    /// Set the fader position, clamped to 0.0 to 1.0.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    /// Move the fader by `change`, clamped to 0.0 to 1.0.
    pub fn change_volume(&mut self, change: f32) {
        self.set_volume(self.volume + change);
    }

    // --------------------------------------------------------------------------------- position

    /// Seek within the current song, clamped to its length where the length is known.
    pub fn set_position(&mut self, position: Duration) {
        self.position = match self.current.as_ref().and_then(Song::duration) {
            Some(duration) => position.min(duration),
            None => position,
        };
    }

    /// Advance the position, as the decoder does while it feeds the output.
    pub fn advance(&mut self, elapsed: Duration) {
        self.set_position(self.position + elapsed);
    }

    /// Whether playback has reached the end of the current song.
    pub fn at_end(&self) -> bool {
        match self.current.as_ref().and_then(Song::duration) {
            Some(duration) => self.position >= duration,
            None => false,
        }
    }

    // ------------------------------------------------------------------------------------ queue

    /// Add a song to the back of the queue.
    pub fn add_queue(&mut self, song: Song) {
        self.queue.push(song);
    }

    /// Add several songs to the back of the queue.
    pub fn add_queue_all(&mut self, songs: impl IntoIterator<Item = Song>) {
        self.queue.extend(songs);
    }

    /// Throw away the queue and put `songs` in its place.
    ///
    /// What is playing keeps playing: replacing what comes next is not the same as interrupting.
    pub fn replace_queue(&mut self, songs: impl IntoIterator<Item = Song>) {
        self.queue = songs.into_iter().collect();
    }

    /// Replace the queue with a playlist, in the playlist's own sort order.
    ///
    /// The order comes from [`Playlist::ordered_songs`], so the queue matches what the playlist shows
    /// rather than the order its tracks happened to be added in.
    pub fn replace_queue_with_playlist(&mut self, playlist: &Playlist) {
        self.replace_queue(playlist.ordered_songs());
    }

    /// Shuffle the queue. What is playing is left alone; only what comes next is rearranged.
    pub fn shuffle_queue(&mut self) {
        self.queue.shuffle(&mut rand::rng());
    }

    /// Put a song at the front of the queue, so it plays next.
    pub fn play_next(&mut self, song: Song) {
        self.queue.insert(0, song);
    }

    /// Start a song immediately, sending whatever was playing to the history.
    pub fn play_now(&mut self, song: Song) {
        if let Some(playing) = self.current.take() {
            self.previous.push(playing);
        }

        self.current = Some(song);
        self.position = Duration::ZERO;
        self.state = PlaybackState::Playing;
    }

    /// Take the song at `index` out of the queue, if there is one there.
    pub fn remove_queue(&mut self, index: usize) -> Option<Song> {
        (index < self.queue.len()).then(|| self.queue.remove(index))
    }

    /// Empty the queue and forget what has played.
    ///
    /// A song that is playing is left alone — clearing what comes next should not cut it off. With
    /// playback stopped or paused there is nothing to interrupt, so the current song goes too and
    /// the player is left empty.
    pub fn clear_queue(&mut self) {
        self.queue.clear();
        self.previous.clear();

        if self.state != PlaybackState::Playing {
            self.current = None;
            self.position = Duration::ZERO;
            self.state = PlaybackState::Stopped;
        }
    }

    /// Forget what has played.
    pub fn clear_history(&mut self) {
        self.previous.clear();
    }

    /// Start playing, or resume from a pause.
    ///
    /// With nothing playing, the front of the queue starts. Returns whether there is now something
    /// to play: `false` means both the current slot and the queue were empty.
    pub fn play(&mut self) -> bool {
        if self.current.is_none() {
            if self.queue.is_empty() {
                self.state = PlaybackState::Stopped;
                return false;
            }

            self.current = Some(self.queue.remove(0));
            self.position = Duration::ZERO;
        }

        self.state = PlaybackState::Playing;
        true
    }

    /// Hold playback where it is. The position is kept, so [`Player::play`] resumes from here.
    pub fn pause(&mut self) {
        if self.state == PlaybackState::Playing {
            self.state = PlaybackState::Paused;
        }
    }

    /// Pause if playing, play if not.
    pub fn toggle_pause(&mut self) {
        match self.state {
            PlaybackState::Playing => self.pause(),
            PlaybackState::Paused | PlaybackState::Stopped => {
                self.play();
            }
        }
    }

    /// Stop playback, rewinding the current song. The queue and the history are kept, so
    /// [`Player::play`] starts the same song again from the beginning.
    pub fn stop(&mut self) {
        self.state = PlaybackState::Stopped;
        self.position = Duration::ZERO;
    }

    /// Stop and forget everything: current song, queue and history.
    pub fn clear(&mut self) {
        self.queue.clear();
        self.previous.clear();
        self.current = None;
        self.position = Duration::ZERO;
        self.state = PlaybackState::Stopped;
    }

    // ------------------------------------------------------------------------------- navigation

    /// Move to the next song in the queue.
    ///
    /// The current song goes to the history. Returns the new current song, or `None` when the queue
    /// was empty — in which case playback stops.
    pub fn skip(&mut self) -> Option<&Song> {
        if let Some(playing) = self.current.take() {
            self.previous.push(playing);
        }

        // Taking from the front keeps the queue in playing order, which is what the UI shows. Music
        // queues are short enough that the shift costs nothing worth avoiding.
        self.current = (!self.queue.is_empty()).then(|| self.queue.remove(0));
        self.position = Duration::ZERO;

        // The end of the queue is where repeating it means something: what has played becomes what is
        // queued, in the order it played, and the first of them starts. The history empties as it goes
        // back into the queue, so going backwards still only reaches what played this time round.
        if self.current.is_none() && self.repeat == Repeat::Queue && !self.previous.is_empty() {
            self.queue = std::mem::take(&mut self.previous);
            self.current = Some(self.queue.remove(0));
        }

        if self.current.is_none() {
            // Nothing left to play.
            self.state = PlaybackState::Stopped;
        }

        self.current.as_ref()
    }

    /// What to do with a song that has run out on its own.
    ///
    /// Apart from [`Repeat::Song`], which plays it again, this is [`Player::skip`]: the difference
    /// between a song ending and someone pressing next is what a repeat setting is for.
    pub fn song_ended(&mut self) -> Option<&Song> {
        if self.repeat == Repeat::Song && self.current.is_some() {
            self.position = Duration::ZERO;
            self.state = PlaybackState::Playing;

            return self.current.as_ref();
        }

        self.skip()
    }

    /// Move back to the song that played before this one.
    ///
    /// The current song returns to the front of the queue. Returns the new current song, or `None`
    /// when there is no history, leaving the current song playing from its start.
    pub fn previous(&mut self) -> Option<&Song> {
        let Some(earlier) = self.previous.pop() else {
            // Nothing to go back to: restart what is playing, as a physical player would.
            self.position = Duration::ZERO;
            return None;
        };

        if let Some(playing) = self.current.take() {
            self.queue.insert(0, playing);
        }

        self.current = Some(earlier);
        self.position = Duration::ZERO;
        self.state = PlaybackState::Playing;

        self.current.as_ref()
    }

    /// Re-read the current song's file, after something has changed it on disk.
    pub fn refresh_current(&mut self) {
        if let Some(song) = &mut self.current {
            song.refresh();
        }
    }

    /// Restart the current song from its beginning.
    pub fn restart(&mut self) {
        self.position = Duration::ZERO;
    }
}
