//! Layout and interaction of the playback screen.

mod common;

use std::time::Duration;

use ogma::player::{PlaybackState, Player, Repeat};
use ogma::song::Song;
use ogma::ui::{PlayerScreen, PlayerScreenOutcome};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Color;

/// Songs that need not exist: the screen shows what it can and falls back for the rest.
fn song(name: &str) -> Song {
    Song::new(format!("/music/{name}.flac"))
}

fn frame_of(screen: &mut PlayerScreen, player: &Player) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("test terminal");

    terminal
        .draw(|f| screen.render(f, f.area(), player))
        .expect("draw player screen");

    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;

    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn press(
    screen: &mut PlayerScreen,
    player: &mut Player,
    code: KeyCode,
) -> Option<PlayerScreenOutcome> {
    screen.handle_key(KeyEvent::from(code), player)
}

/// Move the focus onto the queue, which is one pane along from where the screen opens.
fn focus_queue(screen: &mut PlayerScreen, player: &mut Player) {
    press(screen, player, KeyCode::Tab);
}

/// A screen that reads nothing from disk: no playlists, and a file listing rooted somewhere empty.
///
/// Tests must not depend on what the machine running them happens to have saved.
fn isolated_screen() -> PlayerScreen {
    PlayerScreen::with_playlists(&common::scratch_dir("screen-empty"), Vec::new())
}

/// A player playing the first of several songs.
fn playing() -> Player {
    let mut player = Player::new();
    player.add_queue_all([song("first"), song("second"), song("third"), song("fourth")]);
    player.play();

    player
}

#[test]
fn lays_out_three_columns_with_the_song_in_the_middle() {
    let player = playing();
    let mut screen = isolated_screen();

    let frame = frame_of(&mut screen, &player);
    let top = frame.lines().next().expect("a first row");

    // Playlists left, song middle, queue right, in that order across the top border.
    let playlists = top.find("Playlists").expect("playlists pane");
    let now = top.find("Now Playing").expect("now playing pane");
    let queue = top.find("Queue").expect("queue pane");

    assert!(playlists < now && now < queue, "{top}");

    // The left column now holds two panes: playlists above, the file listing below.
    let files = top.find("Files").or_else(|| {
        frame.lines().find_map(|row| row.find("Files"))
    });
    assert!(files.is_some(), "the file listing is in the left column: {frame}");
    assert!(frame.contains("No playlists yet"), "with no playlists saved yet");
}

#[test]
fn shows_the_song_its_progress_and_the_transport_state() {
    let mut player = playing();
    let mut screen = isolated_screen();

    let frame = frame_of(&mut screen, &player);

    assert!(frame.contains("first"), "the title falls back to the file name");
    assert!(frame.contains("Unknown Artist"));
    assert!(frame.contains("0:00"), "the clock is shown");
    assert!(frame.contains("playing"));
    // In this pane width the decibel reading takes the place of the percentage.
    assert!(frame.contains("-10.0 dB"), "the level is shown in decibels: {frame}");

    player.pause();
    assert!(frame_of(&mut screen, &player).contains("paused"));

    player.stop();
    assert!(frame_of(&mut screen, &player).contains("stopped"));
}

#[test]
fn the_progress_bar_tracks_the_position() {
    let dir = common::scratch_dir("screen-progress");
    let mut player = Player::new();
    // The fixture is half a second long.
    player.add_queue(Song::new(common::write_tagged_wav(&dir)));
    player.play();

    let mut screen = isolated_screen();
    let empty = frame_of(&mut screen, &player).matches('█').count();

    player.advance(Duration::from_millis(250));
    let half = frame_of(&mut screen, &player).matches('█').count();

    player.advance(Duration::from_millis(250));
    let full = frame_of(&mut screen, &player).matches('█').count();

    assert!(half > empty && full > half, "{empty} then {half} then {full}");
}

#[test]
fn nothing_playing_says_so() {
    let player = Player::new();
    let mut screen = isolated_screen();

    let frame = frame_of(&mut screen, &player);

    assert!(frame.contains("Nothing playing"));
    assert!(frame.contains("queue empty"));
}

