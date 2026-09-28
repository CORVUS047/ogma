//! The playlists pane, and the parts of the player screen that reach into it.
//!
//! Everything here redirects the config location first, so the user's own playlists are never
//! touched. All tests in this binary point it at the same directory, so they do not fight over it.

mod common;

use std::path::PathBuf;

use common::WavSpec;
use ogma::player::Player;
use ogma::playlist::{Playlist, SortBy};
use ogma::song::Song;
use ogma::ui::{PlayerScreen, PlaylistAction, PlaylistPane};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

/// Point the config, and so the playlists directory, at a scratch tree.
fn redirect() -> PathBuf {
    let root = std::env::temp_dir().join("ogma-playlist-pane");

    // SAFETY: every test in this binary sets the same value, so there is nothing to race over.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &root) };

    Playlist::directory().expect("a playlists directory")
}

/// Three tracks with distinct titles, for checking order.
fn songs(name: &str) -> Vec<Song> {
    let dir = common::scratch_dir(name);

    [("c.wav", "Charlie"), ("a.wav", "Alpha"), ("b.wav", "Bravo")]
        .iter()
        .map(|(file, title)| {
            Song::new(common::write_wav(
                &dir,
                &WavSpec { name: file, title: Some(title), ..WavSpec::default() },
            ))
        })
        .collect()
}

fn render(pane: &mut PlaylistPane, focused: bool) -> String {
    render_at(pane, focused, 30)
}

/// The pane at a given width. The player screen gives it 33 cells, which is what its hints are
/// written to fit.
fn render_at(pane: &mut PlaylistPane, focused: bool, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 10)).expect("test terminal");

    terminal
        .draw(|frame| pane.render(frame, frame.area(), focused))
        .expect("draw pane");

    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;

    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether the pane claimed the key.
fn press(pane: &mut PlaylistPane, player: &mut Player, code: KeyCode) -> bool {
    pane.handle_key(KeyEvent::from(code), player).is_some()
}

/// The action the pane asked for, if any.
fn press_for(
    pane: &mut PlaylistPane,
    player: &mut Player,
    code: KeyCode,
) -> Option<PlaylistAction> {
    pane.handle_key(KeyEvent::from(code), player)
}

fn type_text(pane: &mut PlaylistPane, player: &mut Player, text: &str) {
    for c in text.chars() {
        press(pane, player, KeyCode::Char(c));
    }
}

fn titles(player: &Player) -> Vec<String> {
    player.queue().iter().map(Song::display_title).collect()
}

#[test]
fn an_empty_pane_says_so() {
    let mut pane = PlaylistPane::with_playlists(Vec::new());

    assert!(render(&mut pane, true).contains("No playlists yet"));
    assert!(pane.selected().is_none());
}

#[test]
fn a_playlist_is_listed_with_its_length_and_sort() {
    let mut playlist = Playlist::with_songs("Late Night", songs("pane-listing"));
    playlist.set_sort(SortBy::Title);

    let mut pane = PlaylistPane::with_playlists(vec![playlist]);
    let frame = render(&mut pane, true);

    assert!(frame.contains("Late Night"), "{frame}");
    assert!(frame.contains('3'), "its length");
    assert!(frame.contains("title"), "and how it is sorted");
}

#[test]
fn enter_replaces_the_queue_in_the_playlists_own_order() {
    let mut playlist = Playlist::with_songs("Late Night", songs("pane-queue"));
    playlist.set_sort(SortBy::Title);

    let mut pane = PlaylistPane::with_playlists(vec![playlist]);
    let mut player = Player::new();

    // Something already queued, to be replaced rather than added to.
    player.add_queue(Song::new("/music/leftover.flac"));

    assert!(press(&mut pane, &mut player, KeyCode::Enter));

    // Sorted by title, not in the order the tracks were added: the sort is not just a view.
    assert_eq!(titles(&player), ["Alpha", "Bravo", "Charlie"]);
    assert!(render(&mut pane, true).contains("queued 3"));
}

