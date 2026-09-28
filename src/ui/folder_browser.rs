//! The folder browser: walk the filesystem and choose the folder the library is built from.

use std::fs;
use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Padding};

use crate::config::Config;
use crate::meta;
use crate::theme::Theme;

/// The entry that walks up out of the current folder.
const PARENT: &str = "..";

/// What the browser asks the application to do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderBrowserOutcome {
    /// The folder was chosen and already written to the config.
    Chosen(PathBuf),
    /// The user backed out without choosing.
    Cancel,
}

/// A short message about the last action, and whether it went wrong.
#[derive(Debug)]
struct Status {
    text: String,
    is_error: bool,
}

/// A directory listing, and where the user is in it.
#[derive(Debug)]
pub struct FolderBrowser {
    /// The folder being listed.
    cwd: PathBuf,
    /// Subfolders of `cwd`, with the parent entry first where there is one.
    entries: Vec<PathBuf>,
    state: ListState,
    /// How many audio files sit directly in `cwd`, as a hint that this is the right folder.
    audio_here: usize,
    show_hidden: bool,
    status: Option<Status>,
    /// Folders matching this are the only ones listed, when there is one.
    filter: Option<String>,
    /// Indices into `entries` that the filter admits.
    visible: Vec<usize>,
    /// Whether keys are going into the search.
    searching: bool,
    /// Whether the keys are listed along the bottom.
    hints: bool,
    /// Whether to report what an action did.
    messages: bool,
    /// Colours to draw with.
    theme: Theme,
}

impl Default for FolderBrowser {
    fn default() -> Self {
        Self::new()
    }
}

impl FolderBrowser {
    /// Open the browser in the current working directory, falling back to the home directory and
    /// then the root if it cannot be read.
    pub fn new() -> Self {
        let start = std::env::current_dir()
            .or_else(|_| dirs::home_dir().ok_or(()).map_err(|_| std::io::Error::other("no home")))
            .unwrap_or_else(|_| PathBuf::from("/"));

        Self::at(&start)
    }

    /// Open the browser at `path`.
    pub fn at(path: &Path) -> Self {
        let mut browser = FolderBrowser {
            cwd: PathBuf::new(),
            entries: Vec::new(),
            state: ListState::default(),
            audio_here: 0,
            show_hidden: false,
            status: None,
            filter: None,
            visible: Vec::new(),
            searching: false,
            hints: true,
            messages: true,
            theme: Theme::default(),
        };

        browser.open(path);
        browser
    }

    /// Draw with `theme` rather than whatever was set before.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Show or hide the key reminders.
    pub fn set_hints(&mut self, hints: bool) {
        self.hints = hints;
    }

    /// Show or hide the reports of what an action did. The summary of what the folder holds is not a
    /// report, and stays either way.
    pub fn set_messages(&mut self, messages: bool) {
        self.messages = messages;
    }

    /// The folder currently being listed.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The highlighted entry, if the listing is not empty.
    fn highlighted(&self) -> Option<&Path> {
        let index = *self.visible.get(self.state.selected()?)?;

        self.entries.get(index).map(PathBuf::as_path)
    }

    /// What the search is narrowing the listing to, if anything.
    pub fn filter(&self) -> Option<&str> {
        self.filter.as_deref()
    }

    /// Work out which folders the filter admits, keeping the way out reachable and the highlight
    /// inside what is left.
    fn apply_filter(&mut self) {
        self.visible = match &self.filter {
            None => (0..self.entries.len()).collect(),
            Some(query) => {
                let needle = query.to_lowercase();
                let parent = self.cwd.parent();

                (0..self.entries.len())
                    .filter(|index| {
                        let path = &self.entries[*index];

                        Some(path.as_path()) == parent
                            || path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .is_some_and(|name| name.to_lowercase().contains(&needle))
                    })
                    .collect()
            }
        };

        self.state.select((!self.visible.is_empty()).then(|| {
            self.state.selected().unwrap_or(0).min(self.visible.len() - 1)
        }));
    }

    /// Put the whole listing back.
    fn clear_filter(&mut self) {
        self.filter = None;
        self.searching = false;
        self.apply_filter();
    }