#[test]
fn the_queue_shows_the_current_song_above_the_upcoming_ones() {
    let mut player = playing();
    let mut screen = isolated_screen();

    let rows: Vec<String> = frame_of(&mut screen, &player).lines().map(str::to_owned).collect();

    // The now-playing marker sits above the first upcoming song.
    let current = rows.iter().position(|row| row.contains('▶')).expect("current row");
    let upcoming = rows
        .iter()
        .position(|row| row.contains("second"))
        .expect("the next song is listed");

    assert!(current < upcoming, "current at {current}, next at {upcoming}");

    // Everything queued is listed.
    let frame = rows.join("\n");
    for name in ["second", "third", "fourth"] {
        assert!(frame.contains(name), "{name} missing from the queue");
    }

    player.skip();
    assert!(
        frame_of(&mut screen, &player).contains("second"),
        "the new current song is shown"
    );
}

#[test]
fn up_to_three_played_songs_are_listed_above_the_current_one() {
    let mut player = Player::new();
    player.add_queue_all([
        song("a"),
        song("b"),
        song("c"),
        song("d"),
        song("e"),
        song("f"),
    ]);
    player.play();

    let mut screen = isolated_screen();

    // Four songs played, so the oldest must have scrolled off.
    for _ in 0..4 {
        player.skip();
    }
    assert_eq!(player.current(), Some(&song("e")));

    let rows: Vec<String> = frame_of(&mut screen, &player).lines().map(str::to_owned).collect();
    let queue_column: Vec<String> = rows
        .iter()
        .map(|row| row.chars().skip(68).collect::<String>())
        .collect();
    let column = queue_column.join("\n");

    for recent in ["b", "c", "d"] {
        assert!(column.contains(&format!("— {recent}")), "{recent} should be listed: {column}");
    }
    assert!(!column.contains("— a"), "the fourth-oldest is dropped: {column}");

    // The current song keeps its row whether or not three songs have played.
    let current_row = rows.iter().position(|row| row.contains('▶')).expect("current row");
    let mut fresh = playing();
    fresh.clear();
    fresh.add_queue(song("only"));
    fresh.play();
    let fresh_row = frame_of(&mut isolated_screen(), &fresh)
        .lines()
        .position(|row| row.contains('▶'))
        .expect("current row");

    assert_eq!(current_row, fresh_row, "the current song does not move about");
}

#[test]
fn transport_keys_drive_the_player() {
    let mut player = playing();
    let mut screen = isolated_screen();

    press(&mut screen, &mut player, KeyCode::Char(' '));
    assert_eq!(player.state(), PlaybackState::Paused);
    press(&mut screen, &mut player, KeyCode::Char(' '));
    assert_eq!(player.state(), PlaybackState::Playing);

    press(&mut screen, &mut player, KeyCode::Char('n'));
    assert_eq!(player.current(), Some(&song("second")));

    press(&mut screen, &mut player, KeyCode::Char('p'));
    assert_eq!(player.current(), Some(&song("first")));

    press(&mut screen, &mut player, KeyCode::Char('S'));
    assert_eq!(player.state(), PlaybackState::Stopped);
}

#[test]
fn seek_keys_move_the_position_without_running_past_the_start() {
    let mut player = playing();
    let mut screen = isolated_screen();

    press(&mut screen, &mut player, KeyCode::Right);
    press(&mut screen, &mut player, KeyCode::Right);
    assert_eq!(player.position(), Duration::from_secs(10));

    press(&mut screen, &mut player, KeyCode::Left);
    assert_eq!(player.position(), Duration::from_secs(5));

    // Seeking back past the beginning stops at zero.
    press(&mut screen, &mut player, KeyCode::Left);
    press(&mut screen, &mut player, KeyCode::Left);
    assert_eq!(player.position(), Duration::ZERO);
}

