//! Browsing files and putting them in the queue.

mod common;

use std::path::{Path, PathBuf};

use ogma::player::Player;
use ogma::ui::FilePane;
use ogma::song::Song;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

/// A small tree: two tracks here, a subfolder with two more, and files that are not music.
fn tree(name: &str) -> PathBuf {
    let root = common::scratch_dir(name);
    std::fs::create_dir_all(root.join("album")).expect("create album folder");
    std::fs::create_dir_all(root.join(".hidden")).expect("create hidden folder");

    for path in [
        root.join("b-second.flac"),
        root.join("a-first.mp3"),
        root.join("notes.txt"),
        root.join("album").join("01.opus"),
        root.join("album").join("02.wav"),
        root.join(".hidden").join("skipped.flac"),
    ] {
        std::fs::write(path, b"contents do not matter for browsing").expect("write file");
    }

    root.canonicalize().expect("canonical root")
}

fn render(pane: &mut FilePane, focused: bool) -> String {
    render_at(pane, focused, 30)
}

/// The pane at a given width. The player screen gives it 33 cells, which is the width its hints are
/// written to fit.
fn render_at(pane: &mut FilePane, focused: bool, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 16)).expect("test terminal");

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

fn press(pane: &mut FilePane, player: &mut Player, code: KeyCode) -> bool {
    pane.handle_key(KeyEvent::from(code), player)
}

/// Move the highlight onto the row whose text contains `needle`.
fn highlight(pane: &mut FilePane, player: &mut Player, needle: &str) {
    for _ in 0..40 {
        let frame = render(pane, true);

        // The highlighted row is the one carrying the selection marker; the pane border precedes it.
        let on_it = frame
            .lines()
            .any(|row| row.contains('›') && row.contains(needle));

        if on_it {
            return;
        }

        press(pane, player, KeyCode::Down);
    }

    panic!("never highlighted {needle}");
}

#[test]
fn lists_folders_and_tracks_but_not_other_files() {
    let root = tree("pane-listing");
    let mut pane = FilePane::at(&root);

    let frame = render(&mut pane, true);

    assert!(frame.contains("album"), "folders are listed: {frame}");
    assert!(frame.contains("a-first.mp3"), "tracks are listed with their extension");
    assert!(frame.contains("b-second.flac"));
    assert!(frame.contains(".."), "and the way out");

    assert!(!frame.contains("notes.txt"), "files that are not music are left out");
    assert!(!frame.contains(".hidden"), "nor are hidden folders shown");

    // Folders come before tracks, and each is sorted.
    let rows: Vec<&str> = frame.lines().collect();
    let album = rows.iter().position(|r| r.contains("album")).expect("album row");
    let first = rows.iter().position(|r| r.contains("a-first")).expect("first track");
    let second = rows.iter().position(|r| r.contains("b-second")).expect("second track");

    assert!(album < first && first < second, "{frame}");
}

#[test]
fn music_is_listed_by_artist_and_title_where_the_tags_say() {
    let dir = common::scratch_dir("pane-labels");

    // Tagged both ways, one way, and not at all.
    common::write_wav(
        &dir,
        &common::WavSpec {
            name: "01-full.wav",
            title: Some("Xtal"),
            artist: Some("Aphex Twin"),
            ..common::WavSpec::default()
        },
    );
    common::write_wav(
        &dir,
        &common::WavSpec {
            name: "02-title-only.wav",
            title: Some("Untitled Piece"),
            ..common::WavSpec::default()
        },
    );
    common::write_wav(&dir, &common::WavSpec { name: "03-bare.wav", ..common::WavSpec::default() });

    let mut pane = FilePane::at(&dir);
    let frame = render(&mut pane, true);

    assert!(frame.contains("Aphex Twin — Xtal"), "artist and title: {frame}");
    assert!(frame.contains("Untitled Piece"), "the title alone where there is no artist");
    assert!(!frame.contains("01-full.wav"), "the file name gives way to the tags");

    // A track with nothing to go on keeps its file name, which is all there is.
    assert!(frame.contains("03-bare.wav"), "{frame}");
}

#[test]
fn a_queues_the_highlighted_track() {
    let root = tree("pane-queue-track");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    highlight(&mut pane, &mut player, "a-first.mp3");
    assert!(press(&mut pane, &mut player, KeyCode::Char('a')));

    assert_eq!(player.queue().len(), 1);
    assert_eq!(player.queue()[0].path(), root.join("a-first.mp3"));
    assert!(render(&mut pane, true).contains("queued"), "it says what it did");

    // Adding again appends rather than replacing.
    highlight(&mut pane, &mut player, "b-second.flac");
    press(&mut pane, &mut player, KeyCode::Char('a'));

    assert_eq!(player.queue().len(), 2);
    assert_eq!(player.queue()[1].path(), root.join("b-second.flac"));
}

#[test]
fn a_on_a_folder_queues_everything_inside_it() {
    let root = tree("pane-queue-folder");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    highlight(&mut pane, &mut player, "album");
    press(&mut pane, &mut player, KeyCode::Char('a'));

    assert_eq!(player.queue().len(), 2, "both tracks in the folder");
    assert!(player.queue().iter().all(|song| song.path().starts_with(root.join("album"))));
    assert!(render(&mut pane, true).contains("2 tracks"));
}