    /// Keys while typing a search.
    fn handle_search_key(&mut self, key: KeyEvent) {
        let Some(query) = &mut self.filter else {
            self.searching = false;
            return;
        };

        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => query.clear(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => query.push(c),
            KeyCode::Backspace => {
                query.pop();
            }
            KeyCode::Enter | KeyCode::Down | KeyCode::Up => {
                self.searching = false;

                if self.filter.as_ref().is_some_and(|query| query.is_empty()) {
                    self.filter = None;
                }

                self.apply_filter();
                return;
            }
            KeyCode::Esc => {
                self.clear_filter();
                return;
            }
            _ => return,
        }

        self.apply_filter();
    }

    /// List `path`, leaving the previous listing in place if it cannot be read.
    fn open(&mut self, path: &Path) {
        // An absolute path keeps the title honest and makes the chosen folder portable.
        let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        let read = match fs::read_dir(&target) {
            Ok(read) => read,
            Err(err) => {
                self.failed(format!("{}: {err}", target.display()));
                return;
            }
        };

        let mut folders = Vec::new();
        let mut audio_here = 0;

        for entry in read.flatten() {
            let path = entry.path();

            let is_hidden = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'));

            if is_hidden && !self.show_hidden {
                continue;
            }

            // `is_dir` follows symlinks, so a link to a folder is browsable like the folder itself.
            if path.is_dir() {
                folders.push(path);
            } else if meta::has_audio_extension(&path) {
                audio_here += 1;
            }
        }

        folders.sort_by_key(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_lowercase)
                .unwrap_or_default()
        });

        let mut entries = Vec::with_capacity(folders.len() + 1);
        if let Some(parent) = target.parent() {
            entries.push(parent.to_path_buf());
        }
        entries.extend(folders);

        self.cwd = target;
        self.entries = entries;
        self.audio_here = audio_here;
        self.status = None;
        // A new folder is a fresh listing; a search carried into it would hide most of it.
        self.filter = None;
        self.searching = false;
        self.state.select(Some(0));
        self.apply_filter();
    }

    fn failed(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), is_error: true });
    }

    /// Handle a key press, returning what the application should do next.
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        config: &mut Config,
    ) -> Option<FolderBrowserOutcome> {
        if key.kind != KeyEventKind::Press {
            return None;
        }

        if self.searching {
            self.handle_search_key(key);
            return None;
        }

        match key.code {
            // Narrow the listing to folders matching what is typed.
            KeyCode::Char('/') => {
                self.searching = true;
                self.filter = Some(String::new());
                self.apply_filter();
            }

            // A search is undone before the screen is left.
            KeyCode::Esc if self.filter.is_some() => self.clear_filter(),

            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous(),
            KeyCode::Home | KeyCode::Char('g') => self.state.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.state.select_last(),
            KeyCode::PageDown => self.state.scroll_down_by(10),
            KeyCode::PageUp => self.state.scroll_up_by(10),

            // Descend into the highlighted folder.
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(path) = self.highlighted().map(Path::to_path_buf) {
                    self.open(&path);
                }
            }

            // Walk back up.
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => {
                if let Some(parent) = self.cwd.parent().map(Path::to_path_buf) {
                    self.open(&parent);
                }
            }

            KeyCode::Char('~') => {
                if let Some(home) = dirs::home_dir() {
                    self.open(&home);
                }
            }

            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                let cwd = self.cwd.clone();
                self.open(&cwd);
            }

            // Choose the folder being listed, which is the one named in the title.
            KeyCode::Char('s') | KeyCode::Char(' ') => return self.choose(config),

            KeyCode::Esc | KeyCode::Char('q') => return Some(FolderBrowserOutcome::Cancel),

            _ => {}
        }

        None
    }

    /// Set the listed folder as the default folder and persist it.
    ///
    /// The config is written here rather than by the caller so that a failure can be reported in
    /// place, with the browser still open.
    fn choose(&mut self, config: &mut Config) -> Option<FolderBrowserOutcome> {
        let chosen = self.cwd.clone();

        if let Err(err) = config.set_default_folder(&chosen) {
            self.failed(format!("{}: {err}", chosen.display()));
            return None;
        }

        if let Err(err) = config.save() {
            self.failed(format!("Chose the folder, but could not save the config: {err}"));
            return None;
        }

        Some(FolderBrowserOutcome::Chosen(chosen))
    }

    fn select_next(&mut self) {
        match self.state.selected() {
            Some(index) if index + 1 >= self.visible.len() => self.state.select_first(),
            _ => self.state.select_next(),
        }
    }

    fn select_previous(&mut self) {
        match self.state.selected() {
            Some(0) | None if !self.visible.is_empty() => {
                self.state.select(Some(self.visible.len() - 1))
            }
            _ => self.state.select_previous(),
        }
    }

    /// Draw the browser in `area`.
    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        // A browser wants room, but a very wide terminal does not need a very wide list.
        let browser_area = area.centered(Constraint::Max(80), Constraint::Max(24));

        let mut block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(self.theme.border(true))
            .title(Line::from(" Select Folder ").centered().style(self.theme.title()))
            .padding(Padding::horizontal(1));

        if self.hints {
            block = block.title_bottom(
                Line::from(" enter open · s select · / find · esc cancel ")
                    .centered()
                    .style(self.theme.muted()),
            );
        }

        let inner = block.inner(browser_area);
        frame.render_widget(block, browser_area);

        // The path being listed, then the entries, then a summary of what is here.
        let [path_area, list_area, status_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        frame.render_widget(
            Line::from(truncate_left(&self.cwd.display().to_string(), path_area.width as usize))
                .style(self.theme.accent().add_modifier(ratatui::style::Modifier::BOLD)),
            path_area,
        );

        let items: Vec<ListItem<'_>> = self
            .visible
            .iter()
            .map(|index| (*index, &self.entries[*index]))
            .map(|(index, path)| {
                // The first entry is the parent, when there is one.
                let is_parent = index == 0 && self.cwd.parent() == Some(path.as_path());

                let name = if is_parent {
                    PARENT.to_string()
                } else {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string())
                };

                ListItem::new(Line::from(vec![Span::from(" "), Span::from(name)]))
            })
            .collect();

        let list = List::new(items).highlight_symbol("▶ ").highlight_style(self.theme.highlight());

        frame.render_stateful_widget(list, list_area, &mut self.state);

        if self.searching {
            let query = self.filter.clone().unwrap_or_default();

            frame.render_widget(
                Line::from(vec![
                    Span::from("/").style(self.theme.accent()),
                    Span::from(query).style(self.theme.text()),
                    Span::from("█").style(self.theme.accent()),
                ]),
                status_area,
            );

            return;
        }

        if let Some(query) = &self.filter {
            let matches = self.visible.len().saturating_sub(usize::from(self.cwd.parent().is_some()));

            frame.render_widget(
                Line::from(format!("/{query} — {matches} found")).style(self.theme.accent()),
                status_area,
            );

            return;
        }

        let status = match self.status.as_ref().filter(|_| self.messages) {
            Some(status) if status.is_error => {
                Line::from(status.text.as_str()).style(self.theme.error())
            }
            Some(status) => Line::from(status.text.as_str()).style(self.theme.success()),
            // The summary says what is in the folder, which is not a report of an action.
            None => Line::from(self.summary()).style(self.theme.muted()),
        };

        frame.render_widget(status, status_area);
    }

    /// What this folder holds, as a line under the listing.
    fn summary(&self) -> String {
        let folders = self.entries.len() - usize::from(self.cwd.parent().is_some());

        let hidden = if self.show_hidden { ", hidden shown" } else { "" };

        match self.audio_here {
            0 => format!("{folders} folders, no audio files here{hidden}"),
            1 => format!("{folders} folders, 1 audio file here{hidden}"),
            count => format!("{folders} folders, {count} audio files here{hidden}"),
        }
    }
}

/// Shorten a path to `width` cells, keeping the end, which is the part that identifies the folder.
fn truncate_left(text: &str, width: usize) -> String {
    let count = text.chars().count();

    if count <= width || width == 0 {
        return text.to_string();
    }

    // One cell goes to the ellipsis that marks the cut.
    let tail: String = text.chars().skip(count - width.saturating_sub(1)).collect();

    format!("…{tail}")
}
