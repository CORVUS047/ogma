//! The playlists pane: keep named lists of tracks, and send one to the queue.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::Modifier;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Padding};

use crate::player::Controls;
use crate::playlist::Playlist;
use crate::song::Song;
use crate::theme::Theme;

/// What the pane asks the screen to do after handling a key.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaylistAction {
    /// The key belonged to the pane and needs nothing further.
    Handled,
    /// Show this playlist's tracks in the file listing.
    Open(Playlist),
}

/// What the pane is doing: browsing, or typing a new playlist's name.
#[derive(Debug)]
enum Mode {
    Browsing,
    Naming { buffer: String },
    /// Typing a search, which narrows the list to matching names.
    Searching,
    /// A delete has been asked for and is waiting to be confirmed.
    ConfirmDelete,
}

/// The playlists on disk, and where the user is among them.
#[derive(Debug)]
pub struct PlaylistPane {
    playlists: Vec<Playlist>,
    state: ListState,
    mode: Mode,
    status: Option<String>,
    /// Names matching this are the only ones listed, when there is one.
    filter: Option<String>,
    /// Indices into `playlists` that the filter admits.
    visible: Vec<usize>,
    /// Whether to list the keys under the playlists.
    hints: bool,
    /// Whether to report what an action did.
    messages: bool,
    /// What a new playlist would hold: what is playing, then the queue. Kept up to date by the screen
    /// while it draws, since the queue belongs to the daemon rather than to this pane.
    to_save: Vec<Song>,
    /// Colours to draw with.
    theme: Theme,
}

impl Default for PlaylistPane {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaylistPane {
    /// Load the playlists from the playlists directory.
    pub fn new() -> Self {
        Self::with_playlists(Playlist::load_all())
    }

    /// Start with `playlists` rather than whatever is on disk, for tests and for callers that keep
    /// their playlists elsewhere.
    pub fn with_playlists(playlists: Vec<Playlist>) -> Self {
        let selected = (!playlists.is_empty()).then_some(0);

        let mut pane = PlaylistPane {
            visible: (0..playlists.len()).collect(),
            playlists,
            state: ListState::default().with_selected(selected),
            mode: Mode::Browsing,
            status: None,
            filter: None,
            hints: true,
            messages: true,
            to_save: Vec::new(),
            theme: Theme::default(),
        };

        pane.apply_filter();
        pane
    }

    /// Whether keys are going into a name or a search rather than being commands.
    pub fn is_typing(&self) -> bool {
        matches!(self.mode, Mode::Naming { .. } | Mode::Searching)
    }

    /// What the search is narrowing the list to, if anything.
    pub fn filter(&self) -> Option<&str> {
        self.filter.as_deref()
    }

    /// Work out which playlists the filter admits, keeping the highlight inside them.
    fn apply_filter(&mut self) {
        self.visible = match &self.filter {
            None => (0..self.playlists.len()).collect(),
            Some(query) => {
                let needle = query.to_lowercase();

                (0..self.playlists.len())
                    .filter(|index| self.playlists[*index].name().to_lowercase().contains(&needle))
                    .collect()
            }
        };

        self.state.select((!self.visible.is_empty()).then(|| {
            self.state.selected().unwrap_or(0).min(self.visible.len() - 1)
        }));
    }

    /// Draw with `theme` rather than whatever was set before.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Show or hide the key reminders under the playlists.
    pub fn set_hints(&mut self, hints: bool) {
        self.hints = hints;
    }

    /// Show or hide the reports of what an action did.
    pub fn set_messages(&mut self, messages: bool) {
        self.messages = messages;
    }

    /// What a new playlist should hold, were one saved now.
    ///
    /// The queue is the daemon's, so the screen hands it over rather than the pane reaching for it.
    pub fn set_queue_to_save(&mut self, songs: Vec<Song>) {
        self.to_save = songs;
    }

    /// The playlist under the cursor.
    pub fn selected(&self) -> Option<&Playlist> {
        self.playlists.get(*self.visible.get(self.state.selected()?)?)
    }

    fn selected_mut(&mut self) -> Option<&mut Playlist> {
        let index = *self.visible.get(self.state.selected()?)?;

        self.playlists.get_mut(index)
    }

    pub fn playlists(&self) -> &[Playlist] {
        &self.playlists
    }