#[test]
fn a_on_the_way_out_queues_nothing() {
    let root = tree("pane-queue-parent");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    // The parent entry is highlighted first.
    press(&mut pane, &mut player, KeyCode::Char('a'));

    assert!(player.queue().is_empty(), "the folder above is not swept into the queue");
    assert!(render(&mut pane, true).contains("use A"), "and it says what to press instead");
}

#[test]
fn capital_a_queues_the_whole_folder_being_listed() {
    let root = tree("pane-queue-all");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    press(&mut pane, &mut player, KeyCode::Char('A'));

    // Everything at or below here: two tracks in this folder, two in the subfolder.
    assert_eq!(player.queue().len(), 4);
    assert!(render(&mut pane, true).contains("4 tracks"));
}

#[test]
fn enter_opens_a_folder_and_plays_a_track() {
    let root = tree("pane-enter");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    highlight(&mut pane, &mut player, "album");
    press(&mut pane, &mut player, KeyCode::Enter);

    assert_eq!(pane.cwd(), root.join("album"));
    let inside = render(&mut pane, true);
    assert!(inside.contains("01.opus") && inside.contains("02.wav"), "{inside}");

    // Enter on a track starts it straight away rather than queuing it.
    highlight(&mut pane, &mut player, "01.opus");
    press(&mut pane, &mut player, KeyCode::Enter);

    assert_eq!(player.current().map(|song| song.path().to_path_buf()), Some(root.join("album").join("01.opus")));
    assert!(player.queue().is_empty(), "playing now does not queue it as well");
    assert!(render(&mut pane, true).contains("playing"));

    // And back up again. Backspace, not Left: the arrows belong to seeking.
    assert!(!press(&mut pane, &mut player, KeyCode::Left), "Left is left for the transport");
    press(&mut pane, &mut player, KeyCode::Backspace);
    assert_eq!(pane.cwd(), root);
}

#[test]
fn the_highlighted_row_can_be_handed_to_a_playlist() {
    let root = tree("pane-playlist");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    // `P` is not the pane's to answer: the playlists live in their own pane, so the screen routes
    // it there. What the pane provides is what the highlighted row stands for.
    assert!(!press(&mut pane, &mut player, KeyCode::Char('P')));

    highlight(&mut pane, &mut player, "a-first.mp3");
    let songs = pane.highlighted_songs();
    assert_eq!(songs.len(), 1);
    assert_eq!(songs[0].path(), root.join("a-first.mp3"));

    // A folder stands for everything inside it.
    highlight(&mut pane, &mut player, "album");
    assert_eq!(pane.highlighted_songs().len(), 2);

    // And the way out stands for nothing.
    press(&mut pane, &mut player, KeyCode::Char('g'));
    assert!(pane.highlighted_songs().is_empty());

    assert!(player.queue().is_empty(), "none of which queues anything");
}

#[test]
fn keys_the_pane_does_not_use_are_left_for_the_transport() {
    let root = tree("pane-passthrough");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    // Space, `n` and `p` belong to playback, not to browsing.
    assert!(!press(&mut pane, &mut player, KeyCode::Char(' ')));
    assert!(!press(&mut pane, &mut player, KeyCode::Char('n')));
    assert!(!press(&mut pane, &mut player, KeyCode::Char('p')));

    // Whereas the pane's own keys are claimed.
    assert!(press(&mut pane, &mut player, KeyCode::Down));
    assert!(press(&mut pane, &mut player, KeyCode::Char('a')));
}

#[test]
fn an_unreadable_folder_leaves_the_listing_alone() {
    let root = tree("pane-unreadable");
    let mut pane = FilePane::at(&root);
    let before = render(&mut pane, true);

    let mut missing = FilePane::at(&root.join("album").join("nope"));

    assert!(render(&mut missing, true).contains("cannot open"));
    assert_eq!(render(&mut pane, true), before, "the good listing is untouched");
}

#[test]
fn a_message_about_an_action_does_not_cover_the_keys() {
    let root = tree("pane-message-and-keys");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    let before = render(&mut pane, true);
    assert!(before.contains("a queue"), "both key lines to begin with: {before}");
    assert!(before.contains("enter open"));

    // Doing something produces a message, which is exactly when the keys are still wanted.
    highlight(&mut pane, &mut player, "a-first.mp3");
    press(&mut pane, &mut player, KeyCode::Char('a'));

    let after = render(&mut pane, true);
    assert!(after.contains("queued"), "the message is shown: {after}");
    assert!(after.contains("a queue"), "and so are the keys");
    assert!(after.contains("enter open"));

    // Each on its own row.
    let rows: Vec<&str> = after.lines().collect();
    let message = rows.iter().position(|row| row.contains("queued")).expect("message row");
    let first = rows.iter().position(|row| row.contains("a queue")).expect("first key row");
    let second = rows.iter().position(|row| row.contains("enter open")).expect("second key row");

    assert_eq!(first, message + 1, "{after}");
    assert_eq!(second, first + 1);
}

#[test]
fn every_key_the_pane_answers_is_named() {
    let root = tree("pane-keys-listed");
    let mut pane = FilePane::at(&root);

    let frame = render_at(&mut pane, true, 33);

    // Including adding to a playlist, which the screen acts on but which is pressed here.
    for key in ["a queue", "A all", "P playlist", "enter open", "bksp up"] {
        assert!(frame.contains(key), "{key} should be listed: {frame}");
    }
}