#[test]
fn the_queue_follows_the_sort_the_playlist_is_set_to() {
    let mut pane = PlaylistPane::with_playlists(vec![Playlist::with_songs(
        "Mixed",
        songs("pane-sort-queue"),
    )]);
    let mut player = Player::new();
    redirect();

    // Manual to begin with: the order they were added.
    press(&mut pane, &mut player, KeyCode::Enter);
    assert_eq!(titles(&player), ["Charlie", "Alpha", "Bravo"]);

    // `s` walks the sorts; stop at title.
    while pane.selected().expect("a playlist").sort() != SortBy::Title {
        press(&mut pane, &mut player, KeyCode::Char('s'));
    }

    press(&mut pane, &mut player, KeyCode::Enter);
    assert_eq!(titles(&player), ["Alpha", "Bravo", "Charlie"]);

    // And reversing it reverses what reaches the queue too.
    press(&mut pane, &mut player, KeyCode::Char('r'));
    press(&mut pane, &mut player, KeyCode::Enter);
    assert_eq!(titles(&player), ["Charlie", "Bravo", "Alpha"]);
}

#[test]
fn the_sort_is_saved_with_the_playlist() {
    let directory = redirect();

    let playlist = Playlist::with_songs("Keeps Its Sort", songs("pane-sort-saved"));
    playlist.save().expect("save");

    let mut pane = PlaylistPane::with_playlists(vec![playlist]);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('s'));
    press(&mut pane, &mut player, KeyCode::Char('r'));

    let expected = pane.selected().expect("a playlist").sort();

    // Read from disk, not from the pane: the setting has to have been written.
    let reloaded = Playlist::load_from(&directory.join("keeps_its_sort.toml")).expect("reload");

    assert_eq!(reloaded.sort(), expected);
    assert!(reloaded.descending());
}

#[test]
fn c_saves_the_queue_as_a_new_playlist() {
    let directory = redirect();

    let tracks = songs("pane-create");
    let mut player = Player::new();
    player.add_queue_all(tracks.clone());
    player.play();

    let mut pane = PlaylistPane::with_playlists(Vec::new());

    press(&mut pane, &mut player, KeyCode::Char('c'));
    // The prompt shows what is being typed.
    type_text(&mut pane, &mut player, "Late Night");
    assert!(render(&mut pane, true).contains("Late Night"));

    press(&mut pane, &mut player, KeyCode::Enter);

    // What was playing belongs in the playlist too, at the front.
    let saved = Playlist::load_from(&directory.join("late_night.toml")).expect("load saved");
    assert_eq!(saved.name(), "Late Night");
    assert_eq!(saved.len(), 3);
    assert_eq!(saved.as_added()[0], tracks[0], "the playing track leads");

    assert_eq!(pane.playlists().len(), 1, "and it appears in the pane");
    assert!(render(&mut pane, true).contains("saved Late Night"));
}

#[test]
fn naming_a_playlist_can_be_corrected_or_abandoned() {
    redirect();
    let mut player = Player::new();
    let mut pane = PlaylistPane::with_playlists(Vec::new());

    press(&mut pane, &mut player, KeyCode::Char('c'));
    type_text(&mut pane, &mut player, "Wrng");
    press(&mut pane, &mut player, KeyCode::Backspace);
    press(&mut pane, &mut player, KeyCode::Esc);

    assert!(pane.playlists().is_empty(), "esc abandons it");
    assert!(render(&mut pane, true).contains("cancelled"));

    // A name with nothing usable in it is refused, since it has no file name.
    press(&mut pane, &mut player, KeyCode::Char('c'));
    type_text(&mut pane, &mut player, "!!!");
    press(&mut pane, &mut player, KeyCode::Enter);

    assert!(pane.playlists().is_empty());
    assert!(render(&mut pane, true).contains("name needs letters"));
}

