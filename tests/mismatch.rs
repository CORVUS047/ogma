//! The dead end shown when the interface and the daemon speak different protocols.

use ogma::ui::{MismatchOutcome, MismatchPane};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

fn render(pane: &mut MismatchPane) -> String {
    let mut terminal = Terminal::new(TestBackend::new(70, 20)).expect("test terminal");

    terminal
        .draw(|frame| pane.render(frame, frame.area()))
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

#[test]
fn it_names_both_versions() {
    let mut pane = MismatchPane::new(2, Some(1));
    let frame = render(&mut pane);

    assert!(frame.contains("version mismatch"), "the box says what is wrong: {frame}");
    assert!(frame.contains("ogma speaks"), "{frame}");
    assert!(frame.contains("IPC version 2"), "this build's version: {frame}");
    assert!(frame.contains("ogma-daemon speaks"), "{frame}");
    assert!(frame.contains("IPC version 1"), "and the daemon's: {frame}");
}

#[test]
fn a_daemon_too_old_to_answer_is_described_rather_than_numbered() {
    let mut pane = MismatchPane::new(3, None);
    let frame = render(&mut pane);

    assert!(frame.contains("IPC version 3"), "this build still says which: {frame}");
    assert!(frame.contains("an older version"), "{frame}");
    assert!(frame.contains("too old to say which"), "and why it has no number: {frame}");
    assert!(!frame.contains("IPC version 0"), "a version it never claimed: {frame}");
}

#[test]
fn it_says_the_daemon_was_already_restarted_and_how_to_leave() {
    let mut pane = MismatchPane::new(2, Some(1));
    let frame = render(&mut pane);

    assert!(frame.contains("Restarting"), "the retry already happened: {frame}");
    assert!(frame.contains("any key closes ogma"), "{frame}");
    assert!(frame.contains("stops the daemon"), "{frame}");
}

#[test]
fn nothing_it_says_runs_off_the_edge_of_the_box() {
    // The box is a fixed width, so every line is checked whole: a sentence cut in half loses the
    // part that says what to do about it.
    for pane in [&mut MismatchPane::new(2, Some(1)), &mut MismatchPane::new(2, None)] {
        let frame = render(pane);

        for line in [
            "ogma and ogma-daemon are different builds.",
            "Restarting the daemon did not change it,",
            "so the ogma-daemon being started is not",
            "from this build.",
            "Install ogma, ogma-daemon and ogma-cmd",
            "together, into one directory.",
            "any key closes ogma and stops the daemon",
        ] {
            assert!(frame.contains(line), "cut short: {line:?}\n{frame}");
        }
    }
}

#[test]
fn any_key_at_all_closes_the_player() {
    let mut pane = MismatchPane::new(2, Some(1));

    for code in [KeyCode::Char('j'), KeyCode::Enter, KeyCode::Esc, KeyCode::F(5), KeyCode::Char(' ')] {
        assert_eq!(
            pane.handle_key(KeyEvent::from(code)),
            Some(MismatchOutcome::Quit),
            "{code:?} is as good as any other"
        );
    }
}

#[test]
fn the_versions_it_was_built_with_are_the_ones_it_reports() {
    let pane = MismatchPane::new(4, Some(2));

    assert_eq!(pane.ours(), 4);
    assert_eq!(pane.theirs(), Some(2));
}