#[test]
fn reports_of_what_an_action_did_can_be_silenced() {
    let root = tree("pane-quiet");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    highlight(&mut pane, &mut player, "a-first.mp3");
    press(&mut pane, &mut player, KeyCode::Char('a'));
    assert!(render(&mut pane, true).contains("queued"), "it says what it did");

    pane.set_messages(false);
    press(&mut pane, &mut player, KeyCode::Char('a'));

    let quiet = render(&mut pane, true);
    assert!(!quiet.contains("queued"), "and then it does not: {quiet}");
    // The listing, the keys and a search are not reports.
    assert!(quiet.contains("a-first.mp3") || quiet.contains("a queue"));
    assert_eq!(player.queue().len(), 2, "the action itself still happened");

    press(&mut pane, &mut player, KeyCode::Char('/'));
    press(&mut pane, &mut player, KeyCode::Char('a'));
    assert!(render(&mut pane, true).contains("/a"), "a search is still shown");
}

#[test]
fn the_keys_are_shown_while_the_pane_has_the_focus() {
    let root = tree("pane-hint");
    let mut pane = FilePane::at(&root);

    assert!(render(&mut pane, true).contains("a queue"), "hint while focused");
    assert!(!render(&mut pane, false).contains("a queue"), "and not while it does not");
}

/// `FilePane::new` opens where the player was launched from.
#[test]
fn opens_in_the_working_directory_by_default() {
    let pane = FilePane::new();
    let cwd = std::env::current_dir().expect("cwd").canonicalize().expect("canonical");

    assert_eq!(pane.cwd(), cwd.as_path() as &Path);
}

// -------------------------------------------------------------------------------------- searching

/// A folder of tracks with names that a search can tell apart.
fn searchable(name: &str) -> PathBuf {
    let dir = common::scratch_dir(name);

    for (file, title, artist) in [
        ("01.wav", "Xtal", "Aphex Twin"),
        ("02.wav", "Tha", "Aphex Twin"),
        ("03.wav", "Nocturne", "Chopin"),
        ("04.wav", "Prelude", "Chopin"),
    ] {
        common::write_wav(
            &dir,
            &common::WavSpec {
                name: file,
                title: Some(title),
                artist: Some(artist),
                ..common::WavSpec::default()
            },
        );
    }

    std::fs::create_dir_all(dir.join("chopin-box-set")).expect("create folder");

    dir.canonicalize().expect("canonical")
}

fn search(pane: &mut FilePane, player: &mut Player, query: &str) {
    press(pane, player, KeyCode::Char('/'));

    for c in query.chars() {
        press(pane, player, KeyCode::Char(c));
    }
}

#[test]
fn slash_narrows_the_listing_to_what_is_typed() {
    let root = searchable("pane-search");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    search(&mut pane, &mut player, "chop");

    let frame = render_at(&mut pane, true, 33);

    assert!(frame.contains("/chop"), "the query is shown: {frame}");
    assert!(frame.contains("Chopin — Nocturne"), "matches stay");
    assert!(frame.contains("Chopin — Prelude"));
    assert!(frame.contains("chopin-box-set"), "folders match on their names too");
    assert!(!frame.contains("Aphex"), "everything else goes");
    assert!(frame.contains(".."), "and the way out is always reachable");
}

#[test]
fn a_search_matches_what_is_on_screen_not_the_file_name() {
    let root = searchable("pane-search-labels");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    // The tracks are named 01.wav and so on, but listed by artist and title.
    search(&mut pane, &mut player, "xtal");
    assert!(render_at(&mut pane, true, 33).contains("Aphex Twin — Xtal"));

    press(&mut pane, &mut player, KeyCode::Esc);
    search(&mut pane, &mut player, "01.wav");
    // The count is shown once the query is accepted; while typing, the query itself is.
    press(&mut pane, &mut player, KeyCode::Enter);

    let frame = render_at(&mut pane, true, 33);
    assert!(frame.contains("0 found"), "the file name is not what is matched: {frame}");
}

#[test]
fn enter_keeps_the_search_and_esc_undoes_it() {
    let root = searchable("pane-search-keep");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    search(&mut pane, &mut player, "chop");
    assert!(pane.is_typing(), "typing into the search");

    press(&mut pane, &mut player, KeyCode::Enter);
    assert!(!pane.is_typing(), "back to the listing");
    assert_eq!(pane.filter(), Some("chop"), "with the search still narrowing it");

    let frame = render_at(&mut pane, true, 33);
    assert!(frame.contains("3 found"), "which says how many: {frame}");
    assert!(frame.contains("a queue"), "and the listing's keys are back");

    // Escape puts the whole listing back rather than leaving the pane.
    assert!(press(&mut pane, &mut player, KeyCode::Esc), "the pane claims it");
    assert_eq!(pane.filter(), None);
    assert!(render_at(&mut pane, true, 33).contains("Aphex"));

    // With no search to undo, escape is not the pane's to take.
    assert!(!press(&mut pane, &mut player, KeyCode::Esc));
}