    /// Add `songs` to the selected playlist and save it, reporting what happened.
    ///
    /// This is how the file listing's "add to playlist" arrives: the pane owns the playlists, so it
    /// owns the writing too.
    pub fn add_songs(&mut self, songs: Vec<Song>) {
        if songs.is_empty() {
            self.status = Some("nothing to add".to_string());
            return;
        }

        let Some(playlist) = self.selected_mut() else {
            self.status = Some("no playlist selected".to_string());
            return;
        };

        let added = playlist.add_all(songs);
        let name = playlist.name().to_string();

        // Saving straight away means a playlist is never only half in memory.
        match self.selected().map(Playlist::save) {
            Some(Ok(_)) => {
                self.status = Some(match added {
                    0 => format!("already in {name}"),
                    1 => format!("added 1 to {name}"),
                    count => format!("added {count} to {name}"),
                })
            }
            Some(Err(err)) => self.status = Some(format!("cannot save: {err}")),
            None => {}
        }
    }

    /// Take `song` out of the playlist called `name` and save it.
    ///
    /// Returns the playlist as it now stands, for the file listing the row came from to show again,
    /// or why it could not be done. Like adding, removing spans two panes — the file listing says
    /// which track, this pane owns the playlist — so the screen routes it here.
    ///
    /// The write happens before the change is kept, so a playlist that cannot be saved is left as
    /// it was rather than losing a track in memory only.
    pub fn remove_song(&mut self, name: &str, song: &Song) -> Result<Playlist, String> {
        let outcome = self.removing(name, song);

        self.status = Some(match &outcome {
            Ok(playlist) => format!("removed {} from {}", song.display_title(), playlist.name()),
            Err(message) => message.clone(),
        });

        outcome
    }

    /// The work behind [`PlaylistPane::remove_song`], leaving the message to it.
    fn removing(&mut self, name: &str, song: &Song) -> Result<Playlist, String> {
        let index = self
            .playlists
            .iter()
            .position(|playlist| playlist.name() == name)
            .ok_or_else(|| format!("no playlist called {name}"))?;

        let mut updated = self.playlists[index].clone();

        if !updated.remove_song(song) {
            return Err(format!("{} is not in {name}", song.display_title()));
        }

        updated.save().map_err(|err| format!("cannot save: {err}"))?;
        self.playlists[index] = updated.clone();

        Ok(updated)
    }

    /// Handle a key press, acting on `player` where the key asks for it.
    ///
    /// Returns `true` when the key belonged to this pane.
    pub fn handle_key(&mut self, key: KeyEvent, player: &mut dyn Controls) -> Option<PlaylistAction> {
        if key.kind != KeyEventKind::Press {
            return None;
        }

        // Tab belongs to the panes, whatever is being typed.
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            return None;
        }

        if let Mode::Searching = self.mode {
            self.handle_search_key(key);
            return Some(PlaylistAction::Handled);
        }

        if let Mode::Naming { .. } = self.mode {
            return self.handle_naming_key(key, player).then_some(PlaylistAction::Handled);
        }

        // A pending delete is answered by the next key, whatever it is.
        if let Mode::ConfirmDelete = self.mode {
            self.mode = Mode::Browsing;

            if key.code == KeyCode::Char('d') {
                self.delete_selected();
                return Some(PlaylistAction::Handled);
            }

            self.status = Some("delete cancelled".to_string());
            return Some(PlaylistAction::Handled);
        }

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous(),

            // Send the playlist to the queue, in the order the playlist is sorted.
            KeyCode::Enter => self.load_into_queue(player),

            // Open it in the file listing, to look through without disturbing the queue.
            KeyCode::Char('o') => match self.selected().cloned() {
                Some(playlist) => {
                    self.status = Some(format!("opened {}", playlist.name()));
                    return Some(PlaylistAction::Open(playlist));
                }
                None => self.status = Some("no playlists yet".to_string()),
            },

            // Narrow the list to names matching what is typed.
            KeyCode::Char('/') => {
                self.mode = Mode::Searching;
                self.filter = Some(String::new());
                self.status = None;
                self.apply_filter();
            }

            // A search is undone before anything else, so escape does the nearer thing first.
            KeyCode::Esc if self.filter.is_some() => self.clear_filter(),

            // Cycle the sort, and reverse it. Both are part of the playlist, so both are saved.
            KeyCode::Char('s') => self.cycle_sort(),
            KeyCode::Char('r') => self.reverse_sort(),

            // A new playlist takes what is queued now, which is the useful thing to save.
            KeyCode::Char('c') => {
                self.mode = Mode::Naming { buffer: String::new() };
                self.status = None;
            }

            KeyCode::Char('d') => {
                if self.selected().is_some() {
                    self.mode = Mode::ConfirmDelete;
                    self.status = Some("press d again to delete".to_string());
                }
            }