#[test]
fn d_twice_deletes_the_playlist_and_once_does_not() {
    let directory = redirect();

    let playlist = Playlist::with_songs("Doomed", songs("pane-delete"));
    playlist.save().expect("save");
    let path = directory.join("doomed.toml");
    assert!(path.exists());

    let mut pane = PlaylistPane::with_playlists(vec![playlist]);
    let mut player = Player::new();

    // One press only asks.
    press(&mut pane, &mut player, KeyCode::Char('d'));
    assert!(render(&mut pane, true).contains("press d again"));
    assert!(path.exists(), "nothing is deleted yet");

    // Anything else calls it off.
    press(&mut pane, &mut player, KeyCode::Char('j'));
    assert!(render(&mut pane, true).contains("cancelled"));
    assert!(path.exists());

    press(&mut pane, &mut player, KeyCode::Char('d'));
    press(&mut pane, &mut player, KeyCode::Char('d'));

    assert!(!path.exists(), "the second press deletes it");
    assert!(pane.playlists().is_empty());
}

#[test]
fn songs_can_be_added_to_the_selected_playlist() {
    let directory = redirect();

    let tracks = songs("pane-add");
    let playlist = Playlist::new("Growing");
    playlist.save().expect("save");

    let mut pane = PlaylistPane::with_playlists(vec![playlist]);

    pane.add_songs(tracks[..2].to_vec());
    assert_eq!(pane.selected().expect("a playlist").len(), 2);
    assert!(render(&mut pane, true).contains("added 2"));

    // Saved as it goes, so nothing is left only in memory.
    let saved = Playlist::load_from(&directory.join("growing.toml")).expect("load");
    assert_eq!(saved.len(), 2);

    // What is already there is not added twice.
    pane.add_songs(tracks[..2].to_vec());
    assert_eq!(pane.selected().expect("a playlist").len(), 2);
    assert!(render(&mut pane, true).contains("already in"));

    pane.add_songs(Vec::new());
    assert!(render(&mut pane, true).contains("nothing to add"));
}

#[test]
fn a_message_about_an_action_does_not_cover_the_keys() {
    redirect();

    let mut pane = PlaylistPane::with_playlists(vec![Playlist::with_songs(
        "Sort Message",
        songs("pane-message-keys"),
    )]);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('s'));
    let frame = render(&mut pane, true);

    assert!(frame.contains("by "), "the message is shown: {frame}");
    assert!(frame.contains("enter load"), "and both key lines with it");
    assert!(frame.contains("s sort"));

    let rows: Vec<&str> = frame.lines().collect();
    let message = rows.iter().position(|row| row.contains("reverses")).expect("message row");
    let keys = rows.iter().position(|row| row.contains("enter load")).expect("key row");

    assert_eq!(keys, message + 1, "the message has a row of its own: {frame}");
}

#[test]
fn the_naming_prompt_takes_the_message_row_and_keeps_its_own_keys() {
    redirect();

    let mut pane = PlaylistPane::with_playlists(Vec::new());
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('c'));
    type_text(&mut pane, &mut player, "Night");

    let frame = render(&mut pane, true);

    assert!(frame.contains("name: Night"), "what is being typed: {frame}");
    assert!(frame.contains("enter save · esc cancel"), "with the keys for it");
    // The browsing keys give way while typing, since they do not apply.
    assert!(!frame.contains("s sort"));
}

#[test]
fn reports_of_what_an_action_did_can_be_silenced() {
    redirect();

    let mut pane = PlaylistPane::with_playlists(vec![Playlist::with_songs(
        "Quiet Queue",
        songs("pane-quiet"),
    )]);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Enter);
    assert!(render(&mut pane, true).contains("queued"), "it says what it did");

    pane.set_messages(false);
    press(&mut pane, &mut player, KeyCode::Enter);
    let quiet = render(&mut pane, true);

    assert!(!quiet.contains("queued"), "and then it does not: {quiet}");
    assert!(quiet.contains("Quiet Queue"), "the list itself stays");
    assert_eq!(player.queue().len(), 3, "the action itself still happened");

    // The naming prompt is what is being typed, not a report.
    press(&mut pane, &mut player, KeyCode::Char('c'));
    type_text(&mut pane, &mut player, "Quiet");
    assert!(render(&mut pane, true).contains("name: Quiet"));
}

