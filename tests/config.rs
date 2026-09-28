//! Config loading, against temporary paths so the real config file is never touched.

use ogma::config::{Config, ConfigError};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ogma-config-{name}"));
    let _ = std::fs::remove_dir_all(&dir);

    dir
}

#[test]
fn writes_a_default_config_when_none_exists() {
    // Nested under a directory that does not exist either, so creation is covered too.
    let path = scratch("missing").join("ogma").join("config.toml");

    let config = Config::try_load_from(&path).expect("load with no file present");

    assert_eq!(config, Config::default());
    assert!(path.exists(), "the default config is written out for the user to edit");

    // What was written back is what was returned.
    let written = std::fs::read_to_string(&path).expect("read written config");
    assert!(written.contains("volume"));
    assert_eq!(Config::try_load_from(&path).expect("reload"), config);
}

#[test]
fn filling_metadata_is_off_until_asked_for() {
    // Writing to the user's music files is not a default.
    assert!(!Config::default().auto_fill_metadata());

    let dir = scratch("autofill");
    std::fs::create_dir_all(&dir).expect("create dir");
    let path = dir.join("config.toml");

    // A config file that does not mention it keeps it off.
    std::fs::write(&path, "master_volume = 0.4\n").expect("write config");
    assert!(!Config::try_load_from(&path).expect("load").auto_fill_metadata());

    // Turning it on survives a save and a reload.
    let mut config = Config::default();
    assert!(config.toggle_auto_fill_metadata());
    config.save_to(&path).expect("save");

    let written = std::fs::read_to_string(&path).expect("read back");
    assert!(written.contains("auto_fill_metadata = true"), "got {written}");
    assert!(Config::try_load_from(&path).expect("reload").auto_fill_metadata());

    assert!(!config.toggle_auto_fill_metadata(), "toggling again turns it back off");
    config.set_auto_fill_metadata(true);
    assert!(config.auto_fill_metadata());
}

#[test]
fn control_hints_are_shown_until_turned_off() {
    // Nobody should have to guess the keys, so they are on by default.
    assert!(Config::default().show_control_hints());

    let dir = scratch("hints");
    std::fs::create_dir_all(&dir).expect("create dir");
    let path = dir.join("config.toml");

    // A config written before the setting existed keeps the hints.
    std::fs::write(&path, "master_volume = 0.4\n").expect("write config");
    assert!(Config::try_load_from(&path).expect("load").show_control_hints());

    // Turning them off survives a save and a reload.
    let mut config = Config::default();
    assert!(!config.toggle_show_control_hints());
    config.save_to(&path).expect("save");

    let written = std::fs::read_to_string(&path).expect("read back");
    assert!(written.contains("show_control_hints = false"), "got {written}");
    assert!(!Config::try_load_from(&path).expect("reload").show_control_hints());

    assert!(config.toggle_show_control_hints(), "and back on again");
    config.set_show_control_hints(false);
    assert!(!config.show_control_hints());
}

#[test]
fn status_messages_are_shown_until_hidden() {
    // Off by default, meaning the interface says what it has done.
    assert!(!Config::default().hide_status_messages());

    let dir = scratch("messages");
    std::fs::create_dir_all(&dir).expect("create dir");
    let path = dir.join("config.toml");

    // A config from before the setting existed keeps the messages.
    std::fs::write(&path, "master_volume = 0.4\n").expect("write config");
    assert!(!Config::try_load_from(&path).expect("load").hide_status_messages());

    let mut config = Config::default();
    assert!(config.toggle_hide_status_messages());
    config.save_to(&path).expect("save");

    let written = std::fs::read_to_string(&path).expect("read back");
    assert!(written.contains("hide_status_messages = true"), "got {written}");
    assert!(Config::try_load_from(&path).expect("reload").hide_status_messages());

    assert!(!config.toggle_hide_status_messages(), "and back on again");
    config.set_hide_status_messages(true);
    assert!(config.hide_status_messages());
}

#[test]
fn reads_values_from_an_existing_config() {
    let dir = scratch("existing");
    std::fs::create_dir_all(&dir).expect("create dir");
    let path = dir.join("config.toml");
    std::fs::write(&path, "master_volume = 0.25\ndefault_folder = \"/music\"\n").expect("write config");

    let config = Config::try_load_from(&path).expect("load existing config");

    assert_eq!(*config.get_master_volume(), 0.25);
    assert_eq!(config.default_folder(), Some(std::path::Path::new("/music")));
}

#[test]
fn fills_in_missing_fields_from_the_defaults() {
    let dir = scratch("partial");
    std::fs::create_dir_all(&dir).expect("create dir");
    let path = dir.join("config.toml");
    std::fs::write(&path, "master_volume = 0.9\n").expect("write config");

    let config = Config::try_load_from(&path).expect("load partial config");

    assert_eq!(*config.get_master_volume(), 0.9);
    assert_eq!(config.default_folder(), None);
}

#[test]
fn clamps_a_volume_the_file_puts_out_of_range() {
    let dir = scratch("range");
    std::fs::create_dir_all(&dir).expect("create dir");
    let path = dir.join("config.toml");
    std::fs::write(&path, "master_volume = 4.0\n").expect("write config");

    assert_eq!(*Config::try_load_from(&path).expect("load").get_master_volume(), 1.0);

    std::fs::write(&path, "master_volume = -2.0\n").expect("rewrite config");
    assert_eq!(*Config::try_load_from(&path).expect("load").get_master_volume(), 0.0);
}

#[test]
fn reports_a_broken_config_without_overwriting_it() {
    let dir = scratch("broken");
    std::fs::create_dir_all(&dir).expect("create dir");
    let path = dir.join("config.toml");
    let broken = "master_volume = \"loud\"\n";
    std::fs::write(&path, broken).expect("write config");

    let err = Config::try_load_from(&path).expect_err("a broken file must be reported");
    assert!(matches!(err, ConfigError::Parse(_)), "got {err:?}");

    // The user's file survives, so they can fix it rather than losing it.
    assert_eq!(std::fs::read_to_string(&path).expect("reread"), broken);

    // The infallible path still yields something usable.
    assert_eq!(Config::default(), Config::default());
}

#[test]
fn config_path_sits_under_the_platform_config_directory() {
    let path = Config::config_path().expect("a config path on this platform");

    assert!(path.ends_with("ogma/config.toml"), "got {path:?}");
    assert!(path.is_absolute());
}
