//! User configuration, read from and written to the platform's standard config location.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file the config lives in, inside the platform's config directory.
const FILE_NAME: &str = "config.toml";

/// The directory the config lives in, inside the platform's config directory.
const APP_DIR: &str = "ogma";

#[derive(Debug)]
pub enum ConfigError {
    NotDirectory,
    FailedToConvertPathToString,
    /// The platform did not report a config directory to put the file in.
    NoConfigDirectory,
    /// Reading or writing the config file failed.
    Io(io::Error),
    /// The config file exists but is not valid TOML, or has a field of the wrong type.
    Parse(toml::de::Error),
    /// The config could not be rendered as TOML.
    Serialize(toml::ser::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NotDirectory => write!(f, "not a directory"),
            ConfigError::FailedToConvertPathToString => write!(f, "path is not valid UTF-8"),
            ConfigError::NoConfigDirectory => {
                write!(f, "no config directory available on this platform")
            }
            ConfigError::Io(err) => write!(f, "{err}"),
            ConfigError::Parse(err) => write!(f, "invalid config file: {err}"),
            ConfigError::Serialize(err) => write!(f, "cannot write config: {err}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io(err) => Some(err),
            ConfigError::Parse(err) => Some(err),
            ConfigError::Serialize(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for ConfigError {
    fn from(err: io::Error) -> Self {
        ConfigError::Io(err)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(err: toml::de::Error) -> Self {
        ConfigError::Parse(err)
    }
}

impl From<toml::ser::Error> for ConfigError {
    fn from(err: toml::ser::Error) -> Self {
        ConfigError::Serialize(err)
    }
}

// Missing fields fall back to `Default`, so a config file written by an older version, or trimmed
// down by hand, still loads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Master fader position, 0.0 to 1.0, applied to everything the player plays.
    ///
    /// Stored as the fader position rather than an amplitude, so the number means the same thing
    /// here as it does on screen. [`crate::volume::gain`] turns it into the gain the audio gets.
    #[serde(alias = "volume")]
    master_volume: f32,
    default_folder: Option<String>,
    /// Whether to fill in metadata a file is missing, where it can be found on disk.
    ///
    /// Off by default: it writes to the user's music files, which is not something to do unasked.
    /// Only artwork is filled in so far — see [`crate::autofill`].
    auto_fill_metadata: bool,
    /// Whether filling in metadata may ask the internet, rather than only looking on disk.
    ///
    /// Off by default, and separate from [`Config::auto_fill_metadata`] on purpose: reading a file
    /// already on the machine and sending an album title to a third party are different acts, and
    /// agreeing to one is not agreeing to the other. Only takes effect while filling is on.
    fetch_artwork_online: bool,
    /// Whether the interface lists the keys for what is on screen.
    ///
    /// On by default: someone who has learnt the keys can turn them off, but nobody should have to
    /// guess them first.
    #[serde(default = "yes")]
    show_control_hints: bool,
    /// Whether to leave out the reports of what an action did.
    ///
    /// Off by default, so the interface says what it has done. Turning it on quietens the panes, at
    /// the cost of the notices that say when something failed: those share the same line.
    hide_status_messages: bool,
    /// What becomes of the playback daemon when the interface closes.
    #[serde(default)]
    daemon_on_close: DaemonOnClose,
    /// Whether to draw with the colours in `theme.toml` rather than the terminal's own palette.
    ///
    /// Off by default: a player should look like it belongs in the terminal it was opened in until
    /// asked to look like something else. See [`crate::theme`].
    custom_theme: bool,
}

/// Default for [`Config::show_control_hints`], which is on.
fn yes() -> bool {
    true
}

/// What happens to the playback daemon when the interface closes.
///
/// The daemon keeps playing without an interface, which is the point of it; the question is only
/// whether closing the interface should end that.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonOnClose {
    /// Always stop the daemon, so nothing is left playing.
    Stop,
    /// Stop it only if this interface was what started it, leaving a daemon that was already running
    /// to carry on. The default: closing the interface undoes what opening it did, and nothing more.
    #[default]
    StopIfWeStartedIt,
    /// Leave it running, playing on without an interface.
    Keep,
}

impl DaemonOnClose {
    /// Every choice, in the order the interface cycles through them.
    pub const ALL: [DaemonOnClose; 3] =
        [Self::StopIfWeStartedIt, Self::Stop, Self::Keep];

    /// A name for the interface.
    pub fn label(self) -> &'static str {
        match self {
            Self::Stop => "kill daemon",
            Self::StopIfWeStartedIt => "keep daemon if already running",
            Self::Keep => "keep daemon",
        }
    }

    /// What this choice means, spelled out.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Stop => "closing always stops playback",
            Self::StopIfWeStartedIt => "a daemon that was already running keeps playing",
            Self::Keep => "playback carries on without the interface",
        }
    }

