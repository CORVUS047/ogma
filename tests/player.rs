//! Queue handling, navigation and playback state.
//!
//! These use songs pointing at paths that need not exist: the player deals in state, and `Song`
//! only touches the disk when metadata is asked for.

mod common;

use std::time::Duration;

use ogma::config::Config;
use ogma::player::{PlaybackState, Player};
use ogma::playlist::{Playlist, SortBy};
use ogma::song::Song;

fn song(name: &str) -> Song {
    Song::new(format!("/music/{name}.flac"))
}

#[test]
fn starts_empty_and_silent() {
    let player = Player::new();

    assert!(player.is_empty());
    assert!(player.current().is_none());
    assert!(player.queue().is_empty());
    assert!(player.history().is_empty());
    assert_eq!(player.position(), Duration::ZERO);
    assert_eq!(player.volume(), 0.5);
    assert_eq!(player.state(), PlaybackState::Stopped);
}

#[test]
fn takes_its_volume_from_the_config() {
    let mut config = Config::default();
    config.set_master_volume(0.2);

    assert_eq!(Player::with_config(&config).volume(), 0.2);
}

#[test]
fn add_queue_appends_and_remove_queue_takes_back_out() {
    let mut player = Player::new();

    player.add_queue(song("one"));
    player.add_queue(song("two"));
    player.add_queue_all([song("three"), song("four")]);

    assert_eq!(player.queue().len(), 4);
    assert_eq!(player.queue()[0], song("one"));

    assert_eq!(player.remove_queue(1), Some(song("two")));
    assert_eq!(player.queue().len(), 3);
    assert_eq!(player.queue()[1], song("three"));

    // Out of range is not an error, just nothing removed.
    assert_eq!(player.remove_queue(99), None);
    assert_eq!(player.queue().len(), 3);

    player.clear_queue();
    assert!(player.queue().is_empty());
}

#[test]
fn replace_queue_throws_away_what_was_queued() {
    let mut player = Player::new();
    player.add_queue_all([song("old-one"), song("old-two")]);
    player.play();
    let playing = player.current().cloned().expect("something playing");

    player.replace_queue([song("new-one"), song("new-two"), song("new-three")]);

    assert_eq!(player.queue(), [song("new-one"), song("new-two"), song("new-three")]);
    assert_eq!(player.current(), Some(&playing), "what is playing keeps playing");
}

#[test]
fn a_playlist_replaces_the_queue_in_its_own_sort_order() {
    let dir = common::scratch_dir("player-playlist");

    // Titles that sort differently from the order they are added in.
    let tracks: Vec<Song> = [("c.wav", "Charlie"), ("a.wav", "Alpha"), ("b.wav", "Bravo")]
        .iter()
        .map(|(name, title)| {
            Song::new(common::write_wav(
                &dir,
                &common::WavSpec { name, title: Some(title), ..common::WavSpec::default() },
            ))
        })
        .collect();

    let mut playlist = Playlist::with_songs("Mixed", tracks);
    let mut player = Player::new();

    // Manual: the order they were added.
    player.replace_queue_with_playlist(&playlist);
    let queued: Vec<String> = player.queue().iter().map(Song::display_title).collect();
    assert_eq!(queued, ["Charlie", "Alpha", "Bravo"]);

    // Sorted: the queue follows the sort, not the order of addition.
    playlist.set_sort(SortBy::Title);
    player.replace_queue_with_playlist(&playlist);
    let queued: Vec<String> = player.queue().iter().map(Song::display_title).collect();
    assert_eq!(queued, ["Alpha", "Bravo", "Charlie"]);

    // And descending reverses what plays, not only what is listed.
    playlist.toggle_direction();
    player.replace_queue_with_playlist(&playlist);
    let queued: Vec<String> = player.queue().iter().map(Song::display_title).collect();
    assert_eq!(queued, ["Charlie", "Bravo", "Alpha"]);
}

#[test]
fn shuffling_keeps_every_track_and_leaves_the_current_one_alone() {
    let mut player = Player::new();
    let tracks: Vec<Song> = (0..30).map(|index| song(&format!("track-{index:02}"))).collect();

    player.add_queue_all(tracks.clone());
    player.play();

    let playing = player.current().cloned().expect("something playing");
    let before = player.queue().to_vec();

    player.shuffle_queue();
    let after = player.queue().to_vec();

    assert_eq!(player.current(), Some(&playing), "the current track is not shuffled away");
    assert_eq!(after.len(), before.len());

    let mut sorted_before: Vec<_> = before.iter().map(|song| song.path().to_path_buf()).collect();
    let mut sorted_after: Vec<_> = after.iter().map(|song| song.path().to_path_buf()).collect();
    sorted_before.sort();
    sorted_after.sort();
    assert_eq!(sorted_before, sorted_after, "the same tracks, reordered");

    // With 29 tracks the chance of the order surviving untouched is negligible.
    assert_ne!(before, after);
}

#[test]
fn shuffling_an_empty_queue_is_harmless() {
    let mut player = Player::new();

    player.shuffle_queue();
    assert!(player.queue().is_empty());
}

#[test]
fn play_next_jumps_the_queue() {
    let mut player = Player::new();

    player.add_queue(song("later"));
    player.play_next(song("sooner"));

    assert_eq!(player.queue()[0], song("sooner"));
}

#[test]
fn play_starts_the_front_of_the_queue() {
    let mut player = Player::new();

    // Nothing to play yet.
    assert!(!player.play());
    assert_eq!(player.state(), PlaybackState::Stopped);

    player.add_queue(song("first"));
    player.add_queue(song("second"));

    assert!(player.play());
    assert_eq!(player.current(), Some(&song("first")));
    assert_eq!(player.queue().len(), 1, "the started song leaves the queue");
    assert!(player.is_playing());
}

