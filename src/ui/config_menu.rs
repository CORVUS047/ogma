//! The config screen: edit the settings in place and write them back to disk.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::Modifier;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Padding};

use crate::config::Config;
use crate::theme::Theme;
use crate::volume;

/// How much one key press moves the volume.
const VOLUME_STEP: f32 = 0.05;

/// Cells in the volume bar.
const BAR_CELLS: usize = 20;

/// How many results one key press adds to or takes off a search.
const RESULTS_STEP: i32 = 5;

/// The settings the screen can edit, in the order they are listed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Field {
    MasterVolume,
    DefaultFolder,
    AutoFillMetadata,
    FetchArtworkOnline,
    ShowControlHints,
    HideStatusMessages,
    DaemonOnClose,
    DownloadFolder,
    DownloadFormat,
    SearchResults,
    CustomTheme,
}

impl Field {
    const ALL: [Field; 11] = [
        Self::MasterVolume,
        Self::DefaultFolder,
        Self::AutoFillMetadata,
        Self::FetchArtworkOnline,
        Self::ShowControlHints,
        Self::HideStatusMessages,
        Self::DaemonOnClose,
        Self::DownloadFolder,
        Self::DownloadFormat,
        Self::SearchResults,
        Self::CustomTheme,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::MasterVolume => "Master volume",
            Self::DefaultFolder => "Default folder",
            Self::AutoFillMetadata => "Fill metadata",
            Self::FetchArtworkOnline => "Look online",
            Self::ShowControlHints => "Control hints",
            Self::HideStatusMessages => "Hide messages",
            Self::DaemonOnClose => "On close",
            Self::DownloadFolder => "Downloads",
            Self::DownloadFormat => "Download as",
            Self::SearchResults => "Search results",
            Self::CustomTheme => "Theme",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::MasterVolume => "left/right adjust · 0-9 set level",
            Self::DefaultFolder => "enter edit path · d clear",
            Self::AutoFillMetadata => "enter toggle · writes missing artwork into files",
            Self::FetchArtworkOnline => "enter toggle · sends artist and album to services",
            Self::ShowControlHints => "enter toggle · lists the keys on each screen",
            Self::HideStatusMessages => "enter toggle · silences what an action reports",
            Self::DaemonOnClose => "enter cycles · what happens to playback when closed",
            Self::DownloadFolder => "enter edit path · d clear · where YouTube downloads land",
            Self::DownloadFormat => "enter cycles · what a downloaded track is kept as",
            Self::SearchResults => "left/right adjust · how many hits a search asks for",
            Self::CustomTheme => "enter toggle · colours from theme.toml, restart to apply",
        }
    }
}

/// Whether the screen is being navigated or a field is being typed into.
#[derive(Debug)]
enum Mode {
    Browsing,
    /// Typing a folder path into one of the path fields. Holds the text so far, so Esc can abandon
    /// it, and which field it belongs to, since two of them are paths.
    EditingPath { field: Field, buffer: String },
}

/// What the screen asks the application to do next.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConfigMenuOutcome {
    /// Leave the config screen.
    Close,
}

/// A short message about the last action, and whether it went wrong.
#[derive(Debug)]
struct Status {
    text: String,
    is_error: bool,
}

/// The config screen and its editing state. The config itself lives in the application, and is
/// passed in, so edits are visible everywhere immediately.
#[derive(Debug)]
pub struct ConfigMenu {
    state: ListState,
    mode: Mode,
    status: Option<Status>,
    /// Whether there are edits that have not reached the disk yet.
    unsaved: bool,
    /// Whether to report what an action did.
    messages: bool,
    /// Colours to draw with.
    theme: Theme,
}

impl Default for ConfigMenu {
    fn default() -> Self {
        ConfigMenu {
            state: ListState::default().with_selected(Some(0)),
            mode: Mode::Browsing,
            status: None,
            unsaved: false,
            messages: true,
            theme: Theme::default(),
        }
    }
}

impl ConfigMenu {
    pub fn new() -> Self {
        Self::default()
    }

