//! Searching YouTube, and playing or keeping what it finds.
//!
//! One screen: a query at the top, the results under it, and a line at the bottom saying what the
//! last action did. A result can be streamed — played straight from the network, nothing written to
//! disk — or downloaded into the download folder, where it becomes an ordinary part of the library.
//!
//! The searching and the downloading both happen on threads of their own, so the screen keeps
//! drawing while YouTube is being asked. [`YoutubeScreen::poll`] is what brings their answers in,
//! and the application calls it on every tick.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Padding};

use crate::config::Config;
use crate::player::Controls;
use crate::theme::Theme;
use crate::ytdl::{self, Event, Track};

/// What the screen asks the application to do next.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum YoutubeOutcome {
    /// Leave the search screen.
    Close,
}

/// Whether keys are going into the query or into the results.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    /// Typing a query. Holds what has been typed so far.
    Typing(String),
    /// Moving through what came back.
    Browsing,
}

/// A short message about the last action, and whether it went wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Status {
    text: String,
    is_error: bool,
}

/// The search screen and everything in flight for it.
#[derive(Debug)]
pub struct YoutubeScreen {
    /// The query as it was last searched for, which is what the results belong to.
    query: String,
    mode: Mode,
    results: Vec<Track>,
    state: ListState,
    /// Which results are ticked, by position in `results`.
    marked: Vec<bool>,
    /// A search in flight, if one is.
    searching: Option<Receiver<Result<Vec<Track>, String>>>,
    /// A download in flight, if one is.
    downloading: Option<Receiver<Event>>,
    /// What the running download is on, and how far it has got.
    progress: Option<(String, f32)>,
    /// Where downloads are written, which the application works out from the config.
    folder: PathBuf,
    status: Option<Status>,
    /// Whether the keys are listed under the results.
    hints: bool,
    /// Whether to report what an action did.
    messages: bool,
    /// Colours to draw with.
    theme: Theme,
}

impl Default for YoutubeScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl YoutubeScreen {
    /// A screen with nothing searched for yet, waiting for a query.
    pub fn new() -> Self {
        let mut screen = YoutubeScreen {
            query: String::new(),
            // Opening the screen is asking to search, so the cursor starts in the query.
            mode: Mode::Typing(String::new()),
            results: Vec::new(),
            state: ListState::default(),
            marked: Vec::new(),
            searching: None,
            downloading: None,
            progress: None,
            folder: default_folder(),
            status: None,
            hints: true,
            messages: true,
            theme: Theme::default(),
        };

        // Saying this up front beats a search that silently finds nothing.
        if let Some(missing) = ytdl::missing() {
            screen.fail(missing);
        }

        screen
    }

    /// Write downloads into `folder` rather than the default.
    pub fn download_into(&mut self, folder: PathBuf) {
        self.folder = folder;
    }

    /// Where downloads are written.
    pub fn folder(&self) -> &std::path::Path {
        &self.folder
    }

    /// Draw with `theme` rather than whatever was set before.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Show or hide the key reminders.
    pub fn set_hints(&mut self, hints: bool) {
        self.hints = hints;
    }

    /// Show or hide the reports of what an action did.
    pub fn set_messages(&mut self, messages: bool) {
        self.messages = messages;
    }

    /// What the last search turned up.
    pub fn results(&self) -> &[Track] {
        &self.results
    }

    /// Put results in without asking YouTube, for a caller driving the screen itself.
    pub fn show_results(&mut self, results: Vec<Track>) {
        self.marked = vec![false; results.len()];
        self.state.select((!results.is_empty()).then_some(0));
        self.results = results;
        self.mode = Mode::Browsing;
        self.searching = None;
    }

    /// The results that an action applies to: the ticked ones, or the highlighted one when none are
    /// ticked.
    ///
    /// Ticking nothing and pressing a key should still do the obvious thing, which is to act on what
    /// is under the cursor.
    pub fn chosen(&self) -> Vec<Track> {
        let ticked: Vec<Track> = self
            .results
            .iter()
            .enumerate()
            .filter(|(index, _)| self.marked.get(*index).copied().unwrap_or(false))
            .map(|(_, track)| track.clone())
            .collect();

        if !ticked.is_empty() {
            return ticked;
        }

        self.highlighted().cloned().into_iter().collect()
    }