#[test]
fn pause_keeps_the_position_and_play_resumes_from_it() {
    let mut player = Player::new();
    player.add_queue(song("track"));
    player.play();
    player.advance(Duration::from_secs(30));

    player.pause();
    assert!(player.is_paused());
    assert_eq!(player.position(), Duration::from_secs(30));

    assert!(player.play());
    assert!(player.is_playing());
    assert_eq!(player.position(), Duration::from_secs(30), "resuming does not rewind");
    assert_eq!(player.current(), Some(&song("track")), "and does not change song");
}

#[test]
fn toggle_pause_goes_both_ways() {
    let mut player = Player::new();
    player.add_queue(song("track"));

    player.toggle_pause();
    assert!(player.is_playing());

    player.toggle_pause();
    assert!(player.is_paused());

    player.toggle_pause();
    assert!(player.is_playing());
}

#[test]
fn stop_rewinds_but_keeps_the_queue() {
    let mut player = Player::new();
    player.add_queue(song("a"));
    player.add_queue(song("b"));
    player.play();
    player.advance(Duration::from_secs(10));

    player.stop();

    assert_eq!(player.state(), PlaybackState::Stopped);
    assert_eq!(player.position(), Duration::ZERO);
    assert_eq!(player.current(), Some(&song("a")), "stop does not drop the song");
    assert_eq!(player.queue().len(), 1, "nor the queue");

    // So play starts the same song over.
    player.play();
    assert_eq!(player.current(), Some(&song("a")));
}

#[test]
fn clear_forgets_everything() {
    let mut player = Player::new();
    player.add_queue(song("a"));
    player.add_queue(song("b"));
    player.play();
    player.skip();

    player.clear();

    assert!(player.is_empty());
    assert!(player.history().is_empty());
    assert!(player.current().is_none());
    assert_eq!(player.state(), PlaybackState::Stopped);
}

#[test]
fn skip_advances_and_files_the_played_song_in_the_history() {
    let mut player = Player::new();
    player.add_queue_all([song("a"), song("b"), song("c")]);
    player.play();
    player.advance(Duration::from_secs(5));

    assert_eq!(player.skip(), Some(&song("b")));
    assert_eq!(player.history(), [song("a")]);
    assert_eq!(player.position(), Duration::ZERO, "the new song starts at the beginning");

    assert_eq!(player.skip(), Some(&song("c")));
    assert_eq!(player.history(), [song("a"), song("b")]);

    // Past the end of the queue, playback stops.
    assert_eq!(player.skip(), None);
    assert!(player.current().is_none());
    assert_eq!(player.state(), PlaybackState::Stopped);
    assert_eq!(player.history(), [song("a"), song("b"), song("c")]);
}

#[test]
fn previous_walks_back_and_returns_the_song_to_the_queue() {
    let mut player = Player::new();
    player.add_queue_all([song("a"), song("b"), song("c")]);
    player.play();
    player.skip();
    player.skip();
    assert_eq!(player.current(), Some(&song("c")));

    assert_eq!(player.previous(), Some(&song("b")));
    assert_eq!(player.queue()[0], song("c"), "the song stepped over is queued again");
    assert_eq!(player.history(), [song("a")]);
    assert!(player.is_playing());

    assert_eq!(player.previous(), Some(&song("a")));
    assert_eq!(player.queue(), [song("b"), song("c")]);
    assert!(player.history().is_empty());
}

#[test]
fn previous_with_no_history_restarts_the_current_song() {
    let mut player = Player::new();
    player.add_queue(song("only"));
    player.play();
    player.advance(Duration::from_secs(42));

    assert_eq!(player.previous(), None);
    assert_eq!(player.current(), Some(&song("only")), "the song keeps playing");
    assert_eq!(player.position(), Duration::ZERO, "from its start");
}

#[test]
fn volume_is_clamped_both_ways() {
    let mut player = Player::new();

    player.set_volume(2.0);
    assert_eq!(player.volume(), 1.0);

    player.set_volume(-1.0);
    assert_eq!(player.volume(), 0.0);

    player.change_volume(0.3);
    assert_eq!(player.volume(), 0.3);

    player.change_volume(-5.0);
    assert_eq!(player.volume(), 0.0);
}

#[test]
fn position_is_capped_at_the_length_of_a_song_that_states_one() {
    let mut player = Player::new();
    // Half a second long, per the fixture.
    player.add_queue(Song::new(common::tagged_wav("player-position")));
    player.play();

    player.advance(Duration::from_secs(10));
    assert_eq!(player.position(), Duration::from_millis(500));
    assert!(player.at_end());
    assert_eq!(player.remaining(), Some(Duration::ZERO));
    assert_eq!(player.progress(), Some(1.0));

    player.set_position(Duration::from_millis(250));
    assert_eq!(player.progress(), Some(0.5));
    assert!(!player.at_end());

    player.restart();
    assert_eq!(player.position(), Duration::ZERO);
}

#[test]
fn a_song_of_unknown_length_never_reaches_an_end() {
    let mut player = Player::new();
    player.add_queue(song("unknown"));
    player.play();

    player.advance(Duration::from_secs(600));

    assert_eq!(player.position(), Duration::from_secs(600), "nothing to clamp against");
    assert!(!player.at_end());
    assert_eq!(player.remaining(), None);
    assert_eq!(player.progress(), None);
}

#[test]
fn queue_duration_needs_every_song_to_state_its_length() {
    let dir = common::scratch_dir("player-queue-duration");
    let mut player = Player::new();

    player.add_queue(Song::new(common::write_tagged_wav(&dir)));
    assert_eq!(player.queue_duration(), Some(Duration::from_millis(500)));

    player.add_queue(song("unknown"));
    assert_eq!(player.queue_duration(), None);
}