#[test]
fn a_search_can_be_corrected_and_abandoned() {
    let root = searchable("pane-search-edit");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    search(&mut pane, &mut player, "chopx");
    assert!(render_at(&mut pane, true, 33).contains("/chopx"));

    press(&mut pane, &mut player, KeyCode::Backspace);
    press(&mut pane, &mut player, KeyCode::Enter);
    assert_eq!(pane.filter(), Some("chop"));

    // Ctrl-U empties the query, and an empty query is no filter at all.
    search(&mut pane, &mut player, "aphex");
    pane.handle_key(
        KeyEvent::new(KeyCode::Char('u'), ratatui::crossterm::event::KeyModifiers::CONTROL),
        &mut player,
    );
    press(&mut pane, &mut player, KeyCode::Enter);
    assert_eq!(pane.filter(), None, "an empty search narrows nothing");

    // Escape while typing abandons it.
    search(&mut pane, &mut player, "chop");
    press(&mut pane, &mut player, KeyCode::Esc);
    assert_eq!(pane.filter(), None);
    assert!(!pane.is_typing());
}

#[test]
fn tab_is_never_taken_by_the_search() {
    let root = searchable("pane-search-tab");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    // Whether browsing or typing, tab belongs to the panes.
    assert!(!press(&mut pane, &mut player, KeyCode::Tab));
    assert!(!press(&mut pane, &mut player, KeyCode::BackTab));

    search(&mut pane, &mut player, "chop");
    assert!(pane.is_typing());
    assert!(!press(&mut pane, &mut player, KeyCode::Tab), "still not the pane's");
    assert!(!press(&mut pane, &mut player, KeyCode::BackTab));

    // And the search survives changing panes and coming back.
    assert!(pane.is_typing());
    assert_eq!(pane.filter(), Some("chop"));
}

#[test]
fn actions_apply_to_the_matches_while_a_search_is_narrowing_things() {
    let root = searchable("pane-search-actions");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    search(&mut pane, &mut player, "chopin —");
    press(&mut pane, &mut player, KeyCode::Enter);

    // `a` queues what is highlighted among the matches.
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Char('a'));
    assert_eq!(player.queue().len(), 1);
    assert_eq!(player.queue()[0].display_title(), "Nocturne");

    // `A` queues the matches, not the whole folder.
    press(&mut pane, &mut player, KeyCode::Char('A'));
    assert_eq!(player.queue().len(), 3, "the two matches on top of the one");
    assert!(
        player.queue().iter().all(|song| song.display_artist() == "Chopin"),
        "nothing outside the search"
    );
}

#[test]
fn opening_a_folder_leaves_the_search_behind() {
    let root = searchable("pane-search-open");
    let mut player = Player::new();
    let mut pane = FilePane::at(&root);

    search(&mut pane, &mut player, "chopin-box");
    press(&mut pane, &mut player, KeyCode::Enter);

    // Into the one folder that matched.
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Enter);

    assert_eq!(pane.cwd(), root.join("chopin-box-set"));
    assert_eq!(pane.filter(), None, "a new listing starts unnarrowed");
}

// --------------------------------------------------------------------------- moving files about

#[test]
fn x_picks_a_file_up_and_p_puts_it_down_in_another_folder() {
    let root = tree("pane-move");
    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    // Down past `..` and the album folder, onto the first track.
    press(&mut pane, &mut player, KeyCode::Char('g'));
    for _ in 0..2 {
        press(&mut pane, &mut player, KeyCode::Down);
    }

    press(&mut pane, &mut player, KeyCode::Char('x'));
    assert_eq!(pane.held().len(), 1, "one file is held");
    assert!(render(&mut pane, true).contains('✂'), "and the row says so");

    // Into the album folder, and put it down.
    press(&mut pane, &mut player, KeyCode::Char('g'));
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Enter);
    assert_eq!(pane.cwd(), root.join("album"));

    press(&mut pane, &mut player, KeyCode::Char('M'));

    assert!(root.join("album").join("a-first.mp3").is_file(), "the file moved");
    assert!(!root.join("a-first.mp3").exists(), "and is no longer where it was");
    assert!(pane.held().is_empty(), "nothing is still held");

    let frame = render(&mut pane, true);
    assert!(frame.contains("moved 1 here"), "{frame}");
    assert!(frame.contains("a-first"), "and the listing shows it: {frame}");
}

#[test]
fn x_twice_puts_the_file_back_down_without_moving_it() {
    let root = tree("pane-unhold");
    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('g'));
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Down);

    press(&mut pane, &mut player, KeyCode::Char('x'));
    press(&mut pane, &mut player, KeyCode::Char('x'));

    assert!(pane.held().is_empty());
    assert!(render(&mut pane, true).contains("put down"));
}

#[test]
fn several_files_move_at_once() {
    let root = tree("pane-move-several");
    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    // Both tracks in the root.
    press(&mut pane, &mut player, KeyCode::Char('g'));
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Char('x'));
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Char('x'));
    assert_eq!(pane.held().len(), 2);

    press(&mut pane, &mut player, KeyCode::Char('g'));
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Enter);
    press(&mut pane, &mut player, KeyCode::Char('M'));

    assert!(root.join("album").join("a-first.mp3").is_file());
    assert!(root.join("album").join("b-second.flac").is_file());
    assert!(render(&mut pane, true).contains("moved 2 here"));
}

#[test]
fn a_whole_folder_can_be_moved() {
    let root = tree("pane-move-folder");
    std::fs::create_dir_all(root.join("elsewhere")).expect("create folder");

    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    // The album folder, held.
    press(&mut pane, &mut player, KeyCode::Char('g'));
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Char('x'));

    // Into `elsewhere`, which sorts after `album`.
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Enter);
    assert_eq!(pane.cwd(), root.join("elsewhere"));

    press(&mut pane, &mut player, KeyCode::Char('M'));

    assert!(root.join("elsewhere").join("album").join("01.opus").is_file(), "it moved whole");
    assert!(!root.join("album").exists());
}