#[test]
fn volume_keys_move_the_level() {
    let mut player = playing();
    let mut screen = isolated_screen();

    press(&mut screen, &mut player, KeyCode::Char('+'));
    assert_eq!(player.volume(), 0.55);

    press(&mut screen, &mut player, KeyCode::Char('-'));
    press(&mut screen, &mut player, KeyCode::Char('-'));
    assert_eq!(player.volume(), 0.45);
}

#[test]
fn the_level_is_reported_through_the_loudness_curve() {
    let mut player = playing();
    let mut screen = isolated_screen();

    // The fader is where the keys put it; the gain is what the ear gets.
    assert_eq!(player.volume(), 0.5);
    assert!((player.gain() - 0.3162).abs() < 0.001, "got {}", player.gain());
    assert!(frame_of(&mut screen, &player).contains("-10.0 dB"));

    for _ in 0..10 {
        press(&mut screen, &mut player, KeyCode::Char('+'));
    }
    assert_eq!(player.volume(), 1.0);
    assert_eq!(player.gain(), 1.0, "a fader at the top does not attenuate");
    assert!(frame_of(&mut screen, &player).contains("0.0 dB"));

    for _ in 0..20 {
        press(&mut screen, &mut player, KeyCode::Char('-'));
    }
    assert_eq!(player.gain(), 0.0, "the bottom of the fader is silence");
    assert!(frame_of(&mut screen, &player).contains("muted"));
}

#[test]
fn a_wide_pane_shows_both_the_fader_and_the_level() {
    let player = playing();
    let mut screen = isolated_screen();

    let mut terminal = Terminal::new(TestBackend::new(140, 24)).expect("test terminal");
    terminal
        .draw(|f| screen.render(f, f.area(), &player))
        .expect("draw wide");

    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    let frame: String = buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(frame.contains("50%"), "room for the percentage too: {frame}");
    assert!(frame.contains("-10.0 dB"));
}

#[test]
fn enter_plays_the_highlighted_queue_entry() {
    let mut player = playing();
    let mut screen = isolated_screen();
    focus_queue(&mut screen, &mut player);

    // Second entry in the queue: "third".
    press(&mut screen, &mut player, KeyCode::Down);
    press(&mut screen, &mut player, KeyCode::Enter);

    assert_eq!(player.current(), Some(&song("third")));
    assert_eq!(player.history(), [song("first")], "what was playing is filed away");
    assert_eq!(player.queue(), [song("second"), song("fourth")]);
}

#[test]
fn x_removes_the_highlighted_queue_entry() {
    let mut player = playing();
    let mut screen = isolated_screen();
    focus_queue(&mut screen, &mut player);

    press(&mut screen, &mut player, KeyCode::Char('x'));
    assert_eq!(player.queue(), [song("third"), song("fourth")]);

    // Removing the last entry pulls the selection back inside the queue.
    press(&mut screen, &mut player, KeyCode::Char('G'));
    press(&mut screen, &mut player, KeyCode::Char('x'));
    press(&mut screen, &mut player, KeyCode::Char('x'));
    assert!(player.queue().is_empty());

    // And an empty queue takes no more removing.
    press(&mut screen, &mut player, KeyCode::Char('x'));
    assert!(player.queue().is_empty());
}

#[test]
fn capital_x_empties_the_queue_without_stopping_the_music() {
    let mut player = playing();
    let mut screen = isolated_screen();
    focus_queue(&mut screen, &mut player);

    press(&mut screen, &mut player, KeyCode::Char('X'));

    assert!(player.queue().is_empty(), "the lot, not just the highlighted row");
    assert_eq!(player.current(), Some(&song("first")), "what is playing is not in the queue");
    assert!(player.is_playing(), "so clearing the queue does not stop it");

    // The key is listed with the queue's own, and an empty queue takes no more clearing.
    let frame = frame_of(&mut screen, &player);
    assert!(frame.contains("X clear"), "{frame}");

    press(&mut screen, &mut player, KeyCode::Char('X'));
    assert!(player.queue().is_empty());
}

