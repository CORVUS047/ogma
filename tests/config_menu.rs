//! Editing behaviour of the config screen, against a test backend.

use ogma::config::Config;
use ogma::ui::{ConfigMenu, ConfigMenuOutcome};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn render(menu: &mut ConfigMenu, config: &Config) -> String {
    let mut terminal = Terminal::new(TestBackend::new(70, 20)).expect("test terminal");

    terminal
        .draw(|frame| menu.render(frame, frame.area(), config))
        .expect("draw config screen");

    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;

    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn press(menu: &mut ConfigMenu, config: &mut Config, code: KeyCode) -> Option<ConfigMenuOutcome> {
    menu.handle_key(KeyEvent::from(code), config)
}

fn type_text(menu: &mut ConfigMenu, config: &mut Config, text: &str) {
    for c in text.chars() {
        press(menu, config, KeyCode::Char(c));
    }
}

#[test]
fn lists_both_settings_with_their_values() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    let frame = render(&mut menu, &config);
    let rows: Vec<&str> = frame.lines().collect();

    let volume = rows.iter().position(|r| r.contains("Master volume")).expect("volume row");
    let folder = rows.iter().position(|r| r.contains("Default folder")).expect("folder row");

    assert_eq!(folder, volume + 1, "stacked vertically, in order");
    assert!(rows[volume].contains("50%"), "the fader is shown: {}", rows[volume]);
    assert!(
        rows[volume].contains("-10.0 dB"),
        "and what it does, in decibels: {}",
        rows[volume]
    );
    assert!(rows[folder].contains("not set"));
    assert!(rows[volume].contains('▶'), "the first row starts highlighted");

    // Nothing has been edited, so nothing is pending.
    assert!(!menu.unsaved());
    assert!(!frame.contains("unsaved"));

    config.set_master_volume(0.0);
    let muted = render(&mut menu, &config);
    assert!(muted.contains("0%"));
    assert!(muted.contains("muted"), "the bottom of the fader is silence, not a decibel figure");
}

#[test]
fn the_metadata_filling_row_toggles_and_shows_its_state() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    let frame = render(&mut menu, &config);
    let rows: Vec<&str> = frame.lines().collect();

    let folder = rows.iter().position(|r| r.contains("Default folder")).expect("folder row");
    let fill = rows.iter().position(|r| r.contains("Fill metadata")).expect("fill row");

    assert_eq!(fill, folder + 1, "the third row, under the folder");
    assert!(rows[fill].contains("off"), "off by default: {}", rows[fill]);

    // Down twice to reach it, then toggle.
    press(&mut menu, &mut config, KeyCode::Down);
    press(&mut menu, &mut config, KeyCode::Down);
    press(&mut menu, &mut config, KeyCode::Enter);

    assert!(config.auto_fill_metadata());
    assert!(menu.unsaved(), "the change is pending a write");

    let on = render(&mut menu, &config);
    assert!(on.contains("on"), "{on}");
    assert!(on.contains("artwork"), "it says what it fills in: {on}");

    press(&mut menu, &mut config, KeyCode::Char(' '));
    assert!(!config.auto_fill_metadata(), "space toggles it too");
}

#[test]
fn the_control_hints_row_toggles_and_shows_its_state() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    let frame = render(&mut menu, &config);
    let rows: Vec<&str> = frame.lines().collect();

    let online = rows.iter().position(|r| r.contains("Look online")).expect("online row");
    let genres = rows.iter().position(|r| r.contains("Genre lookup")).expect("genres row");
    let hints = rows.iter().position(|r| r.contains("Control hints")).expect("hints row");

    assert_eq!(genres, online + 1, "the genre lookup sits with the other online switch");
    assert_eq!(hints, genres + 1, "the sixth row");
    assert!(rows[hints].contains("shown"), "shown by default: {}", rows[hints]);

    // Five downs to reach it, then toggle.
    for _ in 0..5 {
        press(&mut menu, &mut config, KeyCode::Down);
    }
    press(&mut menu, &mut config, KeyCode::Enter);

    assert!(!config.show_control_hints());
    assert!(menu.unsaved());
    assert!(render(&mut menu, &config).contains("hidden"));

    press(&mut menu, &mut config, KeyCode::Char(' '));
    assert!(config.show_control_hints(), "space toggles it too");
}