#[test]
fn a_name_already_taken_is_refused_and_the_file_stays_held() {
    let root = tree("pane-move-clash");
    std::fs::write(root.join("album").join("a-first.mp3"), b"already here").expect("write file");

    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('g'));
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Char('x'));

    press(&mut pane, &mut player, KeyCode::Char('g'));
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Enter);
    press(&mut pane, &mut player, KeyCode::Char('M'));

    assert_eq!(pane.held().len(), 1, "it is still held, to put down somewhere else");
    assert!(root.join("a-first.mp3").is_file(), "and has not moved");

    let frame = render(&mut pane, true);
    assert!(frame.contains("already there"), "{frame}");

    // What was there is untouched.
    let kept = std::fs::read(root.join("album").join("a-first.mp3")).expect("read");
    assert_eq!(kept, b"already here");
}

#[test]
fn a_folder_cannot_be_moved_into_itself() {
    let root = tree("pane-move-into-itself");
    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('g'));
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Char('x'));
    press(&mut pane, &mut player, KeyCode::Enter);
    assert_eq!(pane.cwd(), root.join("album"));

    press(&mut pane, &mut player, KeyCode::Char('M'));

    assert!(root.join("album").join("01.opus").is_file(), "the folder is where it was");
    assert!(render(&mut pane, true).contains("cannot hold itself"));
}

#[test]
fn moving_with_nothing_held_says_what_to_press() {
    let root = tree("pane-move-nothing");
    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('M'));

    assert!(render(&mut pane, true).contains("nothing held"));
}

#[test]
fn putting_a_file_down_where_it_already_is_changes_nothing() {
    let root = tree("pane-move-same-folder");
    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('g'));
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Char('x'));
    press(&mut pane, &mut player, KeyCode::Char('M'));

    assert!(root.join("a-first.mp3").is_file());
    assert!(render(&mut pane, true).contains("already here"));
}

// ------------------------------------------------------------------- the pane's other modes

/// A stand-in for yt-dlp, so the search and the download can be driven without a network.
///
/// It answers `--version` like the real thing, prints two results for a search, and for a download
/// writes a file into the folder it was given and prints where it went — which is the whole of the
/// conversation this pane has with it.
///
/// Written and pointed at once for the whole test binary: the tests here run alongside each other,
/// and setting an environment variable while another thread is starting a process is exactly the
/// race that leaves one of them talking to the real yt-dlp.
fn fake_yt_dlp() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();

    INSTALLED.call_once(|| {
        let root = common::scratch_dir("pane-fake-ytdlp");
        let script = root.join("fake-yt-dlp");

        std::fs::write(
            &script,
            r#"#!/bin/sh
for arg in "$@"; do
    case "$arg" in
        --version) echo "2026.01.01"; exit 0 ;;
    esac
done

case "$*" in
    *--dump-json*)
        echo '{"id":"aaaaaaaaaaa","url":"https://www.youtube.com/watch?v=aaaaaaaaaaa","title":"First Song","channel":"A Channel","duration":131}'
        echo '{"id":"bbbbbbbbbbb","url":"https://www.youtube.com/watch?v=bbbbbbbbbbb","title":"Second Song","channel":"Another Channel","duration":245}'
        exit 0
        ;;
esac

# A download: the folder it was handed follows --paths.
folder=""
want=""
for arg in "$@"; do
    if [ "$want" = "yes" ]; then folder="$arg"; want=""; fi
    if [ "$arg" = "--paths" ]; then want="yes"; fi
done

echo "[ogma]  50.0%"
echo "[ogma] 100.0%"
printf 'fetched' > "$folder/A Channel - First Song.opus"
echo "$folder/A Channel - First Song.opus"
"#,
        )
        .expect("write fake yt-dlp");

        let mut permissions = std::fs::metadata(&script).expect("stat").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&script, permissions).expect("chmod");

        // SAFETY: written once, before any test here starts a process, and never changed after.
        unsafe {
            std::env::set_var("OGMA_YTDLP", &script);
        }
    });
}

