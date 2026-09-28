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
    fn change_volume(&mut self, change: f32);
    /// Move the position, forwards or back, without running past the start.
    fn seek(&mut self, change: std::time::Duration, forwards: bool);
    fn add_queue(&mut self, song: Song);
    fn add_queue_all(&mut self, songs: Vec<Song>);
    fn remove_queue(&mut self, index: usize);
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

/// Take the state of another player, as the interface does with what the daemon reports.
impl Player {
    /// Replace everything about this player with `state`.
    ///
    /// Used by the interface to mirror the daemon: the queue and what is playing belong to the
    /// daemon, and this is how they arrive.
    pub fn mirror(
        &mut self,
        state: PlaybackState,
        position: std::time::Duration,
        volume: f32,
        current: Option<Song>,
        queue: Vec<Song>,
        history: Vec<Song>,
    ) {
        self.state = state;
        self.position = position;
        self.volume = volume;
        self.current = current;
        self.queue = queue;
        self.previous = history;
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

    /// Empty the queue, leaving the current song alone.
    pub fn clear_queue(&mut self) {
        self.queue.clear();
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

        if self.current.is_none() {
            // Nothing left to play.
            self.state = PlaybackState::Stopped;
        }

        self.current.as_ref()
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
