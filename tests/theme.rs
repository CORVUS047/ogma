//! Reading and writing `theme.toml`, and the switch between it and the terminal's palette.

mod common;

use ogma::config::Config;
use ogma::theme::{Theme, ThemeError};
use ratatui::style::Color;

#[test]
fn a_missing_theme_file_is_written_with_the_defaults() {
    let dir = common::scratch_dir("theme-missing");
    let path = dir.join("nested").join("theme.toml");

    let theme = Theme::try_load_from(&path).expect("load with no file present");

    assert_eq!(theme, Theme::default());
    assert!(path.exists(), "there is something to edit");

    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(text.contains("accent = \"cyan\""), "{text}");
    assert!(text.contains("muted = \"reset\""), "the terminal's own colours, spelled out: {text}");

    // What was written reads back the same.
    assert_eq!(Theme::try_load_from(&path).expect("reload"), theme);
}

#[test]
fn colours_can_be_named_indexed_or_written_in_hex() {
    let dir = common::scratch_dir("theme-colours");
    let path = dir.join("theme.toml");

    std::fs::write(
        &path,
        "accent = \"#7aa2f7\"\nborder = \"104\"\nsuccess = \"light-green\"\n",
    )
    .expect("write theme");

    let theme = Theme::try_load_from(&path).expect("load");

    assert_eq!(theme.accent, Color::Rgb(0x7a, 0xa2, 0xf7));
    assert_eq!(theme.border, Color::Indexed(104));
    assert_eq!(theme.success, Color::LightGreen);
    // Anything the file leaves out keeps its default.
    assert_eq!(theme.error, Theme::default().error);
}

#[test]
fn a_colour_that_is_not_a_colour_is_reported_and_names_the_field() {
    let dir = common::scratch_dir("theme-bad-colour");
    let path = dir.join("theme.toml");
    let text = "accent = \"banana\"\n";
    std::fs::write(&path, text).expect("write theme");

    let err = Theme::try_load_from(&path).expect_err("not a colour");

    assert!(matches!(err, ThemeError::Color { .. }));
    assert!(err.to_string().contains("accent"), "{err}");
    assert!(err.to_string().contains("banana"));

    // The file is left as it was, so the mistake can be corrected rather than overwritten.
    assert_eq!(std::fs::read_to_string(&path).expect("reread"), text);
}

#[test]
fn a_broken_file_is_reported_rather_than_replaced() {
    let dir = common::scratch_dir("theme-broken");
    let path = dir.join("theme.toml");
    let text = "this is not toml";
    std::fs::write(&path, text).expect("write theme");

    assert!(matches!(Theme::try_load_from(&path), Err(ThemeError::Parse(_))));
    assert_eq!(std::fs::read_to_string(&path).expect("reread"), text);
}

#[test]
fn an_unknown_field_is_refused_rather_than_ignored() {
    let dir = common::scratch_dir("theme-unknown");
    let path = dir.join("theme.toml");
    std::fs::write(&path, "acent = \"cyan\"\n").expect("write theme");

    // A typo that was quietly ignored would look like the theme not working.
    assert!(matches!(Theme::try_load_from(&path), Err(ThemeError::Parse(_))));
}

#[test]
fn the_switch_decides_whether_the_file_is_read_at_all() {
    // Off: the terminal's palette, whatever is on disk.
    assert_eq!(Theme::load(false), Theme::default());

    // The theme file sits beside the config.
    let theme_path = Theme::path().expect("a theme path");
    let config_path = Config::config_path().expect("a config path");

    assert_eq!(theme_path.parent(), config_path.parent());
    assert!(theme_path.ends_with("theme.toml"));
}

#[test]
fn the_config_switch_is_off_until_asked_for() {
    assert!(!Config::default().custom_theme(), "the terminal's own colours by default");

    let dir = common::scratch_dir("theme-config");
    let path = dir.join("config.toml");

    // A config from before the setting existed keeps the terminal palette.
    std::fs::write(&path, "master_volume = 0.4\n").expect("write config");
    assert!(!Config::try_load_from(&path).expect("load").custom_theme());

    let mut config = Config::default();
    assert!(config.toggle_custom_theme());
    config.save_to(&path).expect("save");

    let written = std::fs::read_to_string(&path).expect("read back");
    assert!(written.contains("custom_theme = true"), "got {written}");
    assert!(Config::try_load_from(&path).expect("reload").custom_theme());

    config.set_custom_theme(false);
    assert!(!config.custom_theme());
}
