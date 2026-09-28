//! The screen the player opens on: a vertical list of entry points.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Padding};

use crate::theme::Theme;

/// What the menu offers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StartMenuChoice {
    /// Go back to the player, which is still playing.
    Continue,
    /// Pick a folder to build the library from.
    SelectFolder,
    /// Open the config screen.
    OpenConfig,
    /// Leave the player.
    Quit,
}

impl StartMenuChoice {
    /// The entries always offered, in the order they are stacked.
    const ALWAYS: [StartMenuChoice; 3] = [Self::SelectFolder, Self::OpenConfig, Self::Quit];

    fn label(self) -> &'static str {
        match self {
            Self::Continue => "Continue",
            Self::SelectFolder => "Select Folder",
            Self::OpenConfig => "Open Config",
            Self::Quit => "Quit",
        }
    }

    /// One line of help, shown under the menu for the highlighted entry.
    fn hint(self) -> &'static str {
        match self {
            Self::Continue => "Back to what is playing",
            Self::SelectFolder => "Scan a folder for music",
            Self::OpenConfig => "Audio output, theme, keys",
            Self::Quit => "Close ogma",
        }
    }
}

/// The start menu and its selection.
#[derive(Debug)]
pub struct StartMenu {
    /// What this menu offers, which depends on whether there is a player to go back to.
    entries: Vec<StartMenuChoice>,
    state: ListState,
    /// Whether the keys are listed along the bottom.
    hints: bool,
    /// Colours to draw with.
    theme: Theme,
}

impl Default for StartMenu {
    fn default() -> Self {
        Self::new()
    }
}

impl StartMenu {
    /// The menu as it looks with nothing playing.
    pub fn new() -> Self {
        Self::with_continue(false)
    }

    /// The menu, offering a way back to the player when `resumable`.
    ///
    /// Continue comes first and starts highlighted, so leaving the player by accident costs one key
    /// to undo.
    pub fn with_continue(resumable: bool) -> Self {
        let mut entries = Vec::with_capacity(4);

        if resumable {
            entries.push(StartMenuChoice::Continue);
        }
        entries.extend(StartMenuChoice::ALWAYS);

        // Something must always be highlighted, so the first entry starts selected.
        StartMenu {
            entries,
            state: ListState::default().with_selected(Some(0)),
            hints: true,
            theme: Theme::default(),
        }
    }

    /// What the menu is offering.
    pub fn entries(&self) -> &[StartMenuChoice] {
        &self.entries
    }

    /// Draw with `theme` rather than whatever was set before.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Show or hide the key reminders.
    pub fn set_hints(&mut self, hints: bool) {
        self.hints = hints;
    }

    /// The highlighted entry.
    pub fn selected(&self) -> StartMenuChoice {
        let index = self.state.selected().unwrap_or(0);

        self.entries[index.min(self.entries.len() - 1)]
    }

    pub fn select_next(&mut self) {
        // Wrap around rather than sticking at the bottom.
        match self.state.selected() {
            Some(index) if index + 1 >= self.entries.len() => self.state.select_first(),
            _ => self.state.select_next(),
        }
    }

    pub fn select_previous(&mut self) {
        match self.state.selected() {
            Some(0) | None => self.state.select(Some(self.entries.len() - 1)),
            _ => self.state.select_previous(),
        }
    }

    /// Handle a key press, returning the entry the user committed to, if any.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<StartMenuChoice> {
        // Windows reports both press and release; only act on the press.
        if key.kind != KeyEventKind::Press {
            return None;
        }

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous(),
            KeyCode::Home | KeyCode::Char('g') => self.state.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.state.select_last(),
            KeyCode::Enter | KeyCode::Char(' ') => return Some(self.selected()),
            KeyCode::Esc | KeyCode::Char('q') => return Some(StartMenuChoice::Quit),
            _ => {}
        }

        None
    }

    /// Draw the menu centered in `area`.
    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        // Wide enough for the bottom title, which is the longest line in the box; tall enough for
        // the entries, a gap, the hint, the padding and the borders.
        // One row per entry, plus the title, hint, padding and borders.
        let height = self.entries.len() as u16 + 6;
        let menu_area = area.centered(Constraint::Length(38), Constraint::Length(height));

        let mut block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(self.theme.border(true))
            .title(Line::from(" ogma ").centered().style(self.theme.title()))
            .padding(Padding::uniform(1));

        if self.hints {
            block = block.title_bottom(
                Line::from(" up/down · enter select · q quit ")
                    .centered()
                    .style(self.theme.muted()),
            );
        }

        let inner = block.inner(menu_area);
        frame.render_widget(block, menu_area);

        // Entries on top, hint pinned at the bottom of the box with a blank row between.
        let [list_area, hint_area] =
            Layout::vertical([Constraint::Length(self.entries.len() as u16), Constraint::Length(1)])
                .flex(Flex::SpaceBetween)
                .areas(inner);

        let items: Vec<ListItem<'_>> = self
            .entries
            .iter()
            .map(|choice| ListItem::new(Line::from(choice.label())))
            .collect();

        let list = List::new(items)
            .highlight_symbol("▶ ")
            .highlight_style(self.theme.highlight().patch(self.theme.accent()));

        frame.render_stateful_widget(list, list_area, &mut self.state);

        frame.render_widget(
            Line::from(self.selected().hint()).style(self.theme.muted()).centered(),
            hint_area,
        );
    }
}