/// Poll the pane until `done` says the background work has landed, or give up.
fn wait_for(pane: &mut FilePane, done: impl Fn(&mut FilePane) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

    while std::time::Instant::now() < deadline {
        pane.poll();

        if done(pane) {
            return;
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    panic!("the background work never finished");
}

/// A tagged library, so browsing by artist has something to group.
fn tagged_tree(name: &str) -> PathBuf {
    let root = common::scratch_dir(name);

    common::write_wav(
        &root,
        &common::WavSpec {
            name: "01.wav",
            title: Some("Xtal"),
            artist: Some("Aphex Twin"),
            album: Some("Selected Ambient Works"),
            ..common::WavSpec::default()
        },
    );
    common::write_wav(
        &root,
        &common::WavSpec {
            name: "02.wav",
            title: Some("Nocturne"),
            artist: Some("Chopin"),
            album: Some("Nocturnes"),
            ..common::WavSpec::default()
        },
    );

    root.canonicalize().expect("canonical root")
}

#[test]
fn m_cycles_the_pane_through_its_modes() {
    let root = tagged_tree("pane-modes");
    fake_yt_dlp();

    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    assert_eq!(pane.mode_name(), "files", "it opens on the folder");

    press(&mut pane, &mut player, KeyCode::Char('m'));
    assert_eq!(pane.mode_name(), "browse");

    press(&mut pane, &mut player, KeyCode::Char('m'));
    assert_eq!(pane.mode_name(), "youtube");

    // Arriving does not put the keyboard in a text field: `/` is what asks for one.
    assert!(!pane.is_typing(), "the keys are still the pane's");
    press(&mut pane, &mut player, KeyCode::Char('/'));
    assert!(pane.is_typing());
    press(&mut pane, &mut player, KeyCode::Esc);

    press(&mut pane, &mut player, KeyCode::Char('m'));
    assert_eq!(pane.mode_name(), "files", "and round again");
    assert_eq!(pane.cwd(), root, "in the folder it started in");
}

#[test]
fn browse_groups_the_folder_by_artist() {
    let root = tagged_tree("pane-browse");
    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('m'));
    wait_for(&mut pane, |pane| render(pane, true).contains("Aphex"));

    let frame = render(&mut pane, true);
    assert!(frame.contains("artist"), "the header says what it is grouping by: {frame}");
    assert!(frame.contains("Aphex Twin"), "{frame}");
    assert!(frame.contains("Chopin"), "{frame}");
    assert!(!frame.contains("01.wav"), "rows are names, not files: {frame}");
}

#[test]
fn t_changes_what_the_browse_files_tracks_under() {
    let root = tagged_tree("pane-facet");
    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('m'));
    wait_for(&mut pane, |pane| render(pane, true).contains("Aphex"));

    press(&mut pane, &mut player, KeyCode::Char('t'));

    let frame = render(&mut pane, true);
    assert!(frame.contains("album"), "{frame}");
    assert!(frame.contains("Selected Ambient"), "grouped by album now: {frame}");
}

#[test]
fn a_group_opens_into_its_tracks_and_backspace_comes_out() {
    let root = tagged_tree("pane-group-open");
    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('m'));
    wait_for(&mut pane, |pane| render(pane, true).contains("Aphex"));

    press(&mut pane, &mut player, KeyCode::Enter);

    let frame = render(&mut pane, true);
    assert!(frame.contains("Xtal"), "the artist's tracks: {frame}");
    assert!(!frame.contains("Chopin"), "and only theirs: {frame}");

    press(&mut pane, &mut player, KeyCode::Backspace);
    assert!(render(&mut pane, true).contains("Chopin"), "back to the groups");
}

#[test]
fn a_queues_a_whole_group() {
    let root = tagged_tree("pane-group-queue");
    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('m'));
    wait_for(&mut pane, |pane| render(pane, true).contains("Aphex"));

    press(&mut pane, &mut player, KeyCode::Char('a'));

    assert_eq!(player.queue().len(), 1, "the one track that artist has here");
    assert_eq!(player.queue()[0].title(), Some("Xtal"));
    // What `P` sends to a playlist is the same thing.
    assert_eq!(pane.highlighted_songs().len(), 1);
}

#[test]
fn a_search_fills_the_listing_with_tracks_that_stream() {
    let root = tagged_tree("pane-search");
    fake_yt_dlp();

    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('m'));
    press(&mut pane, &mut player, KeyCode::Char('m'));
    assert_eq!(pane.mode_name(), "youtube");

    press(&mut pane, &mut player, KeyCode::Char('/'));
    for c in "aphex".chars() {
        press(&mut pane, &mut player, KeyCode::Char(c));
    }
    press(&mut pane, &mut player, KeyCode::Enter);

    wait_for(&mut pane, |pane| !pane.results().is_empty());

    assert_eq!(pane.results().len(), 2);
    assert!(pane.results()[0].is_stream());

    let frame = render(&mut pane, true);
    assert!(frame.contains("First Song"), "{frame}");
    assert!(frame.contains("2 results"), "{frame}");
}

#[test]
fn a_result_plays_queues_and_goes_to_a_playlist_like_any_other_row() {
    let root = tagged_tree("pane-search-use");
    fake_yt_dlp();

    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('m'));
    press(&mut pane, &mut player, KeyCode::Char('m'));
    press(&mut pane, &mut player, KeyCode::Char('/'));
    for c in "song".chars() {
        press(&mut pane, &mut player, KeyCode::Char(c));
    }
    press(&mut pane, &mut player, KeyCode::Enter);
    wait_for(&mut pane, |pane| !pane.results().is_empty());

    // Enter streams what is highlighted.
    press(&mut pane, &mut player, KeyCode::Enter);
    let playing = player.current().expect("something is playing");
    assert!(playing.is_stream());
    assert_eq!(playing.title(), Some("First Song"));

    // `a` queues it, and what `P` would send to a playlist knows what it is.
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Char('a'));
    assert_eq!(player.queue().len(), 1);
    assert_eq!(player.queue()[0].title(), Some("Second Song"));

    let chosen = pane.highlighted_songs();
    assert_eq!(chosen.len(), 1);
    assert!(chosen[0].is_stream());
    assert_eq!(chosen[0].title(), Some("Second Song"));
}

