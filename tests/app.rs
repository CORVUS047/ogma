//! Where the application starts, which depends on whether a folder is configured.
//!
//! The config location is redirected into a scratch tree, so the user's own config and playlists are
//! untouched. All tests in this binary use the same redirect, and each writes the config it needs
//! before building the application, so they are run one at a time.
//!
//! The socket is redirected too. Building an application starts a playback daemon when none is
//! running, and a test that reached for the machine's own player would drive whatever the user is
//! listening to — and then see its queue rather than the empty one it expects.

mod common;

use std::path::{Path, PathBuf};

use ogma::app::{App, ScreenKind};
use ogma::config::Config;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

/// Point the config at a scratch tree and return where the config file now lives.
fn redirect() -> PathBuf {
    let root = std::env::temp_dir().join("ogma-app-start");

    // SAFETY: every test in this binary sets the same values; they are run single-threaded.
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", &root);
        // Kept short and directly in /tmp: a Unix socket's path has a hard length limit.
        std::env::set_var("OGMA_SOCKET", format!("/tmp/ogma-t-app-{}.sock", std::process::id()));
    }

    let path = Config::config_path().expect("a config path");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create config directory");
    }

    path
}

/// A folder with one track in it and a subfolder holding another, to stand in for a library.
fn music(name: &str) -> PathBuf {
    let dir = common::scratch_dir(name);
    common::write_wav(
        &dir,
        &common::WavSpec { name: "track.wav", title: Some("Only"), ..common::WavSpec::default() },
    );

    let album = dir.join("album");
    std::fs::create_dir_all(&album).expect("create album folder");
    common::write_wav(
        &album,
        &common::WavSpec { name: "inner.wav", title: Some("Inner"), ..common::WavSpec::default() },
    );

    dir
}

/// What the application draws, as text.
fn frame_of(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 30)).expect("test terminal");

    terminal.draw(|frame| app.draw(frame)).expect("draw");

    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;

    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Press a key, for the sequences that only care about the effect.
fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::from(code));
}

/// Write `config` to the redirected location.
fn save(config: &Config, path: &Path) {
    config.save_to(path).expect("save config");
}

/// Every case lives in one test: they share a config file, and tests in a binary run in parallel, so
/// separate tests would overwrite each other's config before the application read it.
#[test]
fn where_the_application_starts_and_how_it_comes_back() {
    let config_path = redirect();

    // --- no folder configured: the menu, where one can be chosen ---
    let config = Config::default();
    assert_eq!(config.default_folder(), None);
    save(&config, &config_path);

    let mut app = App::new();
    assert_eq!(app.screen(), ScreenKind::Start);
    assert_eq!(app.library_root(), None);

    // With nothing playing there is nothing to continue, so the first entry opens the browser.
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.screen(), ScreenKind::Browse);

    // Backing out and returning still browses: no session has appeared in the meantime.
    app.handle_key(KeyEvent::from(KeyCode::Esc));
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.screen(), ScreenKind::Browse);

    // --- a folder configured: straight to the player ---
    let folder = music("app-start-configured");
    let mut config = Config::default();
    config.set_default_folder(&folder).expect("set folder");
    save(&config, &config_path);

    let mut app = App::new();
    assert_eq!(app.screen(), ScreenKind::Play, "no menu to sit through");
    assert_eq!(app.library_root(), Some(folder.as_path()));
    // Nothing is queued: what plays is still the user's choice.
    assert!(app.player().queue().is_empty());
    assert!(app.player().current().is_none());

    // --- leaving the player and continuing back into it ---
    app.handle_key(KeyEvent::from(KeyCode::Down));
    app.handle_key(KeyEvent::from(KeyCode::Char('a')));
    assert_eq!(app.player().queue().len(), 1, "a track is queued");

    app.handle_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.screen(), ScreenKind::Start);
    // Leaving the player does not stop it, which is what makes continuing worth offering.
    assert_eq!(app.player().queue().len(), 1);

    // Continue comes first and starts highlighted, so one key undoes leaving.
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.screen(), ScreenKind::Play, "back at the player");
    assert_eq!(app.player().queue().len(), 1, "with the queue as it was");

    // Moving about the menu does not resume by itself.
    app.handle_key(KeyEvent::from(KeyCode::Esc));
    app.handle_key(KeyEvent::from(KeyCode::Down));
    assert_eq!(app.screen(), ScreenKind::Start);
    app.handle_key(KeyEvent::from(KeyCode::Up));
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.screen(), ScreenKind::Play);

    // --- the same screen comes back, not a fresh one ---
    // Walk into the album folder, so there is somewhere other than the root to return to.
    app.handle_key(KeyEvent::from(KeyCode::Char('g')));
    app.handle_key(KeyEvent::from(KeyCode::Down));
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    let inside = folder.join("album");
    assert_eq!(app.browsing_path(), Some(inside.as_path()), "the listing moved");

    app.handle_key(KeyEvent::from(KeyCode::Esc));
    app.handle_key(KeyEvent::from(KeyCode::Enter));

    assert_eq!(app.screen(), ScreenKind::Play);
    assert_eq!(
        app.browsing_path(),
        Some(inside.as_path()),
        "and came back where it was, rather than at the library root"
    );

    // --- the YouTube screen is a menu entry away, and comes straight back ---
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), ScreenKind::Youtube);

    // It opens ready to be typed into, and escape with nothing found is the way out.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.screen(), ScreenKind::Start);

    // Continue is waiting where it was, the detour having changed nothing.
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), ScreenKind::Play);
    assert_eq!(app.browsing_path(), Some(inside.as_path()));

    // --- a setting changed in the config reaches the player that was set aside ---
    assert!(frame_of(&mut app).contains("+/-"), "the keys start out listed");

    // To the menu, down to Open Config, and in. Continue, Select Folder and Search YouTube sit
    // above it.
    press(&mut app, KeyCode::Esc);
    for _ in 0..3 {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), ScreenKind::Config);

    // Down to Control hints, and off.
    for _ in 0..4 {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Enter);
    assert!(!app.config().show_control_hints(), "the setting is off");

    // Out of the config and back into the player that was waiting.
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), ScreenKind::Play);

    let frame = frame_of(&mut app);
    assert!(!frame.contains("+/-"), "the transport keys went with the setting: {frame}");
    assert!(!frame.contains("a queue"), "and so did the panes': {frame}");
    assert!(frame.contains("Queue"), "what is not a hint stays: {frame}");

    // And back on again, without reopening the library.
    press(&mut app, KeyCode::Esc);
    for _ in 0..3 {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Enter);
    for _ in 0..4 {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Enter);

    assert!(app.config().show_control_hints());
    assert!(frame_of(&mut app).contains("+/-"), "the keys are listed again");

    // --- a folder that has since gone away: back to the menu ---
    std::fs::remove_dir_all(&folder).expect("remove folder");

    let app = App::new();
    assert_eq!(app.screen(), ScreenKind::Start, "the menu can point it somewhere else");

    // The daemon this test started is this test's to stop: nothing of its own is left playing, and
    // the next run finds a socket with nobody on it, as it expects.
    let _ = ogma::ipc::send(&ogma::ipc::Command::Quit);
}