    /// The next choice in the cycle.
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|choice| *choice == self).unwrap_or(0);

        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            master_volume: 0.5,
            default_folder: None,
            auto_fill_metadata: false,
            fetch_artwork_online: false,
            show_control_hints: true,
            hide_status_messages: false,
            daemon_on_close: DaemonOnClose::default(),
            custom_theme: false,
        }
    }
}

impl Config {
    /// Where the config file lives: `$XDG_CONFIG_HOME/ogma/config.toml` on Linux,
    /// `~/Library/Application Support/ogma/config.toml` on macOS, `%APPDATA%\ogma\config.toml` on
    /// Windows.
    pub fn config_path() -> Result<PathBuf, ConfigError> {
        let dir = dirs::config_dir().ok_or(ConfigError::NoConfigDirectory)?;

        Ok(dir.join(APP_DIR).join(FILE_NAME))
    }

    /// Load the config, falling back to the defaults if it cannot be read.
    ///
    /// A missing config file is written out with the default values, so the user has something to
    /// edit. A file that exists but is broken is left untouched: overwriting it would throw away
    /// settings the user meant to keep, so the defaults are used for this run instead.
    pub fn load() -> Self {
        match Self::try_load() {
            Ok(config) => config,
            Err(err) => {
                // The TUI has not started yet, so stderr is still the right place for this.
                eprintln!("ogma: {err}; using default config");
                Self::default()
            }
        }
    }

    /// [`Config::load`], but reporting why the config could not be read.
    pub fn try_load() -> Result<Self, ConfigError> {
        Self::try_load_from(&Self::config_path()?)
    }

    /// [`Config::try_load`] against a specific path, for callers that keep their config elsewhere.
    pub fn try_load_from(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let mut config: Config = toml::from_str(&text)?;

                // The file is hand-editable, so nothing in it can be trusted to be in range.
                config.master_volume = config.master_volume.clamp(0.0, 1.0);

                Ok(config)
            }
            // No config yet: write the defaults out so there is a file to edit, and use them.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let config = Self::default();
                config.save_to(path)?;

                Ok(config)
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Write the config to the standard location, creating the directory if needed.
    pub fn save(&self) -> Result<(), ConfigError> {
        self.save_to(&Self::config_path()?)
    }

    /// Write the config to `path`, creating parent directories if needed.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(path, toml::to_string_pretty(self)?)?;