#[test]
fn a_stream_cannot_be_picked_up_to_move() {
    let root = tagged_tree("pane-search-move");
    fake_yt_dlp();

    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('m'));
    press(&mut pane, &mut player, KeyCode::Char('m'));
    press(&mut pane, &mut player, KeyCode::Char('/'));
    for c in "song".chars() {
        press(&mut pane, &mut player, KeyCode::Char(c));
    }
    press(&mut pane, &mut player, KeyCode::Enter);
    wait_for(&mut pane, |pane| !pane.results().is_empty());

    press(&mut pane, &mut player, KeyCode::Char('x'));

    assert!(pane.held().is_empty());
    assert!(render(&mut pane, true).contains("not a file"));
}

#[test]
fn d_downloads_the_highlighted_result() {
    let root = tagged_tree("pane-download");
    fake_yt_dlp();

    let into = root.join("downloads");
    let mut pane = FilePane::at(&root);
    pane.set_download_folder(into.clone());

    let mut player = Player::new();

    press(&mut pane, &mut player, KeyCode::Char('m'));
    press(&mut pane, &mut player, KeyCode::Char('m'));
    press(&mut pane, &mut player, KeyCode::Char('/'));
    for c in "song".chars() {
        press(&mut pane, &mut player, KeyCode::Char(c));
    }
    press(&mut pane, &mut player, KeyCode::Enter);
    wait_for(&mut pane, |pane| !pane.results().is_empty());

    press(&mut pane, &mut player, KeyCode::Char('d'));
    wait_for(&mut pane, |pane| render(pane, true).contains("saved"));

    assert!(into.join("A Channel - First Song.opus").is_file(), "the file is where it was asked for");
}

#[test]
fn the_title_says_which_mode_is_showing() {
    let root = tagged_tree("pane-title");
    fake_yt_dlp();

    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    assert!(render(&mut pane, true).contains("Files"));

    press(&mut pane, &mut player, KeyCode::Char('m'));
    assert!(render(&mut pane, true).contains("Browse"));

    press(&mut pane, &mut player, KeyCode::Char('m'));
    assert!(render(&mut pane, true).contains("YouTube"));

    press(&mut pane, &mut player, KeyCode::Esc);
    press(&mut pane, &mut player, KeyCode::Char('m'));

    let frame = render(&mut pane, true);
    assert!(frame.contains("Files"), "back round to the folder: {frame}");
    assert!(!frame.contains("YouTube"), "and nothing of the last mode is left: {frame}");
}

#[test]
fn a_playlist_is_titled_as_one() {
    let root = tagged_tree("pane-title-playlist");
    let mut pane = FilePane::at(&root);

    pane.show_playlist(&ogma::playlist::Playlist::new("Late Night"));

    let frame = render(&mut pane, true);
    assert!(frame.contains("Playlist"), "{frame}");
    assert!(frame.contains("Late Night"), "and which one, under the border: {frame}");
}

#[test]
fn every_mode_lists_its_own_keys_in_full() {
    let root = tagged_tree("pane-hints");
    fake_yt_dlp();

    let mut pane = FilePane::at(&root);
    let mut player = Player::new();

    // At the width the player screen gives the pane. A key reminder that runs off the edge loses
    // the key, which is the part worth reading, so each one is checked whole.
    let shown = |pane: &mut FilePane| render_at(pane, true, 33);

    let files = shown(&mut pane);
    for keys in ["a queue", "A all", "P playlist", "enter open", "bksp up", "/ find", "x hold", "M move", "m mode"] {
        assert!(files.contains(keys), "the files mode does not show {keys}:\n{files}");
    }

    press(&mut pane, &mut player, KeyCode::Char('m'));
    let browse = shown(&mut pane);
    for keys in ["a queue", "A all", "P playlist", "enter open", "t by", "bksp back", "/ find", "m mode"] {
        assert!(browse.contains(keys), "the browse mode does not show {keys}:\n{browse}");
    }

    press(&mut pane, &mut player, KeyCode::Char('m'));
    let youtube = shown(&mut pane);
    for keys in ["enter play", "a queue", "A all", "P playlist", "d download", "/ search", "m mode"] {
        assert!(youtube.contains(keys), "the youtube mode does not show {keys}:\n{youtube}");
    }

    press(&mut pane, &mut player, KeyCode::Char('/'));
    let typing = shown(&mut pane);
    assert!(typing.contains("enter search"), "{typing}");
    assert!(typing.contains("esc cancel"), "{typing}");
    press(&mut pane, &mut player, KeyCode::Esc);

    pane.show_playlist(&ogma::playlist::Playlist::new("Late Night"));
    let playlist = shown(&mut pane);
    for keys in ["a queue", "A all", "P playlist", "bksp back", "/ find", "m mode"] {
        assert!(playlist.contains(keys), "the playlist mode does not show {keys}:\n{playlist}");
    }
}

// -------------------------------------------------------------------------------------- genres

/// Type `text` into whatever prompt is open.
fn type_text(pane: &mut FilePane, player: &mut Player, text: &str) {
    for c in text.chars() {
        press(pane, player, KeyCode::Char(c));
    }
}

/// Empty the prompt, which is ctrl-u as it is everywhere else in the interface.
fn clear_prompt(pane: &mut FilePane, player: &mut Player) {
    let key = KeyEvent::new(
        KeyCode::Char('u'),
        ratatui::crossterm::event::KeyModifiers::CONTROL,
    );

    pane.handle_key(key, player);
}