#[test]
fn queue_keys_do_nothing_while_another_pane_has_focus() {
    let mut player = playing();
    let mut screen = isolated_screen();

    // The playlists pane claims nothing, so its keys fall through harmlessly.
    press(&mut screen, &mut player, KeyCode::Char('h'));
    press(&mut screen, &mut player, KeyCode::Char('x'));
    press(&mut screen, &mut player, KeyCode::Char('X'));

    assert_eq!(player.queue().len(), 3, "the queue is left alone");
    assert_eq!(player.current(), Some(&song("first")));

    // Tab moves on to the files, then to the queue, where the keys apply again.
    press(&mut screen, &mut player, KeyCode::Tab);
    press(&mut screen, &mut player, KeyCode::Tab);
    press(&mut screen, &mut player, KeyCode::Char('x'));
    assert_eq!(player.queue().len(), 2);
}

#[test]
fn the_key_reminders_can_be_turned_off() {
    let player = playing();
    let mut screen = isolated_screen();

    // Shown to begin with: the transport's keys, and those of whichever pane has the focus. The
    // screen opens on the files.
    let mut player = player;
    let frame = frame_of(&mut screen, &player);
    assert!(frame.contains("space"), "the transport keys: {frame}");
    assert!(frame.contains("a queue"), "the focused pane's keys");
    assert!(!frame.contains("enter load"), "an unfocused pane keeps quiet");

    press(&mut screen, &mut player, KeyCode::BackTab);
    assert!(
        frame_of(&mut screen, &player).contains("enter load"),
        "the playlists' keys once it has the focus"
    );

    screen.set_hints(false);

    let frame = frame_of(&mut screen, &player);
    assert!(!frame.contains("space"), "nothing left of the transport keys: {frame}");
    assert!(!frame.contains("enter load"));

    press(&mut screen, &mut player, KeyCode::Tab);
    assert!(!frame_of(&mut screen, &player).contains("a queue"), "nor the file listing's");

    // What is playing is not a hint, and stays.
    assert!(frame.contains("playing"));
    assert!(frame.contains("first"));

    screen.set_hints(true);
    assert!(frame_of(&mut screen, &player).contains("space"), "and back again");
}