#[test]
fn the_hide_messages_row_toggles_and_quietens_the_screen() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    let frame = render(&mut menu, &config);
    let rows: Vec<&str> = frame.lines().collect();

    let hints = rows.iter().position(|r| r.contains("Control hints")).expect("hints row");
    let messages = rows.iter().position(|r| r.contains("Hide messages")).expect("messages row");

    assert_eq!(messages, hints + 1, "the seventh row");
    assert!(rows[messages].contains("off"), "off by default: {}", rows[messages]);

    // Six downs to reach it, then toggle.
    for _ in 0..6 {
        press(&mut menu, &mut config, KeyCode::Down);
    }
    press(&mut menu, &mut config, KeyCode::Enter);

    assert!(config.hide_status_messages());
    assert!(menu.unsaved());
    assert!(render(&mut menu, &config).contains("on"));

    press(&mut menu, &mut config, KeyCode::Char(' '));
    assert!(!config.hide_status_messages(), "space toggles it too");
}

#[test]
fn the_screens_own_messages_can_be_silenced() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    press(&mut menu, &mut config, KeyCode::Right);
    assert!(render(&mut menu, &config).contains("Master volume 55%"), "it reports the change");

    menu.set_messages(false);
    let quiet = render(&mut menu, &config);
    assert!(!quiet.contains("Master volume 55%"), "and then it does not: {quiet}");

    // The unsaved marker is state rather than a report, so a save is still visibly a save.
    assert!(quiet.contains("unsaved"));
    assert!(quiet.contains("55%"), "nor is the value itself a report");
}

#[test]
fn arrows_adjust_the_master_volume_in_steps() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    press(&mut menu, &mut config, KeyCode::Right);
    assert_eq!(*config.get_master_volume(), 0.55);

    press(&mut menu, &mut config, KeyCode::Left);
    press(&mut menu, &mut config, KeyCode::Char('h'));
    assert_eq!(*config.get_master_volume(), 0.45);

    assert!(menu.unsaved(), "an edit is pending a write");
    assert!(render(&mut menu, &config).contains("unsaved"));
}

#[test]
fn volume_stops_at_the_ends_of_its_range() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    for _ in 0..40 {
        press(&mut menu, &mut config, KeyCode::Right);
    }
    assert_eq!(*config.get_master_volume(), 1.0);

    for _ in 0..40 {
        press(&mut menu, &mut config, KeyCode::Left);
    }
    assert_eq!(*config.get_master_volume(), 0.0);
}

#[test]
fn the_master_fader_reads_out_in_decibels_through_the_loudness_curve() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    // Half the fader is half the perceived loudness, which is ten decibels down.
    assert!((config.master_gain() - 0.3162).abs() < 0.001, "got {}", config.master_gain());

    press(&mut menu, &mut config, KeyCode::Char('9'));
    assert_eq!(config.master_gain(), 1.0, "the top of the fader does not attenuate");
    let frame = render(&mut menu, &config);
    assert!(frame.contains("100%") && frame.contains("0.0 dB"), "{frame}");

    press(&mut menu, &mut config, KeyCode::Char('0'));
    assert_eq!(config.master_gain(), 0.0);
    assert!(render(&mut menu, &config).contains("muted"));

    // The status line reports both.
    press(&mut menu, &mut config, KeyCode::Right);
    assert!(render(&mut menu, &config).contains("Master volume 5% ("), "{}", render(&mut menu, &config));
}

#[test]
fn digits_set_the_volume_directly() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    press(&mut menu, &mut config, KeyCode::Char('0'));
    assert_eq!(*config.get_master_volume(), 0.0);

    press(&mut menu, &mut config, KeyCode::Char('9'));
    assert_eq!(*config.get_master_volume(), 1.0);
}

#[test]
fn volume_keys_do_nothing_on_the_folder_row() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    press(&mut menu, &mut config, KeyCode::Down);
    press(&mut menu, &mut config, KeyCode::Right);
    press(&mut menu, &mut config, KeyCode::Char('5'));

    assert_eq!(*config.get_master_volume(), 0.5);
    assert!(!menu.unsaved());
}