    /// Draw with `theme` rather than whatever was set before.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Show or hide the reports of what an action did.
    ///
    /// The unsaved marker in the title is state rather than a report, and stays either way — so a
    /// save is still visibly a save.
    pub fn set_messages(&mut self, messages: bool) {
        self.messages = messages;
    }

    /// Whether edits are pending a write to disk.
    pub fn unsaved(&self) -> bool {
        self.unsaved
    }

    fn selected(&self) -> Field {
        let index = self.state.selected().unwrap_or(0);

        Field::ALL[index.min(Field::ALL.len() - 1)]
    }

    fn ok(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), is_error: false });
    }

    fn failed(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), is_error: true });
    }

    /// Handle a key press against `config`, returning what the application should do next.
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        config: &mut Config,
    ) -> Option<ConfigMenuOutcome> {
        if key.kind != KeyEventKind::Press {
            return None;
        }

        match &mut self.mode {
            Mode::Browsing => self.handle_browsing_key(key, config),
            // Borrow the buffer for the duration of the edit, so typing does not clone it per key.
            Mode::EditingPath { .. } => {
                self.handle_editing_key(key, config);
                None
            }
        }
    }

    fn handle_browsing_key(
        &mut self,
        key: KeyEvent,
        config: &mut Config,
    ) -> Option<ConfigMenuOutcome> {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous(),

            // Adjusting a value. The volume and the result count are what respond to these.
            KeyCode::Left | KeyCode::Char('h') if self.selected() == Field::SearchResults => {
                self.nudge_results(config, -RESULTS_STEP)
            }
            KeyCode::Right | KeyCode::Char('l') if self.selected() == Field::SearchResults => {
                self.nudge_results(config, RESULTS_STEP)
            }
            KeyCode::Left | KeyCode::Char('h') => self.nudge_volume(config, -VOLUME_STEP),
            KeyCode::Right | KeyCode::Char('l') => self.nudge_volume(config, VOLUME_STEP),

            // 0 through 9 set the volume directly, 0 being muted and 9 full.
            KeyCode::Char(digit @ '0'..='9') if self.selected() == Field::MasterVolume => {
                let level = f32::from(digit as u8 - b'0') / 9.0;
                config.set_master_volume(level);
                self.unsaved = true;
                self.report_volume(config);
            }

            KeyCode::Enter | KeyCode::Char(' ') if self.selected() == Field::DaemonOnClose => {
                let choice = config.cycle_daemon_on_close();
                self.unsaved = true;

                self.ok(choice.describe());
            }

            KeyCode::Enter | KeyCode::Char(' ')
                if self.selected() == Field::HideStatusMessages =>
            {
                let hidden = config.toggle_hide_status_messages();
                self.unsaved = true;

                // Reported before the setting takes hold, so turning it on still says so once.
                self.ok(if hidden { "Messages hidden" } else { "Messages shown" });
            }

            KeyCode::Enter | KeyCode::Char(' ') if self.selected() == Field::CustomTheme => {
                let custom = config.toggle_custom_theme();
                self.unsaved = true;

                // The theme file is written now if it does not exist, so there is something to edit
                // straight away rather than only after the next start.
                if custom {
                    match crate::theme::Theme::try_load() {
                        Ok(_) => match crate::theme::Theme::path() {
                            Ok(path) => self.ok(format!("Theme from {}", path.display())),
                            Err(_) => self.ok("Custom theme on"),
                        },
                        Err(err) => self.failed(format!("{err}")),
                    }
                } else {
                    self.ok("Terminal colours");
                }
            }

            KeyCode::Enter | KeyCode::Char(' ')
                if self.selected() == Field::ShowControlHints =>
            {
                let shown = config.toggle_show_control_hints();
                self.unsaved = true;

                self.ok(if shown {
                    "Control hints shown"
                } else {
                    "Control hints hidden"
                });
            }

            KeyCode::Enter | KeyCode::Char(' ')
                if self.selected() == Field::FetchArtworkOnline =>
            {
                let enabled = config.toggle_fetch_artwork_online();
                self.unsaved = true;

                self.ok(if enabled {
                    "Online lookup on: MusicBrainz, Cover Art Archive, iTunes"
                } else {
                    "Online lookup off: disk only"
                });
            }

            KeyCode::Enter | KeyCode::Char(' ')
                if self.selected() == Field::AutoFillMetadata =>
            {
                let enabled = config.toggle_auto_fill_metadata();
                self.unsaved = true;

                self.ok(if enabled {
                    "Filling metadata on: artwork written into files"
                } else {
                    "Filling metadata off"
                });
            }

            KeyCode::Enter if self.selected() == Field::DefaultFolder => {
                let buffer = config
                    .default_folder()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default();

                self.mode = Mode::EditingPath { field: Field::DefaultFolder, buffer };
                self.status = None;
            }

            KeyCode::Enter if self.selected() == Field::DownloadFolder => {
                let buffer = config
                    .download_folder()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default();

                self.mode = Mode::EditingPath { field: Field::DownloadFolder, buffer };
                self.status = None;
            }

            KeyCode::Char('d') | KeyCode::Delete if self.selected() == Field::DownloadFolder => {
                config.clear_download_folder();
                self.unsaved = true;
                self.ok("Downloads go to the default folder");
            }

            KeyCode::Enter | KeyCode::Char(' ') if self.selected() == Field::DownloadFormat => {
                let format = config.cycle_download_format();
                self.unsaved = true;

                self.ok(format.describe());
            }

            KeyCode::Char('d') | KeyCode::Delete if self.selected() == Field::DefaultFolder => {
                config.clear_default_folder();
                self.unsaved = true;
                self.ok("Default folder cleared");
            }

            KeyCode::Char('s') => self.save(config),

            // Leaving writes the edits out, so a config screen cannot silently lose them.
            KeyCode::Esc | KeyCode::Char('q') => {
                if self.unsaved {
                    self.save(config);
                }

                return Some(ConfigMenuOutcome::Close);
            }

            _ => {}
        }

        None
    }

    fn handle_editing_key(&mut self, key: KeyEvent, config: &mut Config) {
        let Mode::EditingPath { field, buffer } = &mut self.mode else {
            return;
        };

        let field = *field;

        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => buffer.push(c),
            KeyCode::Backspace => {
                buffer.pop();
            }
            // Ctrl-U clears the line, as it does in a shell.
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => buffer.clear(),
            KeyCode::Esc => {
                self.mode = Mode::Browsing;
                self.ok("Edit cancelled");
            }
            KeyCode::Enter => {
                let entered = buffer.trim().to_string();
                self.mode = Mode::Browsing;

                if entered.is_empty() {
                    match field {
                        Field::DownloadFolder => {
                            config.clear_download_folder();
                            self.ok("Downloads go to the default folder");
                        }
                        _ => {
                            config.clear_default_folder();
                            self.ok("Default folder cleared");
                        }
                    }

                    self.unsaved = true;
                    return;
                }

                let path = expand_home(&entered);

                // The download folder is made when a download starts, so it need not exist yet;
                // a library folder that is not there has nothing to scan.
                let result = match field {
                    Field::DownloadFolder => config.set_download_folder(&path),
                    _ => config.set_default_folder(&path),
                };

                match result {
                    Ok(()) => {
                        self.unsaved = true;

                        match field {
                            Field::DownloadFolder => {
                                self.ok(format!("Downloads go to {}", path.display()))
                            }
                            _ => self.ok(format!("Default folder set to {}", path.display())),
                        }
                    }
                    Err(err) => self.failed(format!("{}: {err}", path.display())),
                }
            }
            _ => {}
        }
    }

    fn select_next(&mut self) {
        match self.state.selected() {
            Some(index) if index + 1 >= Field::ALL.len() => self.state.select_first(),
            _ => self.state.select_next(),
        }
    }

    fn select_previous(&mut self) {
        match self.state.selected() {
            Some(0) | None => self.state.select(Some(Field::ALL.len() - 1)),
            _ => self.state.select_previous(),
        }
    }

    fn nudge_volume(&mut self, config: &mut Config, change: f32) {
        if self.selected() != Field::MasterVolume {
            return;
        }

        config.change_master_volume(change);
        self.unsaved = true;
        self.report_volume(config);
    }

    /// Ask for more or fewer search results.
    fn nudge_results(&mut self, config: &mut Config, change: i32) {
        let wanted = config.search_results() as i32 + change;

        config.set_search_results(wanted.max(1) as usize);
        self.unsaved = true;

        let count = config.search_results();
        self.ok(format!("Searches ask for {count} results"));
    }

    /// Say where the fader now sits, in both the fader's terms and the ear's.
    fn report_volume(&mut self, config: &Config) {
        let level = *config.get_master_volume();

        self.ok(format!(
            "Master volume {}% ({})",
            volume::percent(level),
            volume::display_decibels(level)
        ));
    }

    fn save(&mut self, config: &Config) {
        match config.save() {
            Ok(()) => {
                self.unsaved = false;

                match Config::config_path() {
                    Ok(path) => self.ok(format!("Saved to {}", path.display())),
                    Err(_) => self.ok("Saved"),
                }
            }
            Err(err) => self.failed(format!("Could not save: {err}")),
        }
    }

    /// Draw the screen centered in `area`.
    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect, config: &Config) {
        // The fields, the hint and the status line, with the spacing, padding and borders around
        // them.
        let height = Field::ALL.len() as u16 + 8;
        let menu_area = area.centered(Constraint::Length(62), Constraint::Length(height));

        let title = if self.unsaved { " Config · unsaved " } else { " Config " };

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(self.theme.border(true))
            .title(Line::from(title).centered().style(self.theme.title()))
            .title_bottom(
                Line::from(" up/down move · s save · esc back ")
                    .centered()
                    .style(self.theme.muted()),
            )
            .padding(Padding::uniform(1));

        let inner = block.inner(menu_area);
        frame.render_widget(block, menu_area);

        // Fields at the top, then the per-field hint, then the result of the last action.
        let [fields_area, hint_area, status_area] = Layout::vertical([
            Constraint::Length(Field::ALL.len() as u16),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .spacing(1)
        .areas(inner);

        let items: Vec<ListItem<'_>> = Field::ALL
            .iter()
            .map(|field| ListItem::new(self.field_line(*field, config)))
            .collect();

        let list = List::new(items).highlight_symbol("▶ ").highlight_style(self.theme.highlight());

        frame.render_stateful_widget(list, fields_area, &mut self.state);

        let hint = match &self.mode {
            Mode::Browsing => self.selected().hint(),
            Mode::EditingPath { .. } => "enter apply · esc cancel · ctrl-u clear",
        };
        frame.render_widget(Line::from(hint).style(self.theme.muted()).centered(), hint_area);

        if let Some(status) = self.status.as_ref().filter(|_| self.messages) {
            let line = if status.is_error {
                Line::from(status.text.as_str()).style(self.theme.error())
            } else {
                Line::from(status.text.as_str()).style(self.theme.success())
            };

            frame.render_widget(line.centered(), status_area);
        }
    }

    /// One row: the label, then the value or the text being typed into it.
    fn field_line(&self, field: Field, config: &Config) -> Line<'static> {
        let label = format!("{:<15}", field.label());

        let value: Vec<Span<'static>> = match field {
            Field::MasterVolume => {
                let level = *config.get_master_volume();

                // Both readings: the percentage is the fader, the decibels are what it does.
                vec![
                    Span::from(volume_bar(level)).style(self.theme.accent()),
                    Span::from(format!(" {:>3}%", volume::percent(level))).style(self.theme.text()),
                    Span::from(format!("  {:>9}", volume::display_decibels(level)))
                        .style(self.theme.muted()),
                ]
            }
            Field::AutoFillMetadata => {
                if config.auto_fill_metadata() {
                    vec![
                        Span::from("on").style(self.theme.success()),
                        Span::from("   missing artwork").style(self.theme.muted()),
                    ]
                } else {
                    vec![Span::from("off").style(self.theme.muted())]
                }
            }
            Field::FetchArtworkOnline => {
                // Only meaningful while filling is on, and the row says so rather than lying about
                // what it will do.
                if !config.auto_fill_metadata() {
                    let state = if config.fetch_artwork_online() { "on" } else { "off" };

                    vec![
                        Span::from(state).style(self.theme.muted()),
                        Span::from("   needs Fill metadata").style(self.theme.muted()),
                    ]
                } else if config.fetch_artwork_online() {
                    vec![
                        Span::from("on").style(self.theme.success()),
                        Span::from("   MusicBrainz · iTunes").style(self.theme.muted()),
                    ]
                } else {
                    vec![
                        Span::from("off").style(self.theme.muted()),
                        Span::from("   disk only").style(self.theme.muted()),
                    ]
                }
            }
            Field::ShowControlHints => {
                if config.show_control_hints() {
                    vec![Span::from("shown").style(self.theme.success())]
                } else {
                    vec![Span::from("hidden").style(self.theme.muted())]
                }
            }
            Field::HideStatusMessages => {
                if config.hide_status_messages() {
                    vec![Span::from("on").style(self.theme.success())]
                } else {
                    vec![Span::from("off").style(self.theme.muted())]
                }
            }
            Field::DaemonOnClose => {
                let choice = config.daemon_on_close();
                let style = if choice == crate::config::DaemonOnClose::Keep {
                    self.theme.success()
                } else {
                    self.theme.text()
                };

                vec![Span::from(choice.label()).style(style)]
            }
            Field::CustomTheme => {
                if config.custom_theme() {
                    vec![Span::from("Custom").style(self.theme.success())]
                } else {
                    vec![Span::from("Terminal").style(self.theme.muted())]
                }
            }
            Field::DefaultFolder => match &self.mode {
                // While editing, the row becomes the input, with a block for the cursor.
                Mode::EditingPath { field: Field::DefaultFolder, buffer } => {
                    typing_spans(buffer, &self.theme)
                }
                _ => match config.default_folder() {
                    Some(path) => {
                        vec![Span::from(path.display().to_string()).style(self.theme.text())]
                    }
                    None => vec![Span::from("not set").style(self.theme.muted())],
                },
            },
            Field::DownloadFolder => match &self.mode {
                Mode::EditingPath { field: Field::DownloadFolder, buffer } => {
                    typing_spans(buffer, &self.theme)
                }
                _ => match config.download_folder() {
                    Some(path) => {
                        vec![Span::from(path.display().to_string()).style(self.theme.text())]
                    }
                    // Where they go when nothing is set depends on the platform, so the row says
                    // what will happen rather than a path that may not be the one used.
                    None => vec![
                        Span::from("default").style(self.theme.muted()),
                        Span::from("   beside the library, else the music folder")
                            .style(self.theme.muted()),
                    ],
                },
            },
            Field::DownloadFormat => {
                let format = config.download_format();

                vec![
                    Span::from(format.label()).style(self.theme.text()),
                    Span::from(format!("   {}", format.describe())).style(self.theme.muted()),
                ]
            }
            Field::SearchResults => {
                vec![Span::from(config.search_results().to_string()).style(self.theme.text())]
            }
        };

        let mut spans = vec![Span::from(label).style(self.theme.text())];
        spans.extend(value);

        Line::from(spans)
    }
}

/// A field being typed into: what is there so far, and a block for the cursor.
fn typing_spans(buffer: &str, theme: &Theme) -> Vec<Span<'static>> {
    vec![
        Span::from(buffer.to_string()).style(theme.text().add_modifier(Modifier::UNDERLINED)),
        Span::from("█").style(theme.accent()),
    ]
}

/// A bar of filled and empty cells for a 0.0 to 1.0 level.
fn volume_bar(level: f32) -> String {
    let filled = (level.clamp(0.0, 1.0) * BAR_CELLS as f32).round() as usize;

    "█".repeat(filled) + &"░".repeat(BAR_CELLS - filled)
}

/// Expand a leading `~` to the user's home directory, since paths are typed by hand here.
fn expand_home(input: &str) -> std::path::PathBuf {
    let Some(rest) = input.strip_prefix('~') else {
        return std::path::PathBuf::from(input);
    };

    match dirs::home_dir() {
        Some(home) => home.join(rest.trim_start_matches('/')),
        None => std::path::PathBuf::from(input),
    }
}