#[test]
fn keys_the_pane_does_not_use_fall_through() {
    redirect();
    let mut pane = PlaylistPane::with_playlists(Vec::new());
    let mut player = Player::new();

    assert!(!press(&mut pane, &mut player, KeyCode::Char(' ')), "playback keys are not the pane's");
    assert!(!press(&mut pane, &mut player, KeyCode::Char('n')));
    assert!(!press(&mut pane, &mut player, KeyCode::Char('z')));

    assert!(press(&mut pane, &mut player, KeyCode::Char('c')), "its own keys are claimed");
}

// -------------------------------------------------------------- reaching in from the player screen

#[test]
fn the_file_listing_can_add_to_the_selected_playlist() {
    let directory = redirect();

    // A folder holding one track, for the file listing to point at.
    let dir = common::scratch_dir("screen-add-to-playlist");
    common::write_wav(&dir, &WavSpec { name: "only.wav", title: Some("Only"), ..WavSpec::default() });

    let playlist = Playlist::new("Collected");
    playlist.save().expect("save");

    let mut player = Player::new();
    let mut screen = PlayerScreen::with_playlists(&dir, vec![playlist]);

    // The screen opens on the files, with `..` highlighted; move onto the track and send it across.
    screen.handle_key(KeyEvent::from(KeyCode::Down), &mut player);
    screen.handle_key(KeyEvent::from(KeyCode::Char('P')), &mut player);

    let saved = Playlist::load_from(&directory.join("collected.toml")).expect("load");
    assert_eq!(saved.len(), 1, "the track reached the playlist on disk");
    assert_eq!(saved.as_added()[0].display_title(), "Only");
    assert!(player.queue().is_empty(), "and was not quietly queued as well");
}

#[test]
fn shuffling_rearranges_the_queue_but_not_what_is_playing() {
    redirect();

    let dir = common::scratch_dir("screen-shuffle");
    let tracks: Vec<Song> = (0..24)
        .map(|index| {
            Song::new(common::write_wav(
                &dir,
                &WavSpec {
                    name: &format!("{index:02}.wav"),
                    title: Some(&format!("Track {index:02}")),
                    ..WavSpec::default()
                },
            ))
        })
        .collect();

    let mut player = Player::new();
    player.add_queue_all(tracks.clone());
    player.play();

    let playing = player.current().cloned().expect("something playing");
    let before = titles(&player);

    let mut screen = PlayerScreen::with_playlists(&dir, Vec::new());
    screen.handle_key(KeyEvent::from(KeyCode::Char('z')), &mut player);

    let after = titles(&player);

    assert_eq!(player.current(), Some(&playing), "what is playing is left alone");
    assert_eq!(after.len(), before.len(), "and nothing is lost");

    let mut sorted_before = before.clone();
    let mut sorted_after = after.clone();
    sorted_before.sort();
    sorted_after.sort();
    assert_eq!(sorted_before, sorted_after, "the same tracks, in some other order");

    // With 23 tracks left, the chance of an unchanged order is vanishing.
    assert_ne!(before, after, "the order actually changed");
}

// --------------------------------------------------------------- opening a playlist in the files

#[test]
fn o_asks_for_the_playlist_to_be_opened() {
    redirect();

    let mut playlist = Playlist::with_songs("Late Night", songs("pane-open"));
    playlist.set_sort(SortBy::Title);

    let mut pane = PlaylistPane::with_playlists(vec![playlist.clone()]);
    let mut player = Player::new();

    let action = press_for(&mut pane, &mut player, KeyCode::Char('o'));

    assert_eq!(action, Some(PlaylistAction::Open(playlist)), "the pane hands over the playlist");
    assert!(player.queue().is_empty(), "opening is not queuing");
    assert!(render(&mut pane, true).contains("opened Late Night"));
}

#[test]
fn o_with_nothing_to_open_says_so() {
    redirect();

    let mut pane = PlaylistPane::with_playlists(Vec::new());
    let mut player = Player::new();

    // The key is still the pane's; there is simply nothing to open.
    assert_eq!(
        press_for(&mut pane, &mut player, KeyCode::Char('o')),
        Some(PlaylistAction::Handled)
    );
    assert!(render(&mut pane, true).contains("no playlists yet"));
}