            _ => return None,
        }

        Some(PlaylistAction::Handled)
    }

    /// Keys while typing a search.
    fn handle_search_key(&mut self, key: KeyEvent) {
        let Some(query) = &mut self.filter else {
            self.mode = Mode::Browsing;
            return;
        };

        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => query.clear(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => query.push(c),
            KeyCode::Backspace => {
                query.pop();
            }
            // Keep what was typed and go back to the list, which is now narrowed.
            KeyCode::Enter | KeyCode::Down | KeyCode::Up => {
                self.mode = Mode::Browsing;

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

    /// Put the whole list back.
    fn clear_filter(&mut self) {
        self.filter = None;
        self.mode = Mode::Browsing;
        self.apply_filter();
    }

    fn handle_naming_key(&mut self, key: KeyEvent, _player: &mut dyn Controls) -> bool {
        let Mode::Naming { buffer } = &mut self.mode else {
            return false;
        };

        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => buffer.clear(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => buffer.push(c),
            KeyCode::Backspace => {
                buffer.pop();
            }
            KeyCode::Esc => {
                self.mode = Mode::Browsing;
                self.status = Some("cancelled".to_string());
            }
            KeyCode::Enter => {
                let name = buffer.trim().to_string();
                self.mode = Mode::Browsing;

                self.create(name);
            }
            _ => {}
        }

        true
    }

    /// Save what the screen last handed over as a new playlist.
    fn create(&mut self, name: String) {
        if name.is_empty() {
            self.status = Some("a playlist needs a name".to_string());
            return;
        }

        let playlist = Playlist::with_songs(name.clone(), self.to_save.clone());

        if playlist.file_stem().is_empty() {
            self.status = Some("name needs letters".to_string());
            return;
        }

        match playlist.save() {
            Ok(_) => {
                let count = playlist.len();

                // Keep the list sorted by name, and leave the cursor on what was just made.
                self.playlists.push(playlist);
                self.playlists.sort_by_key(|playlist| playlist.name().to_lowercase());

                let index = self
                    .playlists
                    .iter()
                    .position(|playlist| playlist.name() == name)
                    .unwrap_or(0);

                self.filter = None;
                self.apply_filter();
                self.state.select(Some(index));
                self.status = Some(format!("saved {name} with {count}"));
            }
            Err(err) => self.status = Some(format!("cannot save: {err}")),
        }
    }

    fn delete_selected(&mut self) {
        let Some(index) = self.state.selected().and_then(|row| self.visible.get(row).copied()) else {
            return;
        };

        let playlist = self.playlists.remove(index);

        match playlist.delete() {
            Ok(()) => self.status = Some(format!("deleted {}", playlist.name())),
            Err(err) => self.status = Some(format!("cannot delete: {err}")),
        }

        // Keep the cursor inside what is left of the listing.
        self.apply_filter();
    }

    /// Replace the queue with the selected playlist.
    fn load_into_queue(&mut self, player: &mut dyn Controls) {
        let Some(playlist) = self.selected() else {
            self.status = Some("no playlists yet".to_string());
            return;
        };

        player.replace_queue_with_playlist(playlist);

        self.status = Some(format!(
            "queued {} from {} by {}",
            playlist.len(),
            playlist.name(),
            playlist.sort().label()
        ));
    }

    fn cycle_sort(&mut self) {
        let Some(playlist) = self.selected_mut() else {
            return;
        };

        let sort = playlist.cycle_sort();
        let saved = self.selected().map(Playlist::save);

        self.status = Some(match saved {
            Some(Err(err)) => format!("cannot save: {err}"),
            // `r` has nowhere to fit in the hints, so the moment it is useful is where it is named.
            _ => format!("by {} · r reverses", sort.label()),
        });
    }

    fn reverse_sort(&mut self) {
        let Some(playlist) = self.selected_mut() else {
            return;
        };

        let descending = playlist.toggle_direction();
        let sort = playlist.sort();
        let saved = self.selected().map(Playlist::save);

        self.status = Some(match saved {
            Some(Err(err)) => format!("cannot save: {err}"),
            _ => format!(
                "by {} {}",
                sort.label(),
                if descending { "descending" } else { "ascending" }
            ),
        });
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

    /// Draw the pane in `area`. `focused` brightens the border and the selection.
    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(Line::from(" Playlists ").centered().style(self.theme.title()))
            .border_style(self.theme.border(focused))
            .padding(Padding::horizontal(1));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.height == 0 {
            return;
        }

        // A row for messages and the naming prompt, then the keys. The message never takes a key
        // row: hints that vanish as soon as the pane is used are no use.
        let keys_rows = if self.hints { 2 } else { 0 };

        let [list_area, status_area, keys_area] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(keys_rows),
        ])
        .areas(inner);

        let width = inner.width as usize;

        if self.visible.is_empty() && !matches!(self.mode, Mode::Naming { .. }) {
            let message = if self.filter.is_some() { "No matches" } else { "No playlists yet" };

            frame.render_widget(Line::from(message).style(self.theme.muted()), list_area);
        } else {
            let items: Vec<ListItem<'_>> = self
                .visible
                .iter()
                .map(|index| &self.playlists[*index])
                .map(|playlist| {
                    // The sort belongs to the playlist, so it is shown with it.
                    let detail = format!(
                        "{} · {}{}",
                        playlist.len(),
                        playlist.sort().label(),
                        if playlist.descending() { " ↓" } else { "" }
                    );

                    let room = width.saturating_sub(detail.chars().count() + 3);

                    ListItem::new(Line::from(vec![
                        Span::from(truncate(playlist.name(), room)).style(self.theme.text()),
                        Span::from(" "),
                        Span::from(detail).style(self.theme.muted()),
                    ]))
                })
                .collect();

            let list = List::new(items).highlight_symbol("› ").highlight_style(if focused {
                self.theme.highlight()
            } else {
                self.theme.highlight_unfocused()
            });

            frame.render_stateful_widget(list, list_area, &mut self.state);
        }

        // The keys the pane answers do not fit on one line, so they take two — with a message, when
        // there is one, in place of the first.
        // Split so both halves fit the pane's width rather than being cut off. Keys are named in
        // words: the symbols for enter and backspace are missing from plenty of terminal fonts.
        let keys: [Line<'_>; 2] = [
            Line::from("enter load · o open · c new").style(self.theme.muted()),
            Line::from("s sort · d del · / find").style(self.theme.muted()),
        ];

        // The naming prompt is not a message: it is the thing being typed into, so it takes the
        // status row, and the keys below it become the prompt's own.
        if let Mode::Searching = self.mode {
            let query = self.filter.clone().unwrap_or_default();

            frame.render_widget(
                Line::from(vec![
                    Span::from("/").style(self.theme.accent()),
                    Span::from(truncate(&query, width.saturating_sub(2))).style(self.theme.text()),
                    Span::from("█").style(self.theme.accent()),
                ]),
                status_area,
            );

            if self.hints && focused {
                frame.render_widget(
                    ratatui::widgets::Paragraph::new(vec![
                        Line::from("enter keep · esc cancel").style(self.theme.muted()),
                        Line::from("tab still changes pane").style(self.theme.muted()),
                    ]),
                    keys_area,
                );
            }

            return;
        }

        if let Some(query) = &self.filter {
            frame.render_widget(
                Line::from(truncate(
                    &format!("/{query} — {} found", self.visible.len()),
                    width,
                ))
                .style(self.theme.accent()),
                status_area,
            );

            if focused && self.hints {
                frame.render_widget(ratatui::widgets::Paragraph::new(keys.to_vec()), keys_area);
            }

            return;
        }

        match (&self.mode, &self.status) {
            (Mode::Naming { buffer }, _) => {
                frame.render_widget(
                    Line::from(vec![
                        Span::from("name: ").style(self.theme.text()),
                        Span::from(buffer.clone())
                            .style(self.theme.text().add_modifier(Modifier::UNDERLINED)),
                        Span::from("█").style(self.theme.accent()),
                    ]),
                    status_area,
                );

                if self.hints {
                    frame.render_widget(
                        Line::from("enter save · esc cancel").style(self.theme.muted()),
                        keys_area,
                    );
                }

                return;
            }
            (_, Some(status)) if self.messages => frame.render_widget(
                Line::from(truncate(status, width)).style(self.theme.success()),
                status_area,
            ),
            (_, Some(_)) => {}
            (_, None) => {}
        }

        if focused && self.hints {
            frame.render_widget(ratatui::widgets::Paragraph::new(keys.to_vec()), keys_area);
        }
    }
}

/// Cut `text` to `width` cells, marking the cut with an ellipsis.
fn truncate(text: &str, width: usize) -> String {
    let count = text.chars().count();

    if count <= width {
        return text.to_string();
    }
    if width <= 1 {
        return text.chars().take(width).collect();
    }

    text.chars().take(width - 1).collect::<String>() + "…"
}
