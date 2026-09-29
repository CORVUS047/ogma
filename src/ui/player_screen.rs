//! The playback screen: playlists on the left, the current song in the middle, the queue on the
//! right.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Padding};

use crate::player::{Controls, PlaybackState, Player, Repeat};
use crate::song::Song;
use crate::theme::Theme;
use crate::volume;
use crate::ui::artwork_view::ArtworkView;
use crate::ui::file_pane::FilePane;
use crate::ui::playlist_pane::{PlaylistAction, PlaylistPane};

/// How far one seek key moves.
const SEEK_STEP: Duration = Duration::from_secs(5);

/// How much one volume key moves.
const VOLUME_STEP: f32 = 0.05;

/// How many played songs are shown above the current one.
const HISTORY_SHOWN: usize = 3;

/// Cells in the volume bar.
const VOLUME_CELLS: usize = 10;

/// Which pane the keys act on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Focus {
    Playlists,
    Files,
    Queue,
}

/// What the screen asks the application to do next.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlayerScreenOutcome {
    /// Leave the playback screen.
    Close,
}

/// The playback screen and its view state. The player itself lives in the application.
#[derive(Debug)]
pub struct PlayerScreen {
    focus: Focus,
    /// Selection within the upcoming queue.
    queue_state: ListState,
    artwork: ArtworkView,
    /// The multimodal panel in the left column, for filling the queue.
    files: FilePane,
    /// The playlists above it.
    playlists: PlaylistPane,
    /// Whether the keys are listed on screen.
    hints: bool,
    /// Colours to draw with.
    theme: Theme,
}

impl Default for PlayerScreen {
    fn default() -> Self {
        PlayerScreen {
            // The queue starts empty, so the files are where the work begins.
            focus: Focus::Files,
            queue_state: ListState::default().with_selected(Some(0)),
            artwork: ArtworkView::new(),
            files: FilePane::new(),
            playlists: PlaylistPane::new(),
            hints: true,
            theme: Theme::default(),
        }
    }
}

impl PlayerScreen {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open with the file listing rooted at `root`.
    pub fn browsing(root: &std::path::Path) -> Self {
        PlayerScreen { files: FilePane::at(root), ..Self::default() }
    }

    /// Open with the given playlists rather than whatever is on disk.
    pub fn with_playlists(root: &std::path::Path, playlists: Vec<crate::playlist::Playlist>) -> Self {
        PlayerScreen {
            files: FilePane::at(root),
            playlists: PlaylistPane::with_playlists(playlists),
            ..Self::default()
        }
    }