#[test]
fn the_keys_include_opening_and_deleting() {
    redirect();
    let mut pane = PlaylistPane::with_playlists(vec![Playlist::new("Something")]);

    // At the width the player screen gives the pane, which is what the hints are written to fit.
    let frame = render_at(&mut pane, true, 33);

    // Keys are named in words: the enter and backspace glyphs are missing from many terminal fonts.
    for key in ["enter load", "o open", "c new", "s sort", "d del", "/ find"] {
        assert!(frame.contains(key), "{key} should be listed: {frame}");
    }
}

#[test]
fn opening_a_playlist_lists_it_in_the_file_viewer() {
    redirect();

    let dir = common::scratch_dir("screen-open-playlist");
    let tracks: Vec<Song> = [("c.wav", "Zulu"), ("a.wav", "Alpha"), ("b.wav", "Kilo")]
        .iter()
        .map(|(name, title)| {
            Song::new(common::write_wav(
                &dir,
                &WavSpec { name, title: Some(title), ..WavSpec::default() },
            ))
        })
        .collect();

    let mut playlist = Playlist::with_songs("Late Night", tracks);
    playlist.set_sort(SortBy::Title);

    let mut player = Player::new();
    let mut screen = PlayerScreen::with_playlists(&dir, vec![playlist]);

    // Onto the playlists pane, then open what is selected.
    screen.handle_key(KeyEvent::from(KeyCode::BackTab), &mut player);
    screen.handle_key(KeyEvent::from(KeyCode::Char('o')), &mut player);

    let frame = screen_frame(&mut screen, &player);

    // The file listing now shows the playlist, in the playlist's order, by artist and title.
    assert!(frame.contains("♪ Late Night"), "the listing says which playlist: {frame}");

    let rows: Vec<&str> = frame.lines().collect();
    let alpha = rows.iter().position(|row| row.contains("Alpha")).expect("Alpha listed");
    let kilo = rows.iter().position(|row| row.contains("Kilo")).expect("Kilo listed");
    let zulu = rows.iter().position(|row| row.contains("Zulu")).expect("Zulu listed");

    assert!(alpha < kilo && kilo < zulu, "sorted by title, as the playlist is: {frame}");
    // Tracks are named by what they are, not by their file names.
    assert!(!frame.contains("a.wav"), "the file name is not what is shown");

    // Adding from a playlist listing works the same as from a folder.
    screen.handle_key(KeyEvent::from(KeyCode::Char('a')), &mut player);
    assert_eq!(player.queue().len(), 1);
    assert_eq!(player.queue()[0].display_title(), "Alpha");

    // `A` takes the whole playlist.
    screen.handle_key(KeyEvent::from(KeyCode::Char('A')), &mut player);
    assert_eq!(player.queue().len(), 4, "one, then the three of them");

    // And backspace comes back to the folder it was browsing, which is headed by the folder's name
    // rather than the playlist's.
    screen.handle_key(KeyEvent::from(KeyCode::Backspace), &mut player);
    let frame = screen_frame(&mut screen, &player);
    assert!(!frame.contains("♪ Late Night"), "no longer the playlist: {frame}");
    assert!(frame.contains("screen-open-playlist"), "the folder is named: {frame}");
    assert!(frame.contains(".."), "and its way out is listed");
}

