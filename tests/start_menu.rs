//! Rendering and navigation checks for the start menu, against a test backend.

use ogma::ui::{StartMenu, StartMenuChoice};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

fn render(menu: &mut StartMenu) -> String {
    let mut terminal = Terminal::new(TestBackend::new(44, 15)).expect("test terminal");

    terminal
        .draw(|frame| menu.render(frame, frame.area()))
        .expect("draw menu");

    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;

    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn press(menu: &mut StartMenu, code: KeyCode) -> Option<StartMenuChoice> {
    menu.handle_key(KeyEvent::from(code))
}

#[test]
fn stacks_the_three_entries_vertically() {
    let mut menu = StartMenu::new();
    let frame = render(&mut menu);

    let rows: Vec<&str> = frame.lines().collect();

    let folder = rows.iter().position(|row| row.contains("Select Folder")).expect("folder row");
    let config = rows.iter().position(|row| row.contains("Open Config")).expect("config row");
    let quit = rows.iter().position(|row| row.contains("Quit")).expect("quit row");

    // Each on its own row, in order, top to bottom.
    assert_eq!(config, folder + 1);
    assert_eq!(quit, config + 1);

    assert!(frame.contains("ogma"), "the box is titled");
    // The first entry starts highlighted.
    assert!(rows[folder].contains('▶'));
}

#[test]
fn the_key_reminders_can_be_turned_off() {
    let mut menu = StartMenu::new();

    assert!(render(&mut menu).contains("enter select"), "shown by default");

    menu.set_hints(false);
    let frame = render(&mut menu);

    assert!(!frame.contains("enter select"), "{frame}");
    // The entries themselves are not hints.
    assert!(frame.contains("Select Folder"));
    assert!(frame.contains("Scan a folder for music"), "nor is the entry's description");

    menu.set_hints(true);
    assert!(render(&mut menu).contains("enter select"));
}

#[test]
fn moves_the_highlight_and_wraps_around() {
    let mut menu = StartMenu::new();
    assert_eq!(menu.selected(), StartMenuChoice::SelectFolder);

    press(&mut menu, KeyCode::Down);
    assert_eq!(menu.selected(), StartMenuChoice::OpenConfig);

    press(&mut menu, KeyCode::Char('j'));
    assert_eq!(menu.selected(), StartMenuChoice::Quit);

    // Past the last entry comes the first.
    press(&mut menu, KeyCode::Down);
    assert_eq!(menu.selected(), StartMenuChoice::SelectFolder);

    // And backwards from the first comes the last.
    press(&mut menu, KeyCode::Up);
    assert_eq!(menu.selected(), StartMenuChoice::Quit);
}

#[test]
fn commits_on_enter_and_quits_on_q() {
    let mut menu = StartMenu::new();

    assert_eq!(press(&mut menu, KeyCode::Down), None, "moving does not commit");
    assert_eq!(press(&mut menu, KeyCode::Enter), Some(StartMenuChoice::OpenConfig));

    // q quits from anywhere, whatever is highlighted.
    assert_eq!(press(&mut menu, KeyCode::Char('q')), Some(StartMenuChoice::Quit));
}

#[test]
fn shows_a_hint_for_the_highlighted_entry() {
    let mut menu = StartMenu::new();
    assert!(render(&mut menu).contains("Scan a folder for music"));

    press(&mut menu, KeyCode::Down);
    assert!(render(&mut menu).contains("Audio output, theme, keys"));
}

// ------------------------------------------------------------------ coming back from the player

#[test]
fn continue_is_offered_only_when_there_is_something_to_go_back_to() {
    let mut fresh = StartMenu::new();
    let frame = render(&mut fresh);

    assert!(!frame.contains("Continue"), "nothing is playing yet: {frame}");
    assert_eq!(fresh.entries().len(), 3);

    let mut resumable = StartMenu::with_continue(true);
    let frame = render(&mut resumable);
    let rows: Vec<&str> = frame.lines().collect();

    let resume = rows.iter().position(|row| row.contains("Continue")).expect("continue row");
    let folder = rows.iter().position(|row| row.contains("Select Folder")).expect("folder row");

    assert!(resume < folder, "it comes first: {frame}");
    assert!(rows[resume].contains('▶'), "and starts highlighted, so one key goes back");
    assert_eq!(resumable.selected(), StartMenuChoice::Continue);
    assert!(frame.contains("Back to what is playing"), "with its own hint");
}

#[test]
fn the_extra_entry_does_not_disturb_the_others() {
    let mut menu = StartMenu::with_continue(true);
    let mut player_only = StartMenu::new();

    // Four entries, and the highlight still wraps through all of them.
    assert_eq!(menu.entries().len(), 4);

    for expected in [
        StartMenuChoice::SelectFolder,
        StartMenuChoice::OpenConfig,
        StartMenuChoice::Quit,
        StartMenuChoice::Continue,
    ] {
        press(&mut menu, KeyCode::Down);
        assert_eq!(menu.selected(), expected);
    }

    press(&mut menu, KeyCode::Up);
    assert_eq!(menu.selected(), StartMenuChoice::Quit, "and backwards too");

    // The menu without it is unchanged.
    press(&mut player_only, KeyCode::Up);
    assert_eq!(player_only.selected(), StartMenuChoice::Quit);
}

#[test]
fn choosing_continue_reports_it() {
    let mut menu = StartMenu::with_continue(true);

    assert_eq!(press(&mut menu, KeyCode::Enter), Some(StartMenuChoice::Continue));
}