    /// Draw with `theme`, here and in both side panes.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.files.set_theme(theme);
        self.playlists.set_theme(theme);
    }

    /// Show or hide the reports of what an action did, in both side panes.
    pub fn set_messages(&mut self, messages: bool) {
        self.files.set_messages(messages);
        self.playlists.set_messages(messages);
    }

    /// Take in whatever the multimodal panel's background work has finished with.
    pub fn poll(&mut self) {
        self.files.poll();
    }

    /// Whether the screen should be cleared before it is drawn again, taking the flag.
    pub fn take_repaint(&mut self) -> bool {
        self.files.take_repaint()
    }

    /// Pass the settings the panel's YouTube mode works from.
    pub fn set_youtube(
        &mut self,
        search_results: usize,
        format: crate::ytdl::Format,
        folder: std::path::PathBuf,
    ) {
        self.files.set_search_results(search_results);
        self.files.set_download_format(format);
        self.files.set_download_folder(folder);
    }

    /// Show or hide the key reminders, here and in both side panes.
    pub fn set_hints(&mut self, hints: bool) {
        self.hints = hints;
        self.files.set_hints(hints);
        self.playlists.set_hints(hints);
    }

    /// The playlists the screen is showing.
    pub fn playlists(&self) -> &[crate::playlist::Playlist] {
        self.playlists.playlists()
    }

    /// The folder the file listing is showing.
    pub fn browsing_path(&self) -> &std::path::Path {
        self.files.cwd()
    }

    /// Handle a key press against `player`, returning what the application should do next.
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        player: &mut dyn Controls,
    ) -> Option<PlayerScreenOutcome> {
        if key.kind != KeyEventKind::Press {
            return None;
        }

        // Keys the focused pane claims are its own; the rest fall through to the transport below.
        // Tab is never a pane's to take, so panes can always be changed. Escape and `q` are the
        // screen's too — unless the pane is being typed into, where a `q` is a letter and escape ends
        // what is being typed rather than the screen.
        let typing = match self.focus {
            Focus::Files => self.files.is_typing(),
            Focus::Playlists => self.playlists.is_typing(),
            Focus::Queue => false,
        };

        let pane_keys = if typing {
            !matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
        } else {
            !matches!(
                key.code,
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc | KeyCode::Char('q')
            )
        };

        // Adding to a playlist spans two panes: the files say what, the playlists say where.
        if pane_keys && self.focus == Focus::Files && key.code == KeyCode::Char('P') {
            let songs = self.files.highlighted_songs();

            if songs.is_empty() {
                self.files.report("nothing to add");
            } else if self.playlists.selected().is_none() {
                self.files.report("no playlist selected");
            } else {
                let count = songs.len();
                self.playlists.add_songs(songs);
                self.files.report(format!("sent {count} to playlist"));
            }

            return None;
        }

        if pane_keys && self.focus == Focus::Playlists {
            match self.playlists.handle_key(key, player) {
                // Opening a playlist puts it in the file listing, where it can be looked through.
                Some(PlaylistAction::Open(playlist)) => {
                    self.files.show_playlist(&playlist);
                    self.focus = Focus::Files;
                    return None;
                }
                Some(PlaylistAction::Handled) => return None,
                None => {}
            }
        }

        if pane_keys && self.focus == Focus::Files && self.files.handle_key(key, player) {
            return None;
        }

        // A search left in place is undone by escape before the screen is.
        if key.code == KeyCode::Esc {
            let cleared = match self.focus {
                Focus::Files => self.files.filter().is_some(),
                Focus::Playlists => self.playlists.filter().is_some(),
                Focus::Queue => false,
            };

            if cleared {
                match self.focus {
                    Focus::Files => self.files.handle_key(key, player),
                    Focus::Playlists => self.playlists.handle_key(key, player).is_some(),
                    Focus::Queue => false,
                };

                return None;
            }
        }

        match key.code {
            // Transport, on the keys a music player is expected to use.
            KeyCode::Char(' ') => player.toggle_pause(),
            KeyCode::Char('n') => player.skip(),
            KeyCode::Char('p') => player.previous(),
            KeyCode::Char('S') => player.stop(),
            // Upper case, like stop: the lower-case letter belongs to the playlists' sort.
            KeyCode::Char('R') => {
                player.cycle_repeat();
            }
            // Shuffling rearranges what is coming, not what is playing.
            KeyCode::Char('z') => player.shuffle_queue(),

            // Seeking.
            KeyCode::Left => player.seek(SEEK_STEP, false),
            KeyCode::Right => player.seek(SEEK_STEP, true),

            // Volume.
            KeyCode::Char('+') | KeyCode::Char('=') => player.change_volume(VOLUME_STEP),
            KeyCode::Char('-') | KeyCode::Char('_') => player.change_volume(-VOLUME_STEP),

            // Moving about the panes.
            KeyCode::Tab | KeyCode::Char('l') => self.focus = self.next_focus(),
            KeyCode::BackTab | KeyCode::Char('h') => self.focus = self.previous_focus(),

            KeyCode::Down | KeyCode::Char('j') if self.focus == Focus::Queue => {
                self.select_next(player.queue_len())
            }
            KeyCode::Up | KeyCode::Char('k') if self.focus == Focus::Queue => {
                self.select_previous(player.queue_len())
            }

            // Queue management.
            KeyCode::Enter if self.focus == Focus::Queue => self.play_selected(player),
            KeyCode::Char('x') | KeyCode::Delete if self.focus == Focus::Queue => {
                self.remove_selected(player)
            }

            KeyCode::Esc | KeyCode::Char('q') => return Some(PlayerScreenOutcome::Close),

            _ => {}
        }

        None
    }

    fn next_focus(&self) -> Focus {
        match self.focus {
            Focus::Playlists => Focus::Files,
            Focus::Files => Focus::Queue,
            Focus::Queue => Focus::Playlists,
        }
    }

    fn previous_focus(&self) -> Focus {
        match self.focus {
            Focus::Playlists => Focus::Queue,
            Focus::Files => Focus::Playlists,
            Focus::Queue => Focus::Files,
        }
    }

    fn select_next(&mut self, queued: usize) {
        if queued == 0 {
            return;
        }

        match self.queue_state.selected() {
            Some(index) if index + 1 >= queued => self.queue_state.select_first(),
            _ => self.queue_state.select_next(),
        }
    }

    fn select_previous(&mut self, queued: usize) {
        if queued == 0 {
            return;
        }

        match self.queue_state.selected() {
            Some(0) | None => self.queue_state.select(Some(queued - 1)),
            _ => self.queue_state.select_previous(),
        }
    }

    /// Start the highlighted queue entry now, sending the current song to the history.
    fn play_selected(&mut self, player: &mut dyn Controls) {
        if let Some(index) = self.queue_state.selected() {
            player.play_queued(index);
        }
    }

    /// Drop the highlighted entry from the queue.
    fn remove_selected(&mut self, player: &mut dyn Controls) {
        if let Some(index) = self.queue_state.selected() {
            player.remove_queue(index);
        }
    }

    /// Keep the selection inside the queue after it has shrunk.
    fn clamp_selection(&mut self, len: usize) {
        match (len, self.queue_state.selected()) {
            (0, _) => self.queue_state.select(None),
            (len, Some(index)) if index >= len => self.queue_state.select(Some(len - 1)),
            (_, None) => self.queue_state.select(Some(0)),
            _ => {}
        }
    }

    /// Draw the screen across `area`.
    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect, player: &Player) {
        // The side columns keep a fixed share; the song takes whatever is left.
        let [left_area, now_area, queue_area] = Layout::horizontal([
            Constraint::Length(33),
            Constraint::Fill(1),
            Constraint::Length(35),
        ])
        .areas(area);

        // Playlists sit above the files, which take the rest of the column: a listing wants room.
        let [playlists_area, files_area] =
            Layout::vertical([Constraint::Length(10), Constraint::Fill(1)]).areas(left_area);

        // What a new playlist would hold, were one saved now: the current song leads, since the queue
        // alone would lose it.
        let mut to_save: Vec<Song> = player.current().cloned().into_iter().collect();
        to_save.extend(player.queue().iter().cloned());
        self.playlists.set_queue_to_save(to_save);

        self.playlists.render(frame, playlists_area, self.focus == Focus::Playlists);
        self.files.render(frame, files_area, self.focus == Focus::Files);
        self.render_now_playing(frame, now_area, player);
        self.render_queue(frame, queue_area, player);
    }

    /// The middle column: artwork, then what is playing, then how far through it is.
    fn render_now_playing(&mut self, frame: &mut Frame<'_>, area: Rect, player: &Player) {
        let block = pane(" Now Playing ", false, &self.theme).padding(Padding::horizontal(1));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Artwork takes the room the text does not need: three lines of song, two of transport.
        let [art_area, title_area, transport_area] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(4),
            Constraint::Length(3),
        ])
        .areas(inner);

        let Some(song) = player.current() else {
            self.artwork.render(frame, art_area, std::path::Path::new(""), None, &self.theme);
            frame.render_widget(
                Line::from("Nothing playing").style(self.theme.muted()).centered(),
                centered_row(title_area),
            );

            // The volume and the keys are worth showing with nothing playing: the fader is what the
            // next song will come out at, and this is where someone looks to find out how to move it.
            self.render_transport(frame, transport_area, player, None);

            return;
        };

        self.artwork.render(frame, art_area, song.path(), song.front_cover(), &self.theme);

        let mut lines = vec![
            Line::from(song.display_title())
                .style(self.theme.text().add_modifier(Modifier::BOLD))
                .centered(),
            Line::from(song.display_artist().to_string()).style(self.theme.accent()).centered(),
            Line::from(song.display_album().to_string()).style(self.theme.muted()).centered(),
        ];

        // The year and track position, when the file says.
        let mut details = Vec::new();
        if let (Some(track), Some(total)) = (song.track_number(), song.track_total()) {
            details.push(format!("track {track}/{total}"));
        } else if let Some(track) = song.track_number() {
            details.push(format!("track {track}"));
        }
        if let Some(year) = song.year() {
            details.push(year.to_string());
        }
        if let Some(codec) = song.codec() {
            details.push(codec.to_string());
        }
        lines.push(Line::from(details.join(" · ")).style(self.theme.muted()).centered());

        frame.render_widget(ratatui::widgets::Paragraph::new(lines), title_area);

        self.render_transport(frame, transport_area, player, Some(song));
    }

    /// The progress bar, the clock, and what the player is doing.
    ///
    /// Without a song there is no progress to draw, but the volume and the keys are still worth the
    /// rows: they say what the player will do next, not what it is doing.
    fn render_transport(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        player: &Player,
        song: Option<&Song>,
    ) {
        let [bar_area, state_area, keys_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);

        if let Some(song) = song {
            let elapsed = format_time(player.position());
            let total = song.display_duration();

            // Both clocks and the spaces around the bar have to fit, or the line gets clipped.
            let clocks = elapsed.chars().count() + total.chars().count() + 2;
            let cells = (bar_area.width as usize).saturating_sub(clocks);
            let progress = bar(player.progress().unwrap_or(0.0), cells);

            frame.render_widget(
                Line::from(vec![
                    Span::from(format!("{elapsed} ")).style(self.theme.text()),
                    Span::from(progress).style(self.theme.accent()),
                    Span::from(format!(" {total}")).style(self.theme.text()),
                ])
                .centered(),
                bar_area,
            );
        }

        // A filled triangle and a square are in almost every font; the media-control pause and stop
        // glyphs are not, so the pause marker is drawn with plain pipes.
        let (symbol, label) = match player.state() {
            PlaybackState::Playing => ("▶", "playing"),
            PlaybackState::Paused => ("||", "paused"),
            PlaybackState::Stopped => ("■", "stopped"),
        };

        // What goes on this line is decided by what fits, in order of how much each part is worth: the
        // decibel figure says what the fader is doing and cannot be inferred, so the bar gives up
        // cells before that figure is dropped. Clipping is never an option — a cut-off reading reads
        // as a wrong one.
        let state = format!("{symbol} {label}");
        let reading = volume::display_decibels(player.volume());
        let percent = format!("{:>3}%", volume::percent(player.volume()));
        let room = state_area.width as usize;

        /// Bar widths and trailing readings to try, widest and most informative first.
        const SHAPES: &[(usize, &str)] = &[
            (VOLUME_CELLS, "both"),
            (VOLUME_CELLS, "decibels"),
            (8, "decibels"),
            (6, "decibels"),
            (4, "decibels"),
            (VOLUME_CELLS, "percent"),
            (8, "percent"),
            (6, "none"),
        ];

        let mut state_line = vec![Span::from(state.clone()).style(
            self.theme.playing().add_modifier(Modifier::BOLD),
        )];

        for gap in ["  vol ", " "] {
            let base = state.chars().count() + gap.chars().count();

            let fitted = SHAPES.iter().find(|(cells, extras)| {
                let trailing = match *extras {
                    "both" => percent.chars().count() + reading.chars().count() + 3,
                    "decibels" => reading.chars().count() + 1,
                    "percent" => percent.chars().count() + 1,
                    _ => 0,
                };

                base + cells + trailing <= room
            });

            let Some((cells, extras)) = fitted else {
                continue;
            };

            state_line.push(Span::from(gap).style(self.theme.text()));
            state_line.push(Span::from(bar(player.volume(), *cells)).style(self.theme.accent()));

            match *extras {
                "both" => {
                    state_line.push(Span::from(format!(" {percent}")).style(self.theme.text()));
                    state_line.push(Span::from(format!("  {reading}")).style(self.theme.muted()));
                }
                "decibels" => {
                    state_line.push(Span::from(format!(" {reading}")).style(self.theme.muted()))
                }
                "percent" => {
                    state_line.push(Span::from(format!(" {percent}")).style(self.theme.text()))
                }
                _ => {}
            }

            break;
        }

        frame.render_widget(Line::from(state_line).centered(), state_area);

        // Four spellings, so a narrow pane shows a shorter reminder rather than a clipped one. The
        // keys are named in words, since the media-control glyphs are missing from many terminal
        // fonts. The volume keys survive into every spelling: the bar above says where the fader is,
        // and without them nothing on screen says how to move it.
        const LONG: &str = "space play/pause · n/p track · left/right seek · +/- volume · \
                            z shuffle · R repeat · S stop · tab pane · q back";
        const MEDIUM: &str = "space pause · n/p track · left/right seek · +/- volume · \
                              z shuffle · R repeat · q back";
        const SHORT: &str = "space pause · n/p track · +/- volume · R repeat · q back";
        const TINY: &str = "space · n/p · +/- vol · R repeat · q";
        // Keys and nothing else: past this width a word about any of them would push another key off.
        const MICRO: &str = "space · n/p · +/- · R · q";

        if !self.hints {
            return;
        }

        let room = keys_area.width as usize;
        let keys = [LONG, MEDIUM, SHORT, TINY, MICRO]
            .into_iter()
            .find(|candidate| candidate.chars().count() <= room)
            .unwrap_or("");

        frame.render_widget(Line::from(keys).style(self.theme.muted()).centered(), keys_area);
    }

    /// The right column: what has played, what is playing, what is next.
    fn render_queue(&mut self, frame: &mut Frame<'_>, area: Rect, player: &Player) {
        let focused = self.focus == Focus::Queue;

        // A queue that has shrunk since the last frame must not leave the selection past its end.
        self.clamp_selection(player.queue().len());

        // Repeating is about what comes next, so the queue is where it is said. Off is the ordinary
        // state and goes unmentioned; the title would otherwise carry a word that never changes.
        let title = match player.repeat() {
            Repeat::Off => " Queue ".to_string(),
            mode => format!(" Queue · {} ", mode.label()),
        };

        let block = pane(title, focused, &self.theme).padding(Padding::horizontal(1));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // The keys keep their row whether or not this pane has the focus, so the listing does not
        // change height as the focus moves between panes.
        let keys_rows = if self.hints { 1 } else { 0 };

        // The played songs and the current one sit at a fixed height, so the current song stays put
        // as the queue below it changes.
        let [context_area, upcoming_area, keys_area] = Layout::vertical([
            Constraint::Length(HISTORY_SHOWN as u16 + 1),
            Constraint::Fill(1),
            Constraint::Length(keys_rows),
        ])
        .areas(inner);

        self.render_queue_keys(frame, keys_area, focused);

        let width = inner.width as usize;
        let history = player.history();
        let mut context: Vec<Line<'static>> = Vec::with_capacity(HISTORY_SHOWN + 1);

        // Pad above so the current song holds the same row whether or not anything has played yet.
        let shown = history.len().min(HISTORY_SHOWN);
        for _ in 0..HISTORY_SHOWN - shown {
            context.push(Line::from(""));
        }
        for song in &history[history.len() - shown..] {
            context.push(
                Line::from(format!("  {}", entry(song, width.saturating_sub(2))))
                    .style(self.theme.muted()),
            );
        }

        context.push(match player.current() {
            Some(song) => Line::from(vec![
                Span::from("▶ ").style(self.theme.playing()),
                Span::from(entry(song, width.saturating_sub(2)))
                    .style(self.theme.text().add_modifier(Modifier::BOLD)),
            ]),
            None => Line::from("▶ nothing playing").style(self.theme.muted()),
        });

        frame.render_widget(ratatui::widgets::Paragraph::new(context), context_area);

        if player.queue().is_empty() {
            // Only say the queue is empty when there is genuinely nothing: with a song playing or
            // played, the rows above already show what is going on, and the message reads as a
            // contradiction.
            if player.current().is_none() && player.history().is_empty() {
                frame.render_widget(
                    Line::from("  queue empty").style(self.theme.muted()),
                    upcoming_area,
                );
            }

            return;
        }

        let items: Vec<ListItem<'_>> = player
            .queue()
            .iter()
            .map(|song| {
                ListItem::new(
                    Line::from(entry(song, width.saturating_sub(2))).style(self.theme.text()),
                )
            })
            .collect();

        let mut list = List::new(items).highlight_symbol("› ");

        // Highlighting only reads as a selection while the pane has the keys.
        list = if focused {
            list.highlight_style(self.theme.highlight())
        } else {
            list.highlight_style(self.theme.highlight_unfocused())
        };

        frame.render_stateful_widget(list, upcoming_area, &mut self.queue_state);
    }

    /// The queue's own keys, which nothing else on screen names.
    fn render_queue_keys(&self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        // Two spellings, so a narrow pane says less rather than showing a clipped line. The repeat key
        // is not among them: it belongs to the transport, which names it, and what it is set to is in
        // this pane's title.
        const LONG: &str = "enter play · x remove";
        const SHORT: &str = "enter · x";

        // Listed only while the pane has the keys, as the other panes' are: keys that do nothing to
        // what is highlighted would be a lie.
        if !self.hints || !focused {
            return;
        }

        let room = area.width as usize;
        let keys = [LONG, SHORT]
            .into_iter()
            .find(|candidate| candidate.chars().count() <= room)
            .unwrap_or("");

        frame.render_widget(Line::from(keys).style(self.theme.muted()).centered(), area);
    }
}

/// A pane border, brighter when it has the keys.
fn pane(title: impl Into<String>, focused: bool, theme: &Theme) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(title.into()).centered().style(theme.title()))
        .border_style(theme.border(focused))
}

/// One queue row: artist and title, cut to `width`.
fn entry(song: &Song, width: usize) -> String {
    let text = format!("{} — {}", song.display_artist(), song.display_title());

    truncate(&text, width)
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

/// A bar of filled and empty cells for a 0.0 to 1.0 level.
fn bar(level: f32, cells: usize) -> String {
    let filled = (level.clamp(0.0, 1.0) * cells as f32).round() as usize;

    "█".repeat(filled) + &"░".repeat(cells - filled)
}

/// `m:ss`.
fn format_time(duration: Duration) -> String {
    let total = duration.as_secs();

    format!("{}:{:02}", total / 60, total % 60)
}

/// The middle row of `area`, for a single centered line.
fn centered_row(area: Rect) -> Rect {
    Rect { y: area.y + area.height / 2, height: 1, ..area }
}
