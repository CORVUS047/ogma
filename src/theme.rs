//! Colours for the interface, optionally taken from a `theme.toml` beside the config.
//!
//! Without a theme the player leans on the terminal's own palette: named ANSI colours and the
//! terminal's dim rendering, so it fits whatever scheme the user already has. With
//! [`Config::custom_theme`](crate::config::Config::custom_theme) on, the colours come from
//! `theme.toml` instead, where they can be named (`cyan`), a palette index (`104`), or a hex triplet
//! (`#7aa2f7`).
//!
//! A colour of `reset` means "whatever the terminal uses", and for the muted and border colours it
//! also brings the terminal's dim rendering with it — which is what the default look is built from.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

/// File the colours are read from, inside the config directory.
const FILE_NAME: &str = "theme.toml";

/// Why a theme could not be read or written.
#[derive(Debug)]
pub enum ThemeError {
    NoConfigDirectory,
    Io(std::io::Error),
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
    /// A colour in the file is not one this build understands.
    Color { field: &'static str, value: String },
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeError::NoConfigDirectory => {
                write!(f, "no config directory available on this platform")
            }
            ThemeError::Io(err) => write!(f, "{err}"),
            ThemeError::Parse(err) => write!(f, "invalid theme file: {err}"),
            ThemeError::Serialize(err) => write!(f, "cannot write theme: {err}"),
            ThemeError::Color { field, value } => {
                write!(f, "{field}: {value:?} is not a colour")
            }
        }
    }
}

impl std::error::Error for ThemeError {}

impl From<std::io::Error> for ThemeError {
    fn from(err: std::io::Error) -> Self {
        ThemeError::Io(err)
    }
}

impl From<toml::de::Error> for ThemeError {
    fn from(err: toml::de::Error) -> Self {
        ThemeError::Parse(err)
    }
}

impl From<toml::ser::Error> for ThemeError {
    fn from(err: toml::ser::Error) -> Self {
        ThemeError::Serialize(err)
    }
}

/// The colours the interface draws with.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Emphasis: progress and volume bars, paths, the artist line.
    pub accent: Color,
    /// Borders of panes without the focus.
    pub border: Color,
    /// Borders of the pane with the focus.
    pub border_focused: Color,
    /// Pane titles.
    pub title: Color,
    /// Ordinary text.
    pub text: Color,
    /// Secondary text: hints, details, anything deliberately quiet.
    pub muted: Color,
    /// The selected row's text.
    pub highlight: Color,
    /// Behind the selected row.
    pub highlight_background: Color,
    /// The now-playing marker and title.
    pub playing: Color,
    /// A message about something that worked.
    pub success: Color,
    /// A message about something that did not.
    pub error: Color,
}

impl Default for Theme {
    /// The terminal's own palette: ANSI colour names, and `reset` wherever the terminal should decide.
    fn default() -> Self {
        Theme {
            accent: Color::Cyan,
            border: Color::Reset,
            border_focused: Color::Cyan,
            title: Color::Reset,
            text: Color::Reset,
            muted: Color::Reset,
            highlight: Color::Reset,
            highlight_background: Color::Reset,
            playing: Color::Cyan,
            success: Color::Green,
            error: Color::Red,
        }
    }
}

impl Theme {
    /// The theme to draw with: the terminal's palette unless `custom` asks for `theme.toml`.
    ///
    /// A missing theme file is written out with the defaults, so there is something to edit. A file
    /// that exists but is broken is left alone and reported, since overwriting it would throw away
    /// whatever the user was in the middle of writing.
    pub fn load(custom: bool) -> Self {
        if !custom {
            return Self::default();
        }

        match Self::try_load() {
            Ok(theme) => theme,
            Err(err) => {
                // The interface has not started yet, so stderr is still the right place.
                eprintln!("ogma: {err}; using the default theme");
                Self::default()
            }
        }
    }

    /// [`Theme::load`], reporting why the theme could not be read.
    pub fn try_load() -> Result<Self, ThemeError> {
        Self::try_load_from(&Self::path()?)
    }

    /// Read a theme from `path`, writing the defaults there if nothing is there yet.
    pub fn try_load_from(path: &Path) -> Result<Self, ThemeError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str::<OnDisk>(&text)?.into_theme(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let theme = Self::default();
                theme.save_to(path)?;

                Ok(theme)
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Where the theme file lives: beside the config, as `theme.toml`.
    pub fn path() -> Result<PathBuf, ThemeError> {
        let config = crate::config::Config::config_path().map_err(|_| ThemeError::NoConfigDirectory)?;
        let parent = config.parent().ok_or(ThemeError::NoConfigDirectory)?;

        Ok(parent.join(FILE_NAME))
    }

    /// Write the theme to the standard location.
    pub fn save(&self) -> Result<PathBuf, ThemeError> {
        let path = Self::path()?;
        self.save_to(&path)?;

        Ok(path)
    }

    /// Write the theme to `path`, creating parent directories as needed.
    pub fn save_to(&self, path: &Path) -> Result<(), ThemeError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(path, toml::to_string_pretty(&OnDisk::from(self))?)?;

        Ok(())
    }

    // ---------------------------------------------------------------------------------- styles

    /// A pane border, brighter when the pane has the focus.
    pub fn border(&self, focused: bool) -> Style {
        if focused {
            colour(self.border_focused)
        } else {
            colour(self.border)
        }
    }

    /// A pane title.
    pub fn title(&self) -> Style {
        Style::new().fg(self.title).add_modifier(Modifier::BOLD)
    }