        Ok(())
    }

    pub fn set_master_volume(&mut self, master_volume: f32) {
        self.master_volume = master_volume.clamp(0., 1.);
    }

    pub fn change_master_volume(&mut self, change: f32) {
        let volume_candidate = self.master_volume + change;
        self.master_volume = volume_candidate.clamp(0., 1.);
    }

    pub fn get_master_volume(&self) -> &f32 {
        &self.master_volume
    }

    /// The amplitude the master fader corresponds to, through the loudness curve.
    pub fn master_gain(&self) -> f32 {
        crate::volume::gain(self.master_volume)
    }

    /// The folder the library is built from, when one has been chosen.
    pub fn default_folder(&self) -> Option<&Path> {
        self.default_folder.as_ref().map(Path::new)
    }

    /// Whether missing metadata should be filled in from what is on disk.
    pub fn auto_fill_metadata(&self) -> bool {
        self.auto_fill_metadata
    }

    pub fn set_auto_fill_metadata(&mut self, enabled: bool) {
        self.auto_fill_metadata = enabled;
    }

    /// Turn the filling of missing metadata on if it is off, and off if it is on.
    pub fn toggle_auto_fill_metadata(&mut self) -> bool {
        self.auto_fill_metadata = !self.auto_fill_metadata;

        self.auto_fill_metadata
    }

    /// Whether filling in metadata may ask the internet.
    ///
    /// Meaningful only while [`Config::auto_fill_metadata`] is on.
    pub fn fetch_artwork_online(&self) -> bool {
        self.fetch_artwork_online
    }

    pub fn set_fetch_artwork_online(&mut self, enabled: bool) {
        self.fetch_artwork_online = enabled;
    }

    /// Turn the online lookup on if it is off, and off if it is on.
    pub fn toggle_fetch_artwork_online(&mut self) -> bool {
        self.fetch_artwork_online = !self.fetch_artwork_online;

        self.fetch_artwork_online
    }

    /// Whether the interface lists the keys for what is on screen.
    pub fn show_control_hints(&self) -> bool {
        self.show_control_hints
    }

    pub fn set_show_control_hints(&mut self, shown: bool) {
        self.show_control_hints = shown;
    }

    /// Show the key reminders if they are hidden, and hide them if they are shown.
    pub fn toggle_show_control_hints(&mut self) -> bool {
        self.show_control_hints = !self.show_control_hints;

        self.show_control_hints
    }

    /// Whether the reports of what an action did are left out.
    pub fn hide_status_messages(&self) -> bool {
        self.hide_status_messages
    }

    pub fn set_hide_status_messages(&mut self, hidden: bool) {
        self.hide_status_messages = hidden;
    }

    /// Hide the messages if they are shown, and show them if they are hidden.
    pub fn toggle_hide_status_messages(&mut self) -> bool {
        self.hide_status_messages = !self.hide_status_messages;

        self.hide_status_messages
    }

    /// What becomes of the playback daemon when the interface closes.
    pub fn daemon_on_close(&self) -> DaemonOnClose {
        self.daemon_on_close
    }

    pub fn set_daemon_on_close(&mut self, choice: DaemonOnClose) {
        self.daemon_on_close = choice;
    }

    /// Move to the next choice, returning it.
    pub fn cycle_daemon_on_close(&mut self) -> DaemonOnClose {
        self.daemon_on_close = self.daemon_on_close.next();

        self.daemon_on_close
    }

    /// Whether the colours come from `theme.toml` rather than the terminal's palette.
    pub fn custom_theme(&self) -> bool {
        self.custom_theme
    }

    pub fn set_custom_theme(&mut self, custom: bool) {
        self.custom_theme = custom;
    }

    /// Switch between the terminal's palette and `theme.toml`.
    pub fn toggle_custom_theme(&mut self) -> bool {
        self.custom_theme = !self.custom_theme;

        self.custom_theme
    }

    /// Forget the chosen folder.
    pub fn clear_default_folder(&mut self) {
        self.default_folder = None;
    }

    pub fn set_default_folder(&mut self, path: &Path) -> Result<(), ConfigError> {
        if !path.is_dir() {
            return Err(ConfigError::NotDirectory)
        }

        let path_string = match path.to_str() {
            Some(str) => {
                str.to_string()
            }
            None => {
                return Err(ConfigError::FailedToConvertPathToString);
            }
        };

        self.default_folder = Some(path_string);

        Ok(())
    }
}
