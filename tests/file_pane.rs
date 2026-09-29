//! Browsing files and putting them in the queue.

mod common;

use std::path::{Path, PathBuf};

use ogma::player::Player;
use ogma::ui::FilePane;
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