    /// Ordinary text.
    pub fn text(&self) -> Style {
        Style::new().fg(self.text)
    }

    /// Secondary text.
    pub fn muted(&self) -> Style {
        colour(self.muted)
    }

    /// Emphasis.
    pub fn accent(&self) -> Style {
        Style::new().fg(self.accent)
    }

    /// The selected row.
    pub fn highlight(&self) -> Style {
        let style = Style::new().fg(self.highlight).add_modifier(Modifier::BOLD);

        if self.highlight_background == Color::Reset {
            style
        } else {
            style.bg(self.highlight_background)
        }
    }

    /// The selected row in a pane that does not have the focus.
    pub fn highlight_unfocused(&self) -> Style {
        Style::new().fg(self.highlight).add_modifier(Modifier::DIM)
    }

    /// The now-playing marker.
    pub fn playing(&self) -> Style {
        Style::new().fg(self.playing)
    }

    /// A message about something that worked.
    pub fn success(&self) -> Style {
        Style::new().fg(self.success)
    }

    /// A message about something that did not.
    pub fn error(&self) -> Style {
        Style::new().fg(self.error)
    }
}

/// A colour, or the terminal's dim rendering where the colour is `reset`.
///
/// Without this, everything meant to recede would be drawn in the same colour as everything else on
/// a terminal-palette theme.
fn colour(color: Color) -> Style {
    let style = Style::new().fg(color);

    if color == Color::Reset {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}

/// The shape a theme takes on disk: colours as strings, which is how anyone would write them.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct OnDisk {
    accent: String,
    border: String,
    border_focused: String,
    title: String,
    text: String,
    muted: String,
    highlight: String,
    highlight_background: String,
    playing: String,
    success: String,
    error: String,
}

impl Default for OnDisk {
    fn default() -> Self {
        Self::from(&Theme::default())
    }
}

impl From<&Theme> for OnDisk {
    fn from(theme: &Theme) -> Self {
        OnDisk {
            accent: name_of(theme.accent),
            border: name_of(theme.border),
            border_focused: name_of(theme.border_focused),
            title: name_of(theme.title),
            text: name_of(theme.text),
            muted: name_of(theme.muted),
            highlight: name_of(theme.highlight),
            highlight_background: name_of(theme.highlight_background),
            playing: name_of(theme.playing),
            success: name_of(theme.success),
            error: name_of(theme.error),
        }
    }
}

impl OnDisk {
    fn into_theme(self) -> Result<Theme, ThemeError> {
        Ok(Theme {
            accent: parse("accent", &self.accent)?,
            border: parse("border", &self.border)?,
            border_focused: parse("border_focused", &self.border_focused)?,
            title: parse("title", &self.title)?,
            text: parse("text", &self.text)?,
            muted: parse("muted", &self.muted)?,
            highlight: parse("highlight", &self.highlight)?,
            highlight_background: parse("highlight_background", &self.highlight_background)?,
            playing: parse("playing", &self.playing)?,
            success: parse("success", &self.success)?,
            error: parse("error", &self.error)?,
        })
    }
}

/// Read a colour written as a name, a palette index, or a hex triplet.
fn parse(field: &'static str, value: &str) -> Result<Color, ThemeError> {
    Color::from_str(value).map_err(|_| ThemeError::Color { field, value: value.to_string() })
}

/// Write a colour the way it would be typed.
fn name_of(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(index) => index.to_string(),
        // `Display` capitalises the names; lowercase is how anyone would type them, and is what
        // `from_str` reads back.
        other => other.to_string().to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_can_be_written_three_ways() {
        assert_eq!(parse("x", "cyan").expect("a colour"), Color::Cyan);
        assert_eq!(parse("x", "light-blue").expect("a colour"), Color::LightBlue);
        assert_eq!(parse("x", "104").expect("a colour"), Color::Indexed(104));
        assert_eq!(parse("x", "#7aa2f7").expect("a colour"), Color::Rgb(0x7a, 0xa2, 0xf7));
        assert_eq!(parse("x", "reset").expect("a colour"), Color::Reset);

        let err = parse("accent", "banana").expect_err("not a colour");
        assert!(err.to_string().contains("accent"), "{err}");
        assert!(err.to_string().contains("banana"));
    }

    #[test]
    fn every_colour_survives_being_written_and_read() {
        for colour in [
            Color::Cyan,
            Color::Reset,
            Color::LightMagenta,
            Color::Indexed(200),
            Color::Rgb(1, 2, 3),
        ] {
            let written = name_of(colour);

            assert_eq!(parse("x", &written).expect("a colour"), colour, "via {written:?}");
        }
    }

    #[test]
    fn the_terminals_own_colours_come_with_its_dim_rendering() {
        let theme = Theme::default();

        // Muted text on a terminal-palette theme has no colour of its own, so it has to recede some
        // other way.
        assert!(theme.muted().add_modifier.contains(Modifier::DIM));
        assert!(theme.border(false).add_modifier.contains(Modifier::DIM));

        // A theme that names a colour uses it as it is.
        let custom = Theme { muted: Color::Indexed(8), ..Theme::default() };
        assert!(!custom.muted().add_modifier.contains(Modifier::DIM));
        assert_eq!(custom.muted().fg, Some(Color::Indexed(8)));
    }

    #[test]
    fn a_background_of_reset_leaves_the_background_alone() {
        assert_eq!(Theme::default().highlight().bg, None);

        let custom = Theme { highlight_background: Color::Blue, ..Theme::default() };
        assert_eq!(custom.highlight().bg, Some(Color::Blue));
    }
}