/// The whole player screen as text, for the cross-pane checks above.
fn screen_frame(screen: &mut PlayerScreen, player: &Player) -> String {
    let mut terminal = Terminal::new(TestBackend::new(104, 22)).expect("test terminal");

    terminal
        .draw(|frame| screen.render(frame, frame.area(), player))
        .expect("draw screen");

    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;

    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

// -------------------------------------------------------------------------------------- searching

fn several_playlists() -> Vec<Playlist> {
    ["Late Night", "Lazy Sunday", "Road Trip", "Workout"]
        .iter()
        .map(|name| Playlist::new(*name))
        .collect()
}

fn search(pane: &mut PlaylistPane, player: &mut Player, query: &str) {
    press(pane, player, KeyCode::Char('/'));

    for c in query.chars() {
        press(pane, player, KeyCode::Char(c));
    }
}

#[test]
fn slash_narrows_the_playlists_to_matching_names() {
    redirect();
    let mut pane = PlaylistPane::with_playlists(several_playlists());
    let mut player = Player::new();

    search(&mut pane, &mut player, "la");

    let frame = render_at(&mut pane, true, 33);

    assert!(frame.contains("/la"), "the query is shown: {frame}");
    assert!(frame.contains("Late Night"));
    assert!(frame.contains("Lazy Sunday"));
    assert!(!frame.contains("Road Trip"), "the rest go");
    assert!(!frame.contains("Workout"));

    press(&mut pane, &mut player, KeyCode::Enter);
    assert_eq!(pane.filter(), Some("la"));
    assert!(render_at(&mut pane, true, 33).contains("2 found"));
}

#[test]
fn the_selection_and_its_actions_follow_the_matches() {
    redirect();
    let mut pane = PlaylistPane::with_playlists(several_playlists());
    let mut player = Player::new();

    search(&mut pane, &mut player, "work");
    press(&mut pane, &mut player, KeyCode::Enter);

    // The only match is selected, however far down the unnarrowed list it was.
    assert_eq!(pane.selected().map(Playlist::name), Some("Workout"));

    // And moving about stays inside the matches.
    press(&mut pane, &mut player, KeyCode::Down);
    assert_eq!(pane.selected().map(Playlist::name), Some("Workout"));
}

#[test]
fn a_search_with_no_matches_says_so() {
    redirect();
    let mut pane = PlaylistPane::with_playlists(several_playlists());
    let mut player = Player::new();

    search(&mut pane, &mut player, "zzz");
    press(&mut pane, &mut player, KeyCode::Enter);

    let frame = render_at(&mut pane, true, 33);
    assert!(frame.contains("No matches"), "{frame}");
    assert!(pane.selected().is_none(), "and nothing is selected to act on");
}

#[test]
fn esc_undoes_the_search_before_anything_else() {
    redirect();
    let mut pane = PlaylistPane::with_playlists(several_playlists());
    let mut player = Player::new();

    search(&mut pane, &mut player, "la");
    press(&mut pane, &mut player, KeyCode::Enter);

    press(&mut pane, &mut player, KeyCode::Esc);
    assert_eq!(pane.filter(), None);
    assert!(render_at(&mut pane, true, 33).contains("Road Trip"), "the whole list is back");

    // With no search to undo, escape is not the pane's.
    assert!(!press(&mut pane, &mut player, KeyCode::Esc));
}

#[test]
fn tab_is_never_taken_by_the_search_or_the_naming_prompt() {
    redirect();
    let mut pane = PlaylistPane::with_playlists(several_playlists());
    let mut player = Player::new();

    assert!(!press(&mut pane, &mut player, KeyCode::Tab));

    search(&mut pane, &mut player, "la");
    assert!(pane.is_typing());
    assert!(!press(&mut pane, &mut player, KeyCode::Tab), "not while searching either");

    press(&mut pane, &mut player, KeyCode::Esc);
    press(&mut pane, &mut player, KeyCode::Char('c'));
    assert!(pane.is_typing(), "naming a new playlist");
    assert!(!press(&mut pane, &mut player, KeyCode::Tab), "nor while naming one");
    assert!(!press(&mut pane, &mut player, KeyCode::BackTab));
}

#[test]
fn a_new_playlist_clears_the_search_so_it_can_be_seen() {
    redirect();

    let mut pane = PlaylistPane::with_playlists(several_playlists());
    let mut player = Player::new();

    search(&mut pane, &mut player, "la");
    press(&mut pane, &mut player, KeyCode::Enter);

    press(&mut pane, &mut player, KeyCode::Char('c'));
    type_text(&mut pane, &mut player, "Search Cleared");
    press(&mut pane, &mut player, KeyCode::Enter);

    assert_eq!(pane.filter(), None, "a search that hid the new playlist would look like a failure");
    assert_eq!(pane.selected().map(Playlist::name), Some("Search Cleared"));
}
