//! Where the application starts, which depends on whether a folder is configured.
//!
//! The config location is redirected into a scratch tree, so the user's own config and playlists are
//! untouched. All tests in this binary use the same redirect, and each writes the config it needs
//! before building the application, so they are run one at a time.

mod common;

use std::path::{Path, PathBuf};

use ogma::app::{App, ScreenKind};
use ogma::config::Config;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

/// Point the config at a scratch tree and return where the config file now lives.
fn redirect() -> PathBuf {
    let root = std::env::temp_dir().join("ogma-app-start");

    // SAFETY: every test in this binary sets the same value; they are run single-threaded.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &root) };

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

    // --- a folder that has since gone away: back to the menu ---
    std::fs::remove_dir_all(&folder).expect("remove folder");

    let app = App::new();
    assert_eq!(app.screen(), ScreenKind::Start, "the menu can point it somewhere else");
}