    fn highlighted(&self) -> Option<&Track> {
        self.results.get(self.state.selected()?)
    }

    /// Whether keys are going into the query, in which case they are not commands.
    pub fn is_typing(&self) -> bool {
        matches!(self.mode, Mode::Typing(_))
    }

    /// Whether a search or a download is still running.
    pub fn is_busy(&self) -> bool {
        self.searching.is_some() || self.downloading.is_some()
    }

    fn report(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), is_error: false });
    }

    fn fail(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), is_error: true });
    }

    /// Take in whatever the background search and download have to say.
    ///
    /// Called on every tick, and never blocks: an answer that has not arrived is simply not here
    /// yet.
    pub fn poll(&mut self) {
        self.collect_search();
        self.collect_download();
    }

    fn collect_search(&mut self) {
        let Some(searching) = &self.searching else {
            return;
        };

        match searching.try_recv() {
            Ok(Ok(results)) => {
                let count = results.len();
                self.show_results(results);

                match count {
                    0 => self.report("nothing found"),
                    1 => self.report("1 result"),
                    count => self.report(format!("{count} results")),
                }
            }
            Ok(Err(err)) => {
                self.searching = None;
                self.mode = Mode::Browsing;
                self.fail(err);
            }
            Err(TryRecvError::Empty) => {}
            // The thread went away without answering, which is the spawn having failed.
            Err(TryRecvError::Disconnected) => {
                self.searching = None;
                self.mode = Mode::Browsing;
                self.fail("the search could not be started");
            }
        }
    }

    fn collect_download(&mut self) {
        let Some(downloading) = &self.downloading else {
            return;
        };

        // Taken out of the channel first and acted on after: the events say things about the screen,
        // which cannot be changed while the channel it is holding is borrowed.
        let mut events = Vec::new();
        let mut ended = false;

        loop {
            match downloading.try_recv() {
                Ok(event) => {
                    ended |= event == Event::Done;
                    events.push(event);

                    if ended {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    ended = true;
                    break;
                }
            }
        }

        for event in events {
            match event {
                Event::Started { title, index, total } => {
                    self.progress = Some((title.clone(), 0.0));
                    self.report(format!("downloading {index}/{total}: {title}"));
                }
                Event::Progress { percent } => {
                    if let Some((_, done)) = self.progress.as_mut() {
                        *done = percent;
                    }
                }
                Event::Finished { title, path } => {
                    self.progress = None;

                    match path.parent() {
                        Some(folder) => {
                            self.report(format!("saved {title} to {}", folder.display()))
                        }
                        None => self.report(format!("saved {title}")),
                    }
                }
                Event::Failed { title, error } => {
                    self.progress = None;
                    self.fail(format!("{title}: {error}"));
                }
                Event::Done => {}
            }
        }

        if ended {
            self.downloading = None;
            self.progress = None;
        }
    }

    /// Handle a key press, playing or queueing through `player` as asked.
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        player: &mut dyn Controls,
        config: &Config,
    ) -> Option<YoutubeOutcome> {
        if key.kind != KeyEventKind::Press {
            return None;
        }

        if let Mode::Typing(_) = &self.mode {
            return self.handle_typing_key(key, config);
        }

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous(),
            KeyCode::Home | KeyCode::Char('g') => self.state.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.state.select_last(),
            KeyCode::PageDown => self.state.scroll_down_by(10),
            KeyCode::PageUp => self.state.scroll_up_by(10),

            // A new query, which is what this screen is mostly for.
            KeyCode::Char('/') | KeyCode::Char('s') => {
                self.mode = Mode::Typing(self.query.clone());
            }

            // Ticking results, so one action can take several.
            KeyCode::Char(' ') => self.toggle_mark(),
            // All or nothing: pressing it again clears, which is the only other thing it could mean.
            KeyCode::Char('m') => self.mark_all(),

            // Streaming: play one now, or put the chosen ones in the queue.
            KeyCode::Enter => self.play_now(player),
            KeyCode::Char('a') => self.queue(player),

            // Keeping: write the chosen ones to disk.
            KeyCode::Char('d') => self.download(config),

            KeyCode::Esc | KeyCode::Char('q') => return Some(YoutubeOutcome::Close),

            _ => {}
        }

        None
    }

    /// Keys while the query is being typed.
    fn handle_typing_key(&mut self, key: KeyEvent, config: &Config) -> Option<YoutubeOutcome> {
        let Mode::Typing(query) = &mut self.mode else {
            return None;
        };

        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => query.clear(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => query.push(c),
            KeyCode::Backspace => {
                query.pop();
            }
            KeyCode::Enter => {
                let query = query.clone();
                self.start_search(query, config);
            }
            // Out of the query, and out of the screen when there is nothing to go back to.
            KeyCode::Esc => {
                if self.results.is_empty() {
                    return Some(YoutubeOutcome::Close);
                }

                self.mode = Mode::Browsing;
            }
            _ => {}
        }

        None
    }

    /// Ask YouTube for `query`, on a thread of its own.
    fn start_search(&mut self, query: String, config: &Config) {
        let query = query.trim().to_string();

        if query.is_empty() {
            self.fail("nothing to search for");
            return;
        }

        if let Some(missing) = ytdl::missing() {
            self.fail(missing);
            return;
        }

        self.query = query.clone();
        self.mode = Mode::Browsing;
        self.report(format!("searching for {query}…"));
        self.searching = Some(ytdl::search_in_background(query, config.search_results()));
    }

    fn toggle_mark(&mut self) {
        let Some(index) = self.state.selected() else {
            return;
        };

        if let Some(marked) = self.marked.get_mut(index) {
            *marked = !*marked;
        }
    }

    /// Tick everything, or clear the ticks when everything is already ticked.
    fn mark_all(&mut self) {
        let all_marked = !self.marked.is_empty() && self.marked.iter().all(|marked| *marked);

        for marked in &mut self.marked {
            *marked = !all_marked;
        }
    }

    /// Start the highlighted result now, streaming it.
    fn play_now(&mut self, player: &mut dyn Controls) {
        let Some(track) = self.highlighted().cloned() else {
            self.fail("nothing to play");
            return;
        };

        if let Some(missing) = ytdl::missing() {
            self.fail(missing);
            return;
        }

        player.play_now(track.song());
        self.report(format!("streaming {}", track.title));
    }

    /// Put the chosen results in the queue, to be streamed when they come round.
    fn queue(&mut self, player: &mut dyn Controls) {
        let chosen = self.chosen();

        if chosen.is_empty() {
            self.fail("nothing to queue");
            return;
        }

        if let Some(missing) = ytdl::missing() {
            self.fail(missing);
            return;
        }

        let count = chosen.len();
        let first = chosen[0].title.clone();

        player.add_queue_all(chosen.iter().map(Track::song).collect());
        self.clear_marks();

        match count {
            1 => self.report(format!("queued {first}")),
            count => self.report(format!("queued {count}")),
        }
    }

    /// Write the chosen results into the download folder.
    fn download(&mut self, config: &Config) {
        if self.downloading.is_some() {
            self.fail("a download is already running");
            return;
        }

        let chosen = self.chosen();

        if chosen.is_empty() {
            self.fail("nothing to download");
            return;
        }

        if let Some(missing) = ytdl::missing() {
            self.fail(missing);
            return;
        }

        // The folder has to exist before yt-dlp is pointed at it, and saying why it could not be
        // made is more use than a download that fails a second later.
        if let Err(err) = std::fs::create_dir_all(&self.folder) {
            self.fail(format!("cannot use {}: {err}", self.folder.display()));
            return;
        }

        let count = chosen.len();
        self.downloading = Some(ytdl::download_in_background(
            chosen,
            self.folder.clone(),
            config.download_format(),
        ));
        self.clear_marks();
        self.report(format!("downloading {count} to {}", self.folder.display()));
    }

    fn clear_marks(&mut self) {
        for marked in &mut self.marked {
            *marked = false;
        }
    }

    fn select_next(&mut self) {
        match self.state.selected() {
            Some(index) if index + 1 >= self.results.len() => self.state.select_first(),
            _ => self.state.select_next(),
        }
    }

    fn select_previous(&mut self) {
        match self.state.selected() {
            Some(0) | None if !self.results.is_empty() => {
                self.state.select(Some(self.results.len() - 1))
            }
            _ => self.state.select_previous(),
        }
    }

    /// Draw the screen into `area`.
    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let mut block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(self.theme.border(true))
            .title(Line::from(" youtube ").centered().style(self.theme.title()))
            .padding(Padding::horizontal(1));

        if self.hints {
            block = block.title_bottom(
                Line::from(" / search · enter stream · a queue · d download · space mark · esc back ")
                    .centered()
                    .style(self.theme.muted()),
            );
        }

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let [query_area, list_area, status_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        self.render_query(frame, query_area);
        self.render_results(frame, list_area);
        self.render_status(frame, status_area);
    }

    fn render_query(&self, frame: &mut Frame<'_>, area: Rect) {
        let line = match &self.mode {
            Mode::Typing(query) => Line::from(vec![
                Span::from("search ").style(self.theme.muted()),
                Span::from(query.as_str()).style(self.theme.text()),
                Span::from("█").style(self.theme.accent()),
            ]),
            Mode::Browsing if self.query.is_empty() => {
                Line::from("press / to search").style(self.theme.muted())
            }
            Mode::Browsing => Line::from(vec![
                Span::from("search ").style(self.theme.muted()),
                Span::from(self.query.as_str())
                    .style(self.theme.accent().add_modifier(Modifier::BOLD)),
            ]),
        };

        frame.render_widget(line, area);
    }

    fn render_results(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if self.results.is_empty() {
            let text = if self.searching.is_some() {
                "searching…"
            } else {
                "no results yet"
            };

            frame.render_widget(Line::from(text).style(self.theme.muted()), area);
            return;
        }

        let items: Vec<ListItem<'_>> = self
            .results
            .iter()
            .enumerate()
            .map(|(index, track)| {
                let marked = self.marked.get(index).copied().unwrap_or(false);

                let uploader = track.uploader.clone().unwrap_or_else(|| "unknown".to_string());

                ListItem::new(Line::from(vec![
                    // A tick in the margin, so what an action will take is visible at a glance.
                    Span::from(if marked { "● " } else { "  " }).style(self.theme.accent()),
                    Span::from(track.title.clone()).style(self.theme.text()),
                    Span::from(format!("  {uploader}")).style(self.theme.muted()),
                    Span::from(format!("  {}", track.display_duration())).style(self.theme.muted()),
                ]))
            })
            .collect();

        let list = List::new(items)
            .highlight_symbol("▶ ")
            .highlight_style(self.theme.highlight());

        frame.render_stateful_widget(list, area, &mut self.state);
    }

    fn render_status(&self, frame: &mut Frame<'_>, area: Rect) {
        // A running download says how far it has got, which is state rather than a report, and is
        // shown whether or not messages are wanted.
        if let Some((title, percent)) = &self.progress {
            frame.render_widget(
                Line::from(format!("{percent:.0}% {title}")).style(self.theme.accent()),
                area,
            );

            return;
        }

        let Some(status) = self.status.as_ref().filter(|_| self.messages) else {
            return;
        };

        let style = if status.is_error { self.theme.error() } else { self.theme.success() };

        frame.render_widget(Line::from(status.text.as_str()).style(style), area);
    }
}

/// Where downloads go when neither the config nor the application says otherwise.
///
/// The platform's music folder, under the player's own name so a download is never mixed into a
/// library the user arranged by hand.
fn default_folder() -> PathBuf {
    dirs::audio_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ogma")
}
