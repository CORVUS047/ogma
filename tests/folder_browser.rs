//! Folder browsing and choosing, against a scratch directory tree.
//!
//! Every test that saves redirects the config location first, so the real config file is never
//! touched.

use std::path::{Path, PathBuf};

use ogma::config::Config;
use ogma::ui::{FolderBrowser, FolderBrowserOutcome};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

/// Build a tree to browse: two folders, a hidden one, and an audio file.
fn tree(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("ogma-browse-{name}"));
    let _ = std::fs::remove_dir_all(&root);

    for folder in ["albums", "singles", ".hidden"] {
        std::fs::create_dir_all(root.join(folder)).expect("create folder");
    }
    std::fs::create_dir_all(root.join("albums").join("nested")).expect("create nested folder");
    std::fs::write(root.join("track.flac"), b"not really a flac").expect("write audio file");
    std::fs::write(root.join("notes.txt"), b"not audio").expect("write other file");

    root.canonicalize().expect("canonical root")
}

/// Redirect the config path into `root`, so saving cannot reach the user's own config.
fn redirect_config(root: &Path) {
    // SAFETY: the tests that call this do not read the environment from another thread.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", root.join("config")) };
}

fn render(browser: &mut FolderBrowser) -> String {
    let mut terminal = Terminal::new(TestBackend::new(60, 14)).expect("test terminal");

    terminal
        .draw(|frame| browser.render(frame, frame.area()))
        .expect("draw browser");

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
    browser: &mut FolderBrowser,
    config: &mut Config,
    code: KeyCode,
) -> Option<FolderBrowserOutcome> {
    browser.handle_key(KeyEvent::from(code), config)
}

#[test]
fn opens_in_the_current_working_directory() {
    let browser = FolderBrowser::new();
    let cwd = std::env::current_dir().expect("cwd").canonicalize().expect("canonical cwd");

    assert_eq!(browser.cwd(), cwd);
}

#[test]
fn lists_folders_only_and_counts_the_audio_files() {
    let root = tree("listing");
    let mut browser = FolderBrowser::at(&root);

    let frame = render(&mut browser);

    assert!(frame.contains("albums"));
    assert!(frame.contains("singles"));
    assert!(frame.contains(".."), "the parent is reachable");
    // Files are not entries: this is a folder picker.
    assert!(!frame.contains("track.flac"));
    assert!(!frame.contains("notes.txt"));
    // Hidden folders stay out of the way until asked for.
    assert!(!frame.contains(".hidden"));
    // But the audio sitting here is worth knowing about when choosing a library root.
    assert!(frame.contains("2 folders, 1 audio file here"), "{frame}");
}

#[test]
fn toggles_hidden_folders() {
    let root = tree("hidden");
    let mut config = Config::default();
    let mut browser = FolderBrowser::at(&root);

    press(&mut browser, &mut config, KeyCode::Char('.'));
    assert!(render(&mut browser).contains(".hidden"));

    press(&mut browser, &mut config, KeyCode::Char('.'));
    assert!(!render(&mut browser).contains(".hidden"));
}

#[test]
fn walks_into_and_back_out_of_folders() {
    let root = tree("walk");
    let mut config = Config::default();
    let mut browser = FolderBrowser::at(&root);

    // First entry is the parent, so the first real folder is one down.
    press(&mut browser, &mut config, KeyCode::Down);
    press(&mut browser, &mut config, KeyCode::Enter);
    assert_eq!(browser.cwd(), root.join("albums"));
    assert!(render(&mut browser).contains("nested"));

    press(&mut browser, &mut config, KeyCode::Left);
    assert_eq!(browser.cwd(), root);

    // The parent entry walks up as well.
    press(&mut browser, &mut config, KeyCode::Home);
    press(&mut browser, &mut config, KeyCode::Enter);
    assert_eq!(browser.cwd(), root.parent().expect("a parent"));
}

#[test]
fn highlight_wraps_within_the_listing() {
    let root = tree("wrap");
    let mut config = Config::default();
    let mut browser = FolderBrowser::at(&root);

    // Parent, albums, singles: three entries, so four downs come back to the start.
    for _ in 0..3 {
        press(&mut browser, &mut config, KeyCode::Down);
    }
    assert!(render(&mut browser).lines().nth(2).is_some_and(|row| row.contains('▶')));
}

/// Both halves of the save path live in one test: they share the process-wide config location, so
/// running them as separate tests would race.
#[test]
fn choosing_saves_the_folder_while_cancelling_leaves_it_alone() {
    let root = tree("choose");
    redirect_config(&root);

    let config_path = Config::config_path().expect("config path");
    assert!(config_path.starts_with(root.join("config")), "scratch config in force: {config_path:?}");

    // Backing out changes nothing, on disk or in memory.
    let mut config = Config::default();
    let mut browser = FolderBrowser::at(&root);

    assert_eq!(
        press(&mut browser, &mut config, KeyCode::Esc),
        Some(FolderBrowserOutcome::Cancel)
    );
    assert_eq!(config.default_folder(), None);
    assert!(!config_path.exists(), "cancelling writes nothing");

    // Choosing takes the folder being listed, not the highlighted entry.
    press(&mut browser, &mut config, KeyCode::Down);
    press(&mut browser, &mut config, KeyCode::Enter);
    let chosen = browser.cwd().to_path_buf();
    assert_eq!(chosen, root.join("albums"));

    let outcome = press(&mut browser, &mut config, KeyCode::Char('s'));
    assert_eq!(outcome, Some(FolderBrowserOutcome::Chosen(chosen.clone())));
    assert_eq!(config.default_folder(), Some(chosen.as_path()));

    // And it survives a restart.
    let reloaded = Config::try_load_from(&config_path).expect("reload config");
    assert_eq!(reloaded.default_folder(), Some(chosen.as_path()));
}