#[test]
fn the_volume_is_on_screen_along_with_the_keys_that_move_it() {
    /// Wide enough for the fader's full reading, which a narrow pane gives up before the bar.
    fn wide_frame_of(screen: &mut PlayerScreen, player: &Player) -> String {
        let mut terminal = Terminal::new(TestBackend::new(140, 24)).expect("test terminal");

        terminal.draw(|f| screen.render(f, f.area(), player)).expect("draw player screen");

        let buffer = terminal.backend().buffer();
        let width = buffer.area.width as usize;

        buffer
            .content()
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    let mut screen = isolated_screen();

    // Nothing playing is no reason to hide the fader: it is what the next song comes out at, and
    // guessing where it sits is what having it on screen avoids.
    let frame = wide_frame_of(&mut screen, &Player::new());
    assert!(frame.contains("Nothing playing"));
    assert!(frame.contains("vol"), "the fader is labelled: {frame}");
    assert!(frame.contains("50%"), "and reads where it sits: {frame}");
    assert!(frame.contains("+/-"), "with the keys that move it named: {frame}");

    // And with something playing, where the progress bar shares the space.
    let player = playing();
    let frame = wide_frame_of(&mut screen, &player);
    assert!(frame.contains("50%"), "{frame}");
    assert!(frame.contains("+/-"), "{frame}");

    // Hidden hints take the reminder, not the reading: the level is state, not a hint.
    screen.set_hints(false);
    let frame = wide_frame_of(&mut screen, &player);
    assert!(!frame.contains("+/-"), "{frame}");
    assert!(frame.contains("50%"), "{frame}");
}

#[test]
fn the_queue_only_says_it_is_empty_when_there_is_nothing_at_all() {
    let mut screen = isolated_screen();

    // Nothing playing, nothing played, nothing queued.
    let empty = Player::new();
    assert!(frame_of(&mut screen, &empty).contains("queue empty"));

    // Something playing and nothing after it: the row above already says what is going on, so the
    // message would read as a contradiction.
    let mut player = Player::new();
    player.add_queue(song("only"));
    player.play();

    let frame = frame_of(&mut screen, &player);
    assert!(player.queue().is_empty(), "nothing is coming up");
    assert!(!frame.contains("queue empty"), "{frame}");
    assert!(frame.contains("only"), "what is playing is shown instead");

    // Played out: the history is what fills the pane now.
    player.skip();
    assert!(player.current().is_none());
    assert!(!player.history().is_empty());

    let frame = frame_of(&mut screen, &player);
    assert!(!frame.contains("queue empty"), "{frame}");
    assert!(frame.contains("only"), "the played track is still listed");
}

#[test]
fn the_colours_come_from_the_theme() {
    use ogma::theme::Theme;
    use ratatui::style::Color;

    let player = playing();
    let mut screen = isolated_screen();

    // A theme of colours nothing else in the interface uses, so their presence means they were used.
    screen.set_theme(Theme {
        accent: Color::Rgb(1, 2, 3),
        border_focused: Color::Rgb(4, 5, 6),
        playing: Color::Rgb(7, 8, 9),
        ..Theme::default()
    });

    let mut terminal = Terminal::new(TestBackend::new(110, 24)).expect("test terminal");
    terminal
        .draw(|frame| screen.render(frame, frame.area(), &player))
        .expect("draw themed");

    let buffer = terminal.backend().buffer();
    let used: std::collections::HashSet<Color> =
        buffer.content().iter().map(|cell| cell.fg).collect();

    assert!(used.contains(&Color::Rgb(1, 2, 3)), "the accent is drawn with");
    assert!(used.contains(&Color::Rgb(4, 5, 6)), "so is the focused border");
    assert!(used.contains(&Color::Rgb(7, 8, 9)), "and the now-playing marker");

    // The default theme leaves those colours out entirely.
    screen.set_theme(Theme::default());
    let mut terminal = Terminal::new(TestBackend::new(110, 24)).expect("test terminal");
    terminal
        .draw(|frame| screen.render(frame, frame.area(), &player))
        .expect("draw plain");

    let plain: std::collections::HashSet<Color> =
        terminal.backend().buffer().content().iter().map(|cell| cell.fg).collect();

    assert!(!plain.contains(&Color::Rgb(1, 2, 3)));
    assert!(plain.contains(&Color::Cyan), "the terminal's palette instead");
}

#[test]
fn tab_changes_pane_even_while_a_search_is_being_typed() {
    let mut player = playing();
    let mut screen = isolated_screen();

    // The screen opens on the files; start a search there.
    press(&mut screen, &mut player, KeyCode::Char('/'));
    press(&mut screen, &mut player, KeyCode::Char('x'));
    assert!(frame_of(&mut screen, &player).contains("/x"), "typing into the file search");

    // Tab moves on regardless, and the pane it moved to answers its own keys.
    press(&mut screen, &mut player, KeyCode::Tab);
    press(&mut screen, &mut player, KeyCode::Char('x'));

    let frame = frame_of(&mut screen, &player);
    assert!(frame.contains("/x"), "the file search kept what was typed: {frame}");
    assert_eq!(player.queue().len(), 2, "and `x` removed a queue entry in the pane tab reached");
}

#[test]
fn a_letter_typed_into_a_search_is_not_a_command() {
    let mut player = playing();
    let mut screen = isolated_screen();

    press(&mut screen, &mut player, KeyCode::Char('/'));

    // `q` closes the screen while browsing; while typing it is just a letter.
    assert_eq!(press(&mut screen, &mut player, KeyCode::Char('q')), None);
    assert!(frame_of(&mut screen, &player).contains("/q"), "it went into the query");

    // Space likewise: it is part of the query, not play/pause.
    let state = player.state();
    press(&mut screen, &mut player, KeyCode::Char(' '));
    assert_eq!(player.state(), state, "playback was not touched");

    // Escape ends the search, and only then does it close the screen.
    assert_eq!(press(&mut screen, &mut player, KeyCode::Esc), None);
    assert_eq!(
        press(&mut screen, &mut player, KeyCode::Esc),
        Some(PlayerScreenOutcome::Close)
    );
}

#[test]
fn esc_closes_the_screen() {
    let mut player = playing();
    let mut screen = isolated_screen();

    assert_eq!(press(&mut screen, &mut player, KeyCode::Char('j')), None);
    assert_eq!(
        press(&mut screen, &mut player, KeyCode::Esc),
        Some(PlayerScreenOutcome::Close)
    );
}

#[test]
fn embedded_artwork_is_drawn_in_colour() {
    let dir = common::scratch_dir("screen-artwork");
    let Some(path) = common::wav_with_cover(&dir) else {
        // The fixture could not attach a picture; nothing to assert about drawing one.
        return;
    };

    let with_art = Song::new(&path);
    assert!(with_art.front_cover().is_some(), "the fixture has a cover");

    let mut player = Player::new();
    player.add_queue(with_art);
    player.play();

    let mut screen = isolated_screen();
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("test terminal");
    terminal
        .draw(|f| screen.render(f, f.area(), &player))
        .expect("draw with artwork");

    let buffer = terminal.backend().buffer();

    // Half blocks, each carrying the colour of two pixels.
    let blocks = buffer.content().iter().filter(|cell| cell.symbol() == "▀").count();
    assert!(blocks > 100, "the cover should fill many cells, got {blocks}");

    let coloured = buffer
        .content()
        .iter()
        .filter(|cell| matches!(cell.fg, Color::Rgb(..)) && matches!(cell.bg, Color::Rgb(..)))
        .count();
    assert_eq!(coloured, blocks, "every art cell carries a colour pair");

    // A song without a cover falls back to the placeholder instead.
    let mut plain = Player::new();
    plain.add_queue(song("no-art"));
    plain.play();
    assert!(frame_of(&mut isolated_screen(), &plain).contains("no artwork"));
}

#[test]
fn the_repeat_key_cycles_the_mode_and_the_queue_pane_says_which() {
    let mut screen = isolated_screen();
    let mut player = playing();

    // Off is the ordinary state, and the pane says nothing about it.
    assert!(!frame_of(&mut screen, &player).contains("repeat queue"));

    press(&mut screen, &mut player, KeyCode::Char('R'));
    assert_eq!(player.repeat(), Repeat::Queue);
    let frame = frame_of(&mut screen, &player);
    assert!(frame.contains("repeat queue"), "the queue pane says what it will do: {frame}");

    press(&mut screen, &mut player, KeyCode::Char('R'));
    assert_eq!(player.repeat(), Repeat::Song);
    assert!(frame_of(&mut screen, &player).contains("repeat song"));

    press(&mut screen, &mut player, KeyCode::Char('R'));
    assert_eq!(player.repeat(), Repeat::Off);
    let frame = frame_of(&mut screen, &player);
    assert!(!frame.contains("repeat queue") && !frame.contains("repeat song"), "{frame}");
    assert!(frame.contains("Queue"), "the pane is still the queue: {frame}");
}

#[test]
fn the_queue_pane_lists_its_own_keys_once_it_has_the_focus() {
    let mut screen = isolated_screen();
    let mut player = playing();

    // The keys act on what is highlighted, so they are named where the highlight means something.
    assert!(!frame_of(&mut screen, &player).contains("enter play"));

    focus_queue(&mut screen, &mut player);
    let frame = frame_of(&mut screen, &player);
    assert!(frame.contains("enter play"), "how to play a queued song: {frame}");
    assert!(frame.contains("x remove"), "and how to take one out: {frame}");

    screen.set_hints(false);
    assert!(!frame_of(&mut screen, &player).contains("enter play"), "hidden with the rest");
}
