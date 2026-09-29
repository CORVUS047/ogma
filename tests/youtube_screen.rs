//! Driving the YouTube search screen, against a test backend and a player of its own.
//!
//! No search is ever run here: results are handed to the screen directly, which is what
//! `show_results` is for. `OGMA_YTDLP` is pointed at a program that exits cleanly, so the screen's
//! "is yt-dlp installed" check answers yes on a machine that does not have it.

use std::time::Duration;

use ogma::config::Config;
use ogma::player::Player;
use ogma::ui::{YoutubeOutcome, YoutubeScreen};
use ogma::ytdl::Track;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

/// Stand in for yt-dlp with something every system has and that exits successfully, so the screen
/// believes it is installed without anything being downloaded.
fn pretend_yt_dlp_is_installed() {
    // Safe here: the tests in this file are the only thing reading it, and none of them run the
    // program for anything but its exit code.
    unsafe {
        std::env::set_var("OGMA_YTDLP", "true");
    }
}

fn screen() -> YoutubeScreen {
    pretend_yt_dlp_is_installed();

    let mut screen = YoutubeScreen::new();
    screen.show_results(results());

    screen
}

fn results() -> Vec<Track> {
    vec![
        Track {
            id: "aaaaaaaaaaa".to_string(),
            url: "https://www.youtube.com/watch?v=aaaaaaaaaaa".to_string(),
            title: "First Song".to_string(),
            uploader: Some("A Channel".to_string()),
            duration: Some(Duration::from_secs(131)),
        },
        Track {
            id: "bbbbbbbbbbb".to_string(),
            url: "https://www.youtube.com/watch?v=bbbbbbbbbbb".to_string(),
            title: "Second Song".to_string(),
            uploader: Some("Another Channel".to_string()),
            duration: Some(Duration::from_secs(245)),
        },
    ]
}

fn render(screen: &mut YoutubeScreen) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 14)).expect("test terminal");

    terminal
        .draw(|frame| screen.render(frame, frame.area()))
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

fn press(
    screen: &mut YoutubeScreen,
    player: &mut Player,
    code: KeyCode,
) -> Option<YoutubeOutcome> {
    screen.handle_key(KeyEvent::from(code), player, &Config::default())
}

#[test]
fn results_list_with_their_uploader_and_length() {
    let mut screen = screen();
    let frame = render(&mut screen);

    assert!(frame.contains("First Song"), "{frame}");
    assert!(frame.contains("A Channel"), "{frame}");
    assert!(frame.contains("2:11"), "the length reads as minutes and seconds: {frame}");
    assert!(frame.contains("youtube"), "the box says what it is: {frame}");
}

#[test]
fn the_screen_opens_ready_to_be_typed_into() {
    pretend_yt_dlp_is_installed();

    let mut screen = YoutubeScreen::new();
    assert!(screen.is_typing(), "opening it is asking to search");

    let mut player = Player::new();
    press(&mut screen, &mut player, KeyCode::Char('h'));
    press(&mut screen, &mut player, KeyCode::Char('i'));

    let frame = render(&mut screen);
    assert!(frame.contains("hi"), "what is typed shows: {frame}");
}

#[test]
fn escape_leaves_an_empty_screen_and_the_query_of_a_full_one() {
    pretend_yt_dlp_is_installed();

    let mut player = Player::new();
    let mut empty = YoutubeScreen::new();

    assert_eq!(
        press(&mut empty, &mut player, KeyCode::Esc),
        Some(YoutubeOutcome::Close),
        "with nothing found, escape is a way out"
    );

    let mut full = screen();
    press(&mut full, &mut player, KeyCode::Char('/'));
    assert!(full.is_typing());

    assert_eq!(press(&mut full, &mut player, KeyCode::Esc), None, "it leaves the query first");
    assert!(!full.is_typing());

    assert_eq!(
        press(&mut full, &mut player, KeyCode::Esc),
        Some(YoutubeOutcome::Close),
        "and the screen after"
    );
}

#[test]
fn enter_streams_the_highlighted_result() {
    let mut screen = screen();
    let mut player = Player::new();

    press(&mut screen, &mut player, KeyCode::Enter);

    let current = player.current().expect("something is playing");
    assert!(current.is_stream(), "it plays from the network");
    assert_eq!(current.title(), Some("First Song"));
    assert_eq!(current.uri(), "https://www.youtube.com/watch?v=aaaaaaaaaaa");

    assert!(render(&mut screen).contains("streaming First Song"));
}

#[test]
fn a_queues_what_is_highlighted_when_nothing_is_marked() {
    let mut screen = screen();
    let mut player = Player::new();

    press(&mut screen, &mut player, KeyCode::Down);
    press(&mut screen, &mut player, KeyCode::Char('a'));

    assert_eq!(player.queue().len(), 1);
    assert_eq!(player.queue()[0].title(), Some("Second Song"));
}

#[test]
fn marking_several_makes_one_key_act_on_all_of_them() {
    let mut screen = screen();
    let mut player = Player::new();

    press(&mut screen, &mut player, KeyCode::Char(' '));
    press(&mut screen, &mut player, KeyCode::Down);
    press(&mut screen, &mut player, KeyCode::Char(' '));

    assert_eq!(screen.chosen().len(), 2, "both are ticked");
    assert!(render(&mut screen).contains('●'), "and the ticks show");

    press(&mut screen, &mut player, KeyCode::Char('a'));

    assert_eq!(player.queue().len(), 2);
    assert_eq!(player.queue()[0].title(), Some("First Song"));
    assert_eq!(player.queue()[1].title(), Some("Second Song"));

    // Queueing clears the ticks, so the next key does not queue them again by accident.
    assert_eq!(screen.chosen().len(), 1, "only what is highlighted remains chosen");
}

#[test]
fn m_marks_everything_and_then_clears_it() {
    let mut screen = screen();
    let mut player = Player::new();

    press(&mut screen, &mut player, KeyCode::Char('m'));
    assert_eq!(screen.chosen().len(), 2);

    press(&mut screen, &mut player, KeyCode::Char('m'));
    assert_eq!(screen.chosen().len(), 1, "back to the highlighted one alone");
}

#[test]
fn the_selection_wraps_at_both_ends() {
    let mut screen = screen();
    let mut player = Player::new();

    press(&mut screen, &mut player, KeyCode::Up);
    press(&mut screen, &mut player, KeyCode::Char('a'));

    assert_eq!(player.queue()[0].title(), Some("Second Song"), "up from the top is the bottom");
}

#[test]
fn the_key_reminders_can_be_turned_off() {
    let mut screen = screen();

    assert!(render(&mut screen).contains("enter stream"), "shown by default");

    screen.set_hints(false);
    let frame = render(&mut screen);

    assert!(!frame.contains("enter stream"), "{frame}");
    assert!(frame.contains("First Song"), "the results are not a hint: {frame}");
}

#[test]
fn the_reports_of_what_happened_can_be_silenced() {
    let mut screen = screen();
    let mut player = Player::new();

    screen.set_messages(false);
    press(&mut screen, &mut player, KeyCode::Enter);

    let frame = render(&mut screen);
    assert!(!frame.contains("streaming"), "{frame}");
}

#[test]
fn downloads_go_where_the_application_says() {
    let mut screen = screen();
    let folder = std::env::temp_dir().join("ogma-tests-youtube-downloads");

    screen.download_into(folder.clone());

    assert_eq!(screen.folder(), folder);
}