#[test]
fn an_unreadable_folder_is_reported_and_the_listing_stays_put() {
    let root = tree("unreadable");
    let browser = FolderBrowser::at(&root);

    // Nothing to list at a path that does not exist, so the previous listing must remain.
    let missing = root.join("albums").join("gone");
    let before = browser.cwd().to_path_buf();
    let mut replacement = FolderBrowser::at(&missing);

    assert!(render(&mut replacement).contains("No such file"), "{}", render(&mut replacement));
    assert_eq!(browser.cwd(), before);
}

// -------------------------------------------------------------------------------------- searching

#[test]
fn slash_narrows_the_folders_to_what_is_typed() {
    let root = tree("browse-search");
    // A couple more folders, so there is something to narrow.
    for extra in ["albums-live", "bootlegs"] {
        std::fs::create_dir_all(root.join(extra)).expect("create folder");
    }

    let mut config = Config::default();
    let mut browser = FolderBrowser::at(&root);

    press(&mut browser, &mut config, KeyCode::Char('/'));
    for c in "albums".chars() {
        press(&mut browser, &mut config, KeyCode::Char(c));
    }

    let frame = render(&mut browser);

    assert!(frame.contains("/albums"), "the query is shown: {frame}");
    assert!(frame.contains("albums"), "matches stay");
    assert!(frame.contains("albums-live"));
    assert!(!frame.contains("singles"), "the rest go");
    assert!(!frame.contains("bootlegs"));
    assert!(frame.contains(".."), "and the way out is always reachable");

    press(&mut browser, &mut config, KeyCode::Enter);
    assert_eq!(browser.filter(), Some("albums"));
    assert!(render(&mut browser).contains("2 found"));
}

#[test]
fn opening_a_match_leaves_the_search_behind() {
    let root = tree("browse-search-open");
    let mut config = Config::default();
    let mut browser = FolderBrowser::at(&root);

    press(&mut browser, &mut config, KeyCode::Char('/'));
    for c in "album".chars() {
        press(&mut browser, &mut config, KeyCode::Char(c));
    }
    press(&mut browser, &mut config, KeyCode::Enter);

    // Down onto the one match, then into it.
    press(&mut browser, &mut config, KeyCode::Down);
    press(&mut browser, &mut config, KeyCode::Enter);

    assert_eq!(browser.cwd(), root.join("albums"));
    assert_eq!(browser.filter(), None, "a new listing starts unnarrowed");
}

#[test]
fn esc_undoes_the_search_before_cancelling_the_screen() {
    let root = tree("browse-search-esc");
    let mut config = Config::default();
    let mut browser = FolderBrowser::at(&root);

    press(&mut browser, &mut config, KeyCode::Char('/'));
    for c in "album".chars() {
        press(&mut browser, &mut config, KeyCode::Char(c));
    }
    press(&mut browser, &mut config, KeyCode::Enter);

    // The first escape clears the search rather than leaving.
    assert_eq!(press(&mut browser, &mut config, KeyCode::Esc), None);
    assert_eq!(browser.filter(), None);
    assert!(render(&mut browser).contains("singles"), "the whole listing is back");

    // The second leaves.
    assert_eq!(
        press(&mut browser, &mut config, KeyCode::Esc),
        Some(FolderBrowserOutcome::Cancel)
    );
}

#[test]
fn the_search_is_named_in_the_keys() {
    let root = tree("browse-search-hint");
    let mut browser = FolderBrowser::at(&root);

    assert!(render(&mut browser).contains("/ find"));
}

#[test]
fn reports_of_what_an_action_did_can_be_silenced() {
    // No config redirect here: nothing in this test saves, and the redirect is process-wide, so it
    // would fight the test that does.
    let root = tree("browse-quiet");

    let mut config = Config::default();
    let mut browser = FolderBrowser::at(&root);

    // A folder that cannot be listed reports itself.
    let mut missing = FolderBrowser::at(&root.join("gone"));
    assert!(render(&mut missing).contains("No such file"));

    missing.set_messages(false);
    let quiet = render(&mut missing);
    assert!(!quiet.contains("No such file"), "silenced: {quiet}");

    // The summary of what a folder holds is not a report, and stays.
    browser.set_messages(false);
    let frame = render(&mut browser);
    assert!(frame.contains("folders"), "the summary stays: {frame}");

    // Nor is a search.
    press(&mut browser, &mut config, KeyCode::Char('/'));
    press(&mut browser, &mut config, KeyCode::Char('a'));
    assert!(render(&mut browser).contains("/a"));
}