#[test]
fn typing_a_real_folder_sets_it() {
    let dir = std::env::temp_dir().join("ogma-menu-folder");
    std::fs::create_dir_all(&dir).expect("create dir");

    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    press(&mut menu, &mut config, KeyCode::Down);
    press(&mut menu, &mut config, KeyCode::Enter);

    // While editing, the row shows what is being typed.
    type_text(&mut menu, &mut config, dir.to_str().expect("utf-8 path"));
    assert!(render(&mut menu, &config).contains("ogma-menu-folder"));

    press(&mut menu, &mut config, KeyCode::Enter);
    assert_eq!(config.default_folder(), Some(dir.as_path()));
    assert!(render(&mut menu, &config).contains("Default folder set"));
}

#[test]
fn a_folder_that_is_not_a_directory_is_rejected_and_reported() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    press(&mut menu, &mut config, KeyCode::Down);
    press(&mut menu, &mut config, KeyCode::Enter);
    type_text(&mut menu, &mut config, "/definitely/not/here");
    press(&mut menu, &mut config, KeyCode::Enter);

    assert_eq!(config.default_folder(), None);
    assert!(render(&mut menu, &config).contains("not a directory"));
}

#[test]
fn editing_can_be_corrected_and_abandoned() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    press(&mut menu, &mut config, KeyCode::Down);
    press(&mut menu, &mut config, KeyCode::Enter);
    type_text(&mut menu, &mut config, "/abc");
    press(&mut menu, &mut config, KeyCode::Backspace);
    assert!(render(&mut menu, &config).contains("/ab"));

    menu.handle_key(
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        &mut config,
    );
    assert!(!render(&mut menu, &config).contains("/ab"), "^U clears the line");

    type_text(&mut menu, &mut config, "/tmp");
    press(&mut menu, &mut config, KeyCode::Esc);
    assert_eq!(config.default_folder(), None, "esc abandons the edit");
    assert!(render(&mut menu, &config).contains("cancelled"));
}

#[test]
fn a_set_folder_can_be_cleared() {
    let dir = std::env::temp_dir().join("ogma-menu-clear");
    std::fs::create_dir_all(&dir).expect("create dir");

    let mut config = Config::default();
    config.set_default_folder(&dir).expect("set folder");

    let mut menu = ConfigMenu::new();
    press(&mut menu, &mut config, KeyCode::Down);
    press(&mut menu, &mut config, KeyCode::Char('d'));

    assert_eq!(config.default_folder(), None);
    assert!(render(&mut menu, &config).contains("cleared"));
}

#[test]
fn esc_closes_the_screen() {
    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    assert_eq!(press(&mut menu, &mut config, KeyCode::Down), None);
    assert_eq!(
        press(&mut menu, &mut config, KeyCode::Esc),
        Some(ConfigMenuOutcome::Close)
    );
}

#[test]
fn saving_writes_the_edited_values_to_disk() {
    // Point the standard config path at a scratch directory for this process.
    let home = std::env::temp_dir().join("ogma-menu-save");
    let _ = std::fs::remove_dir_all(&home);
    // SAFETY: single-threaded test; no other thread reads the environment here.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &home) };

    let path = Config::config_path().expect("config path");
    assert!(path.starts_with(&home), "the scratch path is in force: {path:?}");

    let mut config = Config::default();
    let mut menu = ConfigMenu::new();

    press(&mut menu, &mut config, KeyCode::Char('9'));
    assert!(menu.unsaved());

    press(&mut menu, &mut config, KeyCode::Char('s'));
    assert!(!menu.unsaved(), "saving clears the pending flag");
    assert!(render(&mut menu, &config).contains("Saved"));

    let written = std::fs::read_to_string(&path).expect("read saved config");
    assert!(written.contains("master_volume"), "got {written}");
    assert_eq!(Config::try_load_from(&path).expect("reload"), config);

    // Leaving the screen with pending edits writes them out too.
    press(&mut menu, &mut config, KeyCode::Char('0'));
    assert_eq!(
        press(&mut menu, &mut config, KeyCode::Esc),
        Some(ConfigMenuOutcome::Close)
    );
    assert!(!menu.unsaved());
    assert_eq!(*Config::try_load_from(&path).expect("reload").get_master_volume(), 0.0);
}