#[test]
fn e_types_a_genre_onto_the_highlighted_track() {
    let dir = common::scratch_dir("pane-genre-one");
    common::write_wav(
        &dir,
        &common::WavSpec {
            name: "01-track.wav",
            title: Some("Xtal"),
            ..common::WavSpec::default()
        },
    );
    common::write_wav(
        &dir,
        &common::WavSpec {
            name: "02-other.wav",
            title: Some("Ptolemy"),
            ..common::WavSpec::default()
        },
    );

    // Both start out tagged, so what the prompt offers and what it leaves alone are both visible.
    for name in ["01-track.wav", "02-other.wav"] {
        ogma::genre::set(&dir.join(name), "Ambient");
    }

    let mut player = Player::new();
    let mut pane = FilePane::at(&dir);

    // Onto the first track, then open the prompt. It starts with what the file already says, so a
    // genre that is nearly right is corrected rather than retyped.
    press(&mut pane, &mut player, KeyCode::Down);
    assert!(press(&mut pane, &mut player, KeyCode::Char('e')));
    assert!(pane.is_typing(), "the keys go into the prompt now");

    let frame = render_at(&mut pane, true, 40);
    assert!(frame.contains("genre: Ambient"), "the prompt offers what is there: {frame}");

    // Clear it and type something else.
    clear_prompt(&mut pane, &mut player);
    type_text(&mut pane, &mut player, "Shoegaze");
    press(&mut pane, &mut player, KeyCode::Enter);

    assert!(!pane.is_typing(), "the prompt is answered");
    wait_for(&mut pane, |pane| render_at(pane, true, 40).contains("genre Shoegaze"));

    assert_eq!(Song::new(dir.join("01-track.wav")).genre(), Some("Shoegaze"));
    assert_eq!(
        Song::new(dir.join("02-other.wav")).genre(),
        Some("Ambient"),
        "the row that was not highlighted is left alone"
    );

    // Asked again, the prompt now offers what was just written: the listing re-read the file.
    press(&mut pane, &mut player, KeyCode::Char('e'));
    let frame = render_at(&mut pane, true, 40);
    assert!(frame.contains("genre: Shoegaze"), "{frame}");
}

#[test]
fn a_genre_typed_onto_a_folder_reaches_every_track_under_it() {
    let dir = common::scratch_dir("pane-genre-folder");
    let album = dir.join("album");
    std::fs::create_dir_all(&album).expect("create album folder");

    for name in ["01.wav", "02.wav"] {
        common::write_wav(&album, &common::WavSpec { name, ..common::WavSpec::default() });
    }

    let mut player = Player::new();
    let mut pane = FilePane::at(&dir);

    // The folder is the first row under `..`.
    press(&mut pane, &mut player, KeyCode::Down);
    press(&mut pane, &mut player, KeyCode::Char('e'));

    // The prompt says how many it will touch, before it is answered rather than after.
    let frame = render_at(&mut pane, true, 40);
    assert!(frame.contains("genre ×2"), "{frame}");

    clear_prompt(&mut pane, &mut player);
    type_text(&mut pane, &mut player, "Jungle");
    press(&mut pane, &mut player, KeyCode::Enter);

    wait_for(&mut pane, |pane| render_at(pane, true, 40).contains("2 tracks"));

    for name in ["01.wav", "02.wav"] {
        assert_eq!(Song::new(album.join(name)).genre(), Some("Jungle"), "{name}");
    }
}

#[test]
fn an_abandoned_or_empty_genre_writes_nothing() {
    let dir = common::scratch_dir("pane-genre-cancel");
    common::write_wav(&dir, &common::WavSpec { name: "track.wav", ..common::WavSpec::default() });
    ogma::genre::set(&dir.join("track.wav"), "Ambient");

    let mut player = Player::new();
    let mut pane = FilePane::at(&dir);

    press(&mut pane, &mut player, KeyCode::Down);

    // Escaped.
    press(&mut pane, &mut player, KeyCode::Char('e'));
    type_text(&mut pane, &mut player, "Gabber");
    press(&mut pane, &mut player, KeyCode::Esc);

    assert!(!pane.is_typing());
    assert_eq!(Song::new(dir.join("track.wav")).genre(), Some("Ambient"), "untouched");

    // Answered with nothing. An empty prompt is a cancellation, not an instruction to clear the
    // genre: the two cannot be told apart, and clearing is the wrong one to guess at.
    press(&mut pane, &mut player, KeyCode::Char('e'));
    clear_prompt(&mut pane, &mut player);
    press(&mut pane, &mut player, KeyCode::Enter);

    assert!(render_at(&mut pane, true, 40).contains("no genre typed"));
    assert_eq!(Song::new(dir.join("track.wav")).genre(), Some("Ambient"), "still untouched");
}

#[test]
fn a_stream_has_no_file_to_tag() {
    let dir = common::scratch_dir("pane-genre-stream");
    let mut player = Player::new();
    let mut pane = FilePane::at(&dir);

    fake_yt_dlp();
    press(&mut pane, &mut player, KeyCode::Char('m'));
    press(&mut pane, &mut player, KeyCode::Char('m'));
    press(&mut pane, &mut player, KeyCode::Char('/'));
    type_text(&mut pane, &mut player, "aphex");
    press(&mut pane, &mut player, KeyCode::Enter);
    wait_for(&mut pane, |pane| !pane.results().is_empty());

    press(&mut pane, &mut player, KeyCode::Char('e'));

    assert!(!pane.is_typing(), "there is nothing to type into");
    assert!(render_at(&mut pane, true, 40).contains("no file to tag"));
}
