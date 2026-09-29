//! The multimodal panel: the listing the queue is filled from, in whichever mode is showing.
//!
//! Sits under the playlists in the left column. Unlike the folder browser that chooses a library
//! root, this one lists the music as well as the folders, so individual tracks can be queued.
//!
//! Four things to list, `m` cycling between the first three, and the border says which:
//!
//! * **files** — the folder on disk, which is where it opens;
//! * **browse** — everything below that folder grouped under one tag at a time, so a library can be
//!   walked by artist or genre rather than by path. See [`crate::browse`];
//! * **youtube** — what a search turned up, to stream or to download. See [`crate::ytdl`];
//! * **playlist** — a playlist's own order, which the playlists pane opens.
//!
//! The rows are the same kind of thing in every mode, which is the point of the panel being one
//! pane rather than the search having a screen of its own: `a` queues, `P` sends to the selected playlist, and
//! enter plays, whether the row came from the disk or from YouTube.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Padding};

use crate::browse::{self, Facet};
use crate::library;
use crate::meta;
use crate::player::Controls;
use crate::playlist::Playlist;
use crate::song::Song;
use crate::theme::Theme;
use crate::ytdl::{self, Event, Format, Track};

/// What one row of the listing is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    /// The folder above this one.
    Parent(PathBuf),
    /// A folder that can be entered, or queued whole.
    Folder(PathBuf),
    /// A track. Holds the song rather than the path so that reading its tags for display happens
    /// once rather than once per frame. A YouTube result is a track like any other, one that plays
    /// from the network.
    File(Song),
    /// Everything filed under one artist, album, genre or year, in browse mode.
    Group { value: String, songs: Vec<Song> },
}

/// What the pane is listing.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    /// A folder on disk.
    Folder,
    /// A playlist's tracks, in the playlist's own order.
    Playlist { name: String, detail: String },
    /// What is below the folder, grouped under one tag: the groups, or one group's tracks.
    Browse { facet: Facet, group: Option<String> },
    /// What a YouTube search turned up.
    Youtube,
}

impl Mode {
    /// The modes `m` moves between, in order.
    ///
    /// A playlist is not among them: it is opened from the playlists pane rather than cycled into,
    /// and `m` leaves it for the folder it came from.
    const CYCLED: [Mode; 3] = [
        Mode::Folder,
        Mode::Browse { facet: Facet::Artist, group: None },
        Mode::Youtube,
    ];

    /// Which of the cycled modes this is, ignoring where it has been walked to within one.
    fn family(&self) -> usize {
        match self {
            Mode::Folder | Mode::Playlist { .. } => 0,
            Mode::Browse { .. } => 1,
            Mode::Youtube => 2,
        }
    }

    /// The mode `m` moves on to.
    fn next(&self) -> Mode {
        match self {
            // Leaving a playlist goes back to the files rather than onwards, since a playlist is
            // something that was opened on top of them.
            Mode::Playlist { .. } => Mode::Folder,
            other => Mode::CYCLED[(other.family() + 1) % Mode::CYCLED.len()].clone(),
        }
    }
}

/// A file listing, and where the user is in it.
#[derive(Debug)]
pub struct FilePane {
    /// The folder being listed, and the one to come back to after looking at anything else.
    cwd: PathBuf,
    mode: Mode,
    entries: Vec<Entry>,
    state: ListState,
    /// What the last action did, shown under the listing.
    status: Option<String>,
    /// Rows matching this are the only ones listed, when there is one.
    filter: Option<String>,
    /// Indices into `entries` that the filter admits, in listing order.
    visible: Vec<usize>,
    /// Whether keys are going into the search rather than into the listing.
    searching: bool,
    /// What has been typed into the YouTube query, while it is being typed.
    typing_query: Option<String>,
    /// The query the results on screen came from.
    query: String,
    /// What the last YouTube search turned up, as songs that play from the network.
    results: Vec<Song>,
    /// A YouTube search in flight, if one is.
    asking: Option<Receiver<Result<Vec<Track>, String>>>,
    /// A download in flight, if one is.
    downloading: Option<Receiver<Event>>,
    /// What the running download is on, and how far it has got.
    progress: Option<(String, f32)>,
    /// The tags of everything below [`FilePane::browse_root`], once they have been read.
    indexed: Vec<Song>,
    /// The folder the index was built from, so walking somewhere else rebuilds it.
    browse_root: Option<PathBuf>,
    /// An index being built, if one is.
    indexing: Option<Receiver<Vec<Song>>>,
    /// How many results a YouTube search asks for, from the config.
    search_results: usize,
    /// What a downloaded track is kept as, from the config.
    download_format: Format,
    /// Where downloads are written, from the config.
    download_folder: PathBuf,
    /// Whether what was drawn last should be thrown away rather than drawn over.
    ///
    /// Only the cells that change are written, which a wide character or an emoji — both common in
    /// YouTube titles — can leave half of behind. Leaving a listing full of them asks for a clean
    /// slate instead.
    repaint: bool,
    /// Files and folders picked up with `x`, waiting to be put down somewhere with `M`.
    ///
    /// Held as absolute paths rather than as positions in the listing: the point of holding
    /// something is to walk somewhere else before putting it down, and the listing changes on the
    /// way.
    held: Vec<PathBuf>,
    /// Whether to list the keys under the listing.
    hints: bool,
    /// Whether to report what an action did.
    messages: bool,
    /// Colours to draw with.
    theme: Theme,
}

impl Default for FilePane {
    fn default() -> Self {
        Self::new()
    }
}

impl FilePane {
    /// Open at the current working directory.
    pub fn new() -> Self {
        let start = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));

        Self::at(&start)
    }

    /// Open at `path`.
    pub fn at(path: &Path) -> Self {
        let mut pane = FilePane {
            cwd: PathBuf::new(),
            mode: Mode::Folder,
            entries: Vec::new(),
            state: ListState::default(),
            filter: None,
            visible: Vec::new(),
            searching: false,
            typing_query: None,
            query: String::new(),
            results: Vec::new(),
            asking: None,
            downloading: None,
            progress: None,
            indexed: Vec::new(),
            browse_root: None,
            indexing: None,
            search_results: ytdl::DEFAULT_RESULTS,
            download_format: Format::default(),
            download_folder: default_download_folder(),
            repaint: false,
            held: Vec::new(),
            status: None,
            hints: true,
            messages: true,
            theme: Theme::default(),
        };

        pane.open(path);
        pane
    }

    /// Draw with `theme` rather than whatever was set before.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Show or hide the key reminders under the listing.
    pub fn set_hints(&mut self, hints: bool) {
        self.hints = hints;
    }

    /// Show or hide the reports of what an action did.
    pub fn set_messages(&mut self, messages: bool) {
        self.messages = messages;
    }

    /// How many results a YouTube search should ask for.
    pub fn set_search_results(&mut self, count: usize) {
        self.search_results = count;
    }

    /// What a downloaded track should be kept as.
    pub fn set_download_format(&mut self, format: Format) {
        self.download_format = format;
    }

    /// Where downloads should be written.
    pub fn set_download_folder(&mut self, folder: PathBuf) {
        self.download_folder = folder;
    }

    /// Where downloads are written.
    pub fn download_folder(&self) -> &Path {
        &self.download_folder
    }

    /// Take in whatever the background work has finished with.
    ///
    /// Reading a folder's tags, asking YouTube and fetching a track all happen on threads of their
    /// own; this is where what they did arrives. Called on every tick, and never blocks.
    pub fn poll(&mut self) {
        self.collect_index();
        self.collect_search();
        self.collect_download();
    }

    /// Whether what was drawn last should be cleared rather than drawn over, taking the flag.
    ///
    /// Set when the listing changes to something of a different kind. A wide character or an emoji
    /// takes two cells, and a terminal told to overwrite half of one can leave the other half
    /// showing; YouTube titles are full of them, so the screen it was listed on is thrown away
    /// rather than written over.
    pub fn take_repaint(&mut self) -> bool {
        std::mem::take(&mut self.repaint)
    }

    /// The folder being listed, or the folder a playlist view will return to.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// What the pane is listing, in a word: `files`, `browse`, `youtube` or `playlist`.
    pub fn mode_name(&self) -> &'static str {
        match self.mode {
            Mode::Folder => "files",
            Mode::Playlist { .. } => "playlist",
            Mode::Browse { .. } => "browse",
            Mode::Youtube => "youtube",
        }
    }

    /// What the pane is called while it is in this mode.
    ///
    /// The border says which of the four is showing, so a pane full of artist names is not read as
    /// a folder full of them. What is being listed within the mode — the folder, the group, the
    /// query — is the line under the border, not this.
    fn title(&self) -> &'static str {
        match self.mode {
            Mode::Folder => " Files ",
            Mode::Playlist { .. } => " Playlist ",
            Mode::Browse { .. } => " Browse ",
            Mode::Youtube => " YouTube ",
        }
    }

    /// What the browse mode is grouping tracks under, when that is what is showing.
    pub fn facet(&self) -> Option<Facet> {
        match self.mode {
            Mode::Browse { facet, .. } => Some(facet),
            _ => None,
        }
    }

    /// What a YouTube search turned up, as songs that play from the network.
    pub fn results(&self) -> &[Song] {
        &self.results
    }

    /// The playlist being listed, if a playlist is open rather than a folder.
    pub fn open_playlist(&self) -> Option<&str> {
        match &self.mode {
            Mode::Playlist { name, .. } => Some(name),
            _ => None,
        }
    }

    /// List `playlist`'s tracks instead of a folder, in the playlist's own order.
    ///
    /// The folder stays remembered, so backspace comes back to it.
    pub fn show_playlist(&mut self, playlist: &Playlist) {
        self.entries = playlist
            .ordered()
            .into_iter()
            .cloned()
            .map(Entry::File)
            .collect();

        let detail = format!(
            "{} · {}{}",
            playlist.len(),
            playlist.sort().label(),
            if playlist.descending() { " ↓" } else { "" }
        );

        self.mode = Mode::Playlist { name: playlist.name().to_string(), detail };
        self.filter = None;
        self.searching = false;
        self.typing_query = None;
        self.repaint = true;
        self.state.select(Some(0));
        self.apply_filter();
        self.status = if playlist.is_empty() {
            Some("playlist is empty".to_string())
        } else {
            None
        };
    }

    /// Go back to listing the folder.
    pub fn show_folder(&mut self) {
        let cwd = self.cwd.clone();

        self.mode = Mode::Folder;
        self.open(&cwd);
    }

    fn highlighted(&self) -> Option<&Entry> {
        self.entries.get(*self.visible.get(self.state.selected()?)?)
    }

    /// Whether keys are being typed — into the filter, or into a YouTube query — in which case the
    /// screen must not treat them as commands.
    pub fn is_typing(&self) -> bool {
        self.searching || self.typing_query.is_some()
    }

    /// What the search is narrowing the listing to, if anything.
    pub fn filter(&self) -> Option<&str> {
        self.filter.as_deref()
    }

    /// How a row reads, which is what the search matches against: searching for what is on screen is
    /// the only thing that behaves predictably.
    fn label_of(&self, entry: &Entry) -> String {
        match entry {
            Entry::Parent(_) => "..".to_string(),
            Entry::Folder(path) => name_of(path),
            Entry::File(song) => match (song.artist().or_else(|| song.album_artist()), song.title())
            {
                (Some(artist), Some(title)) => format!("{artist} — {title}"),
                (Some(artist), None) => format!("{artist} — {}", song.display_title()),
                (None, Some(title)) => title.to_string(),
                (None, None) => name_of(song.path()),
            },
            Entry::Group { value, songs } => format!("{value} ({})", songs.len()),
        }
    }

    /// Work out which rows the filter admits, keeping the highlight on something sensible.
    fn apply_filter(&mut self) {
        let matches: Vec<usize> = match &self.filter {
            None => (0..self.entries.len()).collect(),
            Some(query) => {
                let needle = query.to_lowercase();

                (0..self.entries.len())
                    .filter(|index| {
                        // The way out always stays reachable: a search should not trap anyone in a
                        // folder.
                        matches!(self.entries[*index], Entry::Parent(_))
                            || self.label_of(&self.entries[*index]).to_lowercase().contains(&needle)
                    })
                    .collect()
            }
        };

        self.visible = matches;
        self.state.select((!self.visible.is_empty()).then(|| {
            self.state.selected().unwrap_or(0).min(self.visible.len() - 1)
        }));
    }

    /// List `path`, keeping the previous listing if it cannot be read.
    fn open(&mut self, path: &Path) {
        let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        let Ok(read) = std::fs::read_dir(&target) else {
            self.status = Some(format!("cannot open {}", name_of(&target)));
            return;
        };

        let mut folders = Vec::new();
        let mut files = Vec::new();

        for entry in read.flatten() {
            let path = entry.path();

            let hidden = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'));

            if hidden {
                continue;
            }

            if path.is_dir() {
                folders.push(path);
            } else if meta::has_audio_extension(&path) {
                files.push(path);
            }
        }

        folders.sort();
        files.sort();

        let files: Vec<Song> = files.into_iter().map(Song::new).collect();

        // Folders first, then tracks: the shape a music folder is usually read in.
        let mut entries = Vec::with_capacity(folders.len() + files.len() + 1);
        if let Some(parent) = target.parent() {
            entries.push(Entry::Parent(parent.to_path_buf()));
        }
        entries.extend(folders.into_iter().map(Entry::Folder));
        entries.extend(files.into_iter().map(Entry::File));

        self.cwd = target;
        self.mode = Mode::Folder;
        self.entries = entries;
        self.status = None;
        // A new folder is a fresh listing: carrying a search into it would hide most of it without
        // saying so.
        self.filter = None;
        self.searching = false;
        self.state.select(Some(0));
        self.apply_filter();
    }

    /// Handle a key press, adding to `player` as asked.
    ///
    /// Returns `true` when the key was for this pane, so the screen knows not to treat it as a
    /// transport command.
    pub fn handle_key(&mut self, key: KeyEvent, player: &mut dyn Controls) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }

        // Tab is never the pane's: it moves between panes, search or no search.
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            return false;
        }

        if self.searching {
            return self.handle_search_key(key);
        }

        if self.typing_query.is_some() {
            return self.handle_query_key(key);
        }

        match key.code {
            // Ask YouTube something. The listing here is already the answer to a question, so `/`
            // asks a new one rather than narrowing the last.
            KeyCode::Char('/') if self.mode == Mode::Youtube => {
                self.typing_query = Some(self.query.clone());
            }

            // Narrow the listing to what is typed.
            KeyCode::Char('/') => {
                self.searching = true;
                self.filter = Some(String::new());
                self.apply_filter();
            }

            // A search is undone before the screen is left, so escape does the nearer thing first.
            KeyCode::Esc if self.filter.is_some() => self.clear_filter(),

            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous(),
            KeyCode::Home | KeyCode::Char('g') => self.state.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.state.select_last(),
            KeyCode::PageDown => self.state.scroll_down_by(10),
            KeyCode::PageUp => self.state.scroll_up_by(10),

            // Entering a folder or a group, or starting a track straight away.
            KeyCode::Enter => match self.highlighted().cloned() {
                Some(Entry::Parent(path) | Entry::Folder(path)) => self.open(&path),
                Some(Entry::Group { value, .. }) => self.open_group(&value),
                Some(Entry::File(song)) => {
                    let title = song.display_title();

                    player.play_now(song);
                    self.status = Some(format!("playing {title}"));
                }
                None => {}
            },

            // Back out of whatever the listing is showing, a step at a time: out of a group to
            // the groups, out of anything else to the folder, and up out of the folder itself.
            KeyCode::Backspace => match self.mode.clone() {
                Mode::Playlist { .. } | Mode::Youtube => self.show_folder(),
                Mode::Browse { facet, group: Some(_) } => {
                    self.mode = Mode::Browse { facet, group: None };
                    self.rebuild();
                }
                Mode::Browse { .. } => self.show_folder(),
                Mode::Folder => {
                    if let Some(parent) = self.cwd.parent().map(Path::to_path_buf) {
                        self.open(&parent);
                    }
                }
            },

            // Round the modes: files, browse, youtube.
            KeyCode::Char('m') => self.cycle_mode(),
            // What the browse mode files tracks under.
            KeyCode::Char('t') if matches!(self.mode, Mode::Browse { .. }) => self.cycle_facet(),
            // Keep the chosen results rather than streaming them.
            KeyCode::Char('d') if self.mode == Mode::Youtube => self.download_chosen(),

            // Pick something up to move it, or put it down again.
            KeyCode::Char('x') => self.hold_highlighted(),
            // Put down everything held, here. A capital, like the other keys that act on more than
            // the highlighted row; `p` could not have it, being the player's previous track.
            KeyCode::Char('M') => self.move_held_here(),

            // Queue what is highlighted: a track, or a whole folder.
            KeyCode::Char('a') => self.queue_highlighted(player),
            // Queue the lot: every track below this folder, or the whole playlist.
            KeyCode::Char('A') => match &self.mode {
                // A narrowed listing means the matches, not everything: queuing what is not on
                // screen would be a surprise.
                _ if self.filter.is_some() => {
                    let songs: Vec<Song> = self.songs_listed();

                    self.status = Some(format!("queued {} matching", songs.len()));
                    player.add_queue_all(songs);
                }
                Mode::Playlist { name, .. } => {
                    let name = name.clone();
                    let songs: Vec<Song> = self.songs_listed();

                    self.status = Some(format!("queued {} from {name}", songs.len()));
                    player.add_queue_all(songs);
                }
                Mode::Folder => {
                    let cwd = self.cwd.clone();
                    self.queue_path(player, &cwd);
                }
                // A grouped listing, or a page of results: what is on screen is what there is.
                Mode::Browse { .. } | Mode::Youtube => {
                    let songs: Vec<Song> = self.songs_listed();

                    self.status = Some(format!("queued {}", songs.len()));
                    player.add_queue_all(songs);
                }
            },

            _ => return false,
        }

        true
    }

    /// Keys while typing a search. Everything but the few that end it goes into the query.
    fn handle_search_key(&mut self, key: KeyEvent) -> bool {
        let Some(query) = &mut self.filter else {
            self.searching = false;
            return false;
        };

        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => query.clear(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => query.push(c),
            KeyCode::Backspace => {
                query.pop();
            }
            // Keep what was typed and go back to the listing, which is now narrowed.
            KeyCode::Enter | KeyCode::Down | KeyCode::Up => {
                self.searching = false;

                if self.filter.as_ref().is_some_and(|query| query.is_empty()) {
                    self.filter = None;
                }

                self.apply_filter();
                return true;
            }
            // Abandon the search and put the whole listing back.
            KeyCode::Esc => {
                self.clear_filter();
                return true;
            }
            _ => return true,
        }

        self.apply_filter();
        true
    }

    /// Whether the listing has a way out in it, which is not a search result.
    fn has_parent(&self) -> bool {
        self.entries.first().is_some_and(|entry| matches!(entry, Entry::Parent(_)))
    }

    /// Put the whole listing back.
    fn clear_filter(&mut self) {
        self.filter = None;
        self.searching = false;
        self.apply_filter();
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

    /// The tracks the highlighted row stands for: one for a track, all of them for a folder.
    ///
    /// Used by "add to playlist", which the screen routes to the playlists pane.
    pub fn highlighted_songs(&self) -> Vec<Song> {
        match self.highlighted() {
            Some(Entry::File(song)) => vec![song.clone()],
            Some(Entry::Folder(path)) => library::scan(path),
            // A group stands for its tracks, so queueing or sending one to a playlist takes the
            // lot — the same as a folder does.
            Some(Entry::Group { songs, .. }) => songs.clone(),
            Some(Entry::Parent(_)) | None => Vec::new(),
        }
    }

    /// Every track in the listing as it stands, filter and all, in the order it is listed.
    fn songs_listed(&self) -> Vec<Song> {
        self.visible
            .iter()
            .filter_map(|index| match &self.entries[*index] {
                Entry::File(song) => Some(song.clone()),
                _ => None,
            })
            .collect()
    }

    /// Note what happened, for the screen to report on this pane's behalf.
    pub fn report(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
    }

    /// What is waiting to be moved.
    pub fn held(&self) -> &[PathBuf] {
        &self.held
    }

    /// Pick the highlighted row up, or put it down if it is already held.
    ///
    /// Holding changes nothing on disk: it only says what a later `p` will move. Several things can
    /// be held at once, from as many folders as you like.
    fn hold_highlighted(&mut self) {
        if let Mode::Playlist { .. } = self.mode {
            // A playlist is an order, not a folder: its rows are files that live elsewhere, and
            // moving one from here would say nothing about where it is being moved from.
            self.status = Some("open a folder to move files".to_string());
            return;
        }

        let path = match self.highlighted() {
            // A track that plays from the network is not a file, and a group is not a folder:
            // neither is something the filesystem can be asked to move.
            Some(Entry::File(song)) if song.is_stream() => {
                self.status = Some("a stream is not a file".to_string());
                return;
            }
            Some(Entry::Group { .. }) => {
                self.status = Some("open the group to move its tracks".to_string());
                return;
            }
            Some(Entry::File(song)) => song.path().to_path_buf(),
            Some(Entry::Folder(path)) => path.clone(),
            // `..` is the way out of the folder, not a thing in it.
            Some(Entry::Parent(_)) | None => {
                self.status = Some("nothing to move".to_string());
                return;
            }
        };

        if let Some(at) = self.held.iter().position(|held| *held == path) {
            self.held.remove(at);
            self.status = Some(format!("put down {}", name_of(&path)));

            return;
        }

        self.held.push(path.clone());

        self.status = Some(match self.held.len() {
            1 => format!("holding {}", name_of(&path)),
            count => format!("holding {count}"),
        });
    }

    /// Move everything held into the folder being listed.
    ///
    /// Whatever cannot be moved stays held, so a second attempt somewhere else is one key away, and
    /// the message says how many did not make it.
    fn move_held_here(&mut self) {
        if self.held.is_empty() {
            self.status = Some("nothing held — x picks one up".to_string());
            return;
        }

        if let Mode::Playlist { .. } = self.mode {
            self.status = Some("open a folder to move into".to_string());
            return;
        }

        let folder = self.cwd.clone();
        let mut moved = 0;
        let mut failed = Vec::new();
        let mut last_error = None;

        for path in std::mem::take(&mut self.held) {
            match move_into(&path, &folder) {
                Ok(()) => moved += 1,
                Err(err) => {
                    last_error = Some(err);
                    failed.push(path);
                }
            }
        }

        self.held = failed;

        // The listing is what the folder held a moment ago; the files in it have just changed.
        let selected = self.state.selected();
        self.open(&folder);
        self.state.select(selected.filter(|index| *index < self.visible.len()).or(Some(0)));

        self.status = Some(match (moved, last_error) {
            (0, Some(err)) => err,
            (0, None) => "nothing moved".to_string(),
            (moved, Some(err)) => format!("moved {moved}, {} left: {err}", self.held.len()),
            (1, None) => "moved 1 here".to_string(),
            (moved, None) => format!("moved {moved} here"),
        });
    }

    // ------------------------------------------------------------------------------ the modes

    /// Move to the next mode: files, browse, youtube, round again.
    fn cycle_mode(&mut self) {
        let next = self.mode.next();
        self.enter_mode(next);
    }

    /// Show `mode`, doing whatever it needs before it can be listed.
    fn enter_mode(&mut self, mode: Mode) {
        // A listing of one kind of thing has no business being narrowed by a search for another.
        self.filter = None;
        self.searching = false;
        self.typing_query = None;
        // Leaving a listing of YouTube titles is where the half-drawn wide characters come from.
        self.repaint = true;

        match mode {
            Mode::Folder => {
                let cwd = self.cwd.clone();
                self.mode = Mode::Folder;
                self.open(&cwd);
            }
            Mode::Browse { facet, .. } => {
                self.mode = Mode::Browse { facet, group: None };
                self.start_index();
                self.rebuild();
            }
            Mode::Youtube => {
                self.mode = Mode::Youtube;

                // The keys stay the pane's until `/` is pressed: arriving in a mode should not
                // mean every key that follows is swallowed by a text field nobody asked for.
                self.rebuild();

                self.status = match ytdl::missing() {
                    Some(missing) => Some(missing),
                    None if self.results.is_empty() => Some("press / to search".to_string()),
                    None => None,
                };
            }
            Mode::Playlist { .. } => self.mode = mode,
        }
    }

    /// Group the tracks under the next tag along.
    fn cycle_facet(&mut self) {
        let Mode::Browse { facet, .. } = &self.mode else {
            return;
        };

        let facet = facet.next();

        // Back to the list of groups: the group that was open belongs to the old grouping.
        self.mode = Mode::Browse { facet, group: None };
        self.status = Some(format!("by {}", facet.label()));
        self.rebuild();
    }

    /// Show the tracks filed under `value`.
    fn open_group(&mut self, value: &str) {
        let Mode::Browse { facet, .. } = &self.mode else {
            return;
        };

        self.mode = Mode::Browse { facet: *facet, group: Some(value.to_string()) };
        self.filter = None;
        self.rebuild();
    }

    /// Build the listing the current mode calls for.
    ///
    /// The folder builds its own listing as it opens it, and a playlist is handed one; this is for
    /// the two modes whose rows come from something the pane is holding.
    fn rebuild(&mut self) {
        match self.mode.clone() {
            Mode::Browse { facet, group } => {
                let grouped = browse::group(&self.indexed, facet);

                self.entries = match group {
                    Some(wanted) => grouped
                        .into_iter()
                        .find(|group| group.value == wanted)
                        .map(|group| group.songs.into_iter().map(Entry::File).collect())
                        .unwrap_or_default(),
                    None => grouped
                        .into_iter()
                        .map(|group| Entry::Group { value: group.value, songs: group.songs })
                        .collect(),
                };
            }
            Mode::Youtube => {
                self.entries = self.results.iter().cloned().map(Entry::File).collect();
            }
            Mode::Folder | Mode::Playlist { .. } => return,
        }

        self.state.select((!self.entries.is_empty()).then_some(0));
        self.apply_filter();
    }

    // ---------------------------------------------------------------------------- browsing tags

    /// Start reading the tags of everything below the folder, unless that is already done.
    ///
    /// Reading a library's tags takes long enough to be worth saying so, and long enough that it
    /// cannot happen on the drawing thread. What comes back is kept until the folder changes.
    fn start_index(&mut self) {
        if self.browse_root.as_deref() == Some(self.cwd.as_path()) || self.indexing.is_some() {
            return;
        }

        self.browse_root = Some(self.cwd.clone());
        self.indexed = Vec::new();
        self.indexing = Some(browse::index_in_background(self.cwd.clone()));
        self.status = Some("reading tags…".to_string());
    }

    /// Take in an index that has finished being built.
    fn collect_index(&mut self) {
        let Some(indexing) = &self.indexing else {
            return;
        };

        match indexing.try_recv() {
            Ok(songs) => {
                self.indexing = None;
                let count = songs.len();
                self.indexed = songs;
                self.rebuild();

                self.status = Some(match count {
                    0 => "no music below this folder".to_string(),
                    1 => "1 track".to_string(),
                    count => format!("{count} tracks"),
                });
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.indexing = None;
                self.status = Some("could not read the folder".to_string());
            }
        }
    }

    // --------------------------------------------------------------------------------- youtube

    /// Keys while the query is being typed.
    fn handle_query_key(&mut self, key: KeyEvent) -> bool {
        let Some(query) = &mut self.typing_query else {
            return false;
        };

        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => query.clear(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => query.push(c),
            KeyCode::Backspace => {
                query.pop();
            }
            KeyCode::Enter => {
                let query = query.clone();
                self.start_search(query);
            }
            // Out of the query, leaving whatever was found last on screen.
            KeyCode::Esc => self.typing_query = None,
            _ => {}
        }

        true
    }

    /// Ask YouTube for `query`, on a thread of its own.
    fn start_search(&mut self, query: String) {
        let query = query.trim().to_string();
        self.typing_query = None;

        if query.is_empty() {
            self.status = Some("nothing to search for".to_string());
            return;
        }

        if let Some(missing) = ytdl::missing() {
            self.status = Some(missing);
            return;
        }

        self.query = query.clone();
        self.status = Some(format!("searching for {query}…"));
        self.asking = Some(ytdl::search_in_background(query, self.search_results));
    }

    /// Take in what a search turned up.
    fn collect_search(&mut self) {
        let Some(asking) = &self.asking else {
            return;
        };

        match asking.try_recv() {
            Ok(Ok(results)) => {
                self.asking = None;
                let count = results.len();

                self.results = results.iter().map(Track::song).collect();
                self.rebuild();

                self.status = Some(match count {
                    0 => "nothing found".to_string(),
                    1 => "1 result".to_string(),
                    count => format!("{count} results"),
                });
            }
            Ok(Err(err)) => {
                self.asking = None;
                self.status = Some(err);
            }
            Err(TryRecvError::Empty) => {}
            // The thread went away without answering, which is the spawn having failed.
            Err(TryRecvError::Disconnected) => {
                self.asking = None;
                self.status = Some("the search could not be started".to_string());
            }
        }
    }

    /// Download what is highlighted, rather than streaming it.
    fn download_chosen(&mut self) {
        if self.downloading.is_some() {
            self.status = Some("a download is already running".to_string());
            return;
        }

        // Whatever the listing would queue is what it downloads, so the two keys agree about what
        // they are acting on.
        let songs = self.highlighted_songs();
        let tracks: Vec<Track> = songs.iter().filter_map(Track::of).collect();

        if tracks.is_empty() {
            self.status = Some("nothing to download".to_string());
            return;
        }

        if let Some(missing) = ytdl::missing() {
            self.status = Some(missing);
            return;
        }

        // The folder has to exist before yt-dlp is pointed at it, and saying why it could not be
        // made is more use than a download that fails a second later.
        if let Err(err) = std::fs::create_dir_all(&self.download_folder) {
            self.status = Some(format!("cannot use {}: {err}", self.download_folder.display()));
            return;
        }

        let count = tracks.len();
        self.downloading = Some(ytdl::download_in_background(
            tracks,
            self.download_folder.clone(),
            self.download_format,
        ));
        self.status = Some(format!("downloading {count} to {}", self.download_folder.display()));
    }

    /// Take in whatever a running download has to say.
    fn collect_download(&mut self) {
        let Some(downloading) = &self.downloading else {
            return;
        };

        // Taken out of the channel first and acted on after: the events say things about the pane,
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
                    self.status = Some(format!("{index}/{total} {title}"));
                }
                Event::Progress { percent } => {
                    if let Some((_, done)) = self.progress.as_mut() {
                        *done = percent;
                    }
                }
                Event::Finished { title, .. } => {
                    self.progress = None;
                    self.status = Some(format!("saved {title}"));
                }
                Event::Failed { title, error } => {
                    self.progress = None;
                    self.status = Some(format!("{title}: {error}"));
                }
                Event::Done => {}
            }
        }

        if ended {
            self.downloading = None;
            self.progress = None;
        }
    }

    /// Add whatever is highlighted to the queue.
    fn queue_highlighted(&mut self, player: &mut dyn Controls) {
        match self.highlighted().cloned() {
            Some(Entry::File(song)) => {
                let title = song.display_title();

                player.add_queue(song);
                self.status = Some(format!("queued {title}"));
            }
            Some(Entry::Group { value, songs }) => {
                let count = songs.len();

                player.add_queue_all(songs);
                self.status = Some(format!("queued {count} from {value}"));
            }
            Some(Entry::Folder(path)) => self.queue_path(player, &path),
            // `..` is a way out of the folder, not a thing to queue: queuing it would sweep in
            // everything above here, which is never what pressing `a` on it looks like.
            Some(Entry::Parent(_)) => {
                self.status = Some("use A to queue this folder".to_string());
            }
            None => {}
        }
    }

    /// Add every track at or below `path` to the queue.
    fn queue_path(&mut self, player: &mut dyn Controls, path: &Path) {
        let songs = library::scan(path);

        self.status = Some(match songs.len() {
            0 => format!("no tracks in {}", name_of(path)),
            1 => format!("queued 1 track from {}", name_of(path)),
            count => format!("queued {count} tracks from {}", name_of(path)),
        });

        player.add_queue_all(songs);
    }

    /// The key reminders for what is showing, one line each.
    ///
    /// The player screen gives this pane 33 cells, which leaves 29 inside the border and the
    /// padding, so every line here is written to fit that: a reminder that runs off the edge is
    /// worse than no reminder, since the key it names is the part that goes. The number of lines
    /// varies with the mode, and the listing gives up a row for them.
    fn hint_lines(&self) -> &'static [&'static str] {
        if self.searching {
            return &["enter keep · esc cancel", "tab still changes pane"];
        }

        if self.typing_query.is_some() {
            return &["enter search · esc cancel", "tab still changes pane"];
        }

        match self.mode {
            Mode::Folder => &[
                "a queue · A all · P playlist",
                "enter open · bksp up · / find",
                "x hold · M move · m mode",
            ],
            Mode::Playlist { .. } => &[
                "a queue · A all · P playlist",
                "/ find · bksp back · m mode",
            ],
            Mode::Browse { .. } => &[
                "a queue · A all · P playlist",
                "enter open · t by · bksp back",
                "/ find · m mode",
            ],
            Mode::Youtube => &[
                "enter play · a queue · A all",
                "P playlist · d download",
                "/ search · m mode",
            ],
        }
    }

    /// Draw the pane in `area`. `focused` brightens the border and the selection.
    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(Line::from(self.title()).centered().style(self.theme.title()))
            .border_style(self.theme.border(focused))
            .padding(Padding::horizontal(1));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.height < 2 {
            return;
        }

        // The folder being listed, the listing, then a row for what the last action did and two for
        // the keys. The message gets a row of its own rather than taking one of theirs: a reminder
        // that disappears the moment you use the pane is no reminder at all. The row is kept even
        // when there is no message, so the listing does not jump about as messages come and go.
        let keys = self.hint_lines();
        let keys_rows = if self.hints && focused { keys.len() as u16 } else { 0 };

        let [path_area, list_area, status_area, keys_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(keys_rows),
        ])
        .areas(inner);

        let width = inner.width as usize;

        let header = match &self.mode {
            Mode::Playlist { name, detail } => Line::from(vec![
                Span::from("♪ ").style(self.theme.accent()),
                Span::from(truncate(name, width.saturating_sub(detail.chars().count() + 3)))
                    .style(self.theme.accent().add_modifier(Modifier::BOLD)),
                Span::from(format!(" {detail}")).style(self.theme.muted()),
            ]),
            Mode::Folder => Line::from(truncate_left(&name_of(&self.cwd), width))
                .style(self.theme.accent()),
            Mode::Browse { facet, group } => {
                let what = match group {
                    Some(value) => format!("{} · {value}", facet.label()),
                    None => format!("{} · {}", facet.label(), name_of(&self.cwd)),
                };

                Line::from(vec![
                    Span::from("☰ ").style(self.theme.accent()),
                    Span::from(truncate(&what, width.saturating_sub(2)))
                        .style(self.theme.accent().add_modifier(Modifier::BOLD)),
                ])
            }
            Mode::Youtube => {
                let what = match &self.typing_query {
                    Some(typed) => typed.clone(),
                    None if self.query.is_empty() => "press / to search".to_string(),
                    None => self.query.clone(),
                };

                let mut spans = vec![
                    Span::from("▷ ").style(self.theme.accent()),
                    Span::from(truncate(&what, width.saturating_sub(3)))
                        .style(self.theme.accent().add_modifier(Modifier::BOLD)),
                ];

                // A block for the cursor, so a query being typed looks like one.
                if self.typing_query.is_some() {
                    spans.push(Span::from("█").style(self.theme.accent()));
                }

                Line::from(spans)
            }
        };

        frame.render_widget(header, path_area);

        let items: Vec<ListItem<'_>> = self
            .visible
            .iter()
            .map(|index| &self.entries[*index])
            .map(|entry| {
                let held = match entry {
                    Entry::File(song) => self.held.iter().any(|path| path == song.path()),
                    Entry::Folder(path) => self.held.contains(path),
                    Entry::Parent(_) | Entry::Group { .. } => false,
                };

                let line = match entry {
                    Entry::Parent(_) => Line::from("..").style(self.theme.muted()),
                    Entry::Folder(path) if held => Line::from(vec![
                        Span::from("✂ ").style(self.theme.accent()),
                        Span::from(truncate(&name_of(path), width.saturating_sub(4)))
                            .style(self.theme.accent()),
                    ]),
                    Entry::Folder(path) => Line::from(vec![
                        Span::from("▸ ").style(self.theme.muted()),
                        Span::from(truncate(&name_of(path), width.saturating_sub(4)))
                            .style(self.theme.text()),
                    ]),
                    // How many tracks are in it is the useful part, so it goes on the row rather
                    // than being something to open the group to find out.
                    Entry::Group { value, songs } => {
                        let count = songs.len().to_string();

                        Line::from(vec![
                            Span::from("▸ ").style(self.theme.muted()),
                            Span::from(truncate(
                                value,
                                width.saturating_sub(count.chars().count() + 5),
                            ))
                            .style(self.theme.text()),
                            Span::from(format!("  {count}")).style(self.theme.muted()),
                        ])
                    }
                    // Music is named by what it is, not by what the file is called: artist and
                    // title where the tags provide them, and the file name only for a track that has
                    // neither, where the name is all there is to go on.
                    Entry::File(song) => {
                        let label = match (song.artist().or_else(|| song.album_artist()), song.title())
                        {
                            (Some(artist), Some(title)) => format!("{artist} — {title}"),
                            (Some(artist), None) => {
                                format!("{artist} — {}", song.display_title())
                            }
                            (None, Some(title)) => title.to_string(),
                            (None, None) => name_of(song.path()),
                        };

                        // A held row says so in the margin, so what a `p` will move is visible
                        // from the folder it is being moved into.
                        let (mark, style) = if held {
                            ("✂ ", self.theme.accent())
                        } else {
                            ("  ", self.theme.muted())
                        };

                        Line::from(vec![
                            Span::from(mark).style(self.theme.accent()),
                            Span::from(truncate(&label, width.saturating_sub(4))).style(style),
                        ])
                    }
                };

                ListItem::new(line)
            })
            .collect();

        let list = List::new(items).highlight_symbol("› ").highlight_style(if focused {
            self.theme.highlight()
        } else {
            self.theme.highlight_unfocused()
        });

        frame.render_stateful_widget(list, list_area, &mut self.state);

        // The message row says what is going on: the query while it is being typed, what it narrowed
        // to once it has been, and otherwise whatever the last action did.
        if self.searching {
            let query = self.filter.clone().unwrap_or_default();

            frame.render_widget(
                Line::from(vec![
                    Span::from("/").style(self.theme.accent()),
                    Span::from(truncate(&query, width.saturating_sub(2))).style(self.theme.text()),
                    Span::from("█").style(self.theme.accent()),
                ]),
                status_area,
            );
        } else if let Some(query) = &self.filter {
            let matches = self.visible.len().saturating_sub(usize::from(self.has_parent()));

            frame.render_widget(
                Line::from(truncate(&format!("/{query} — {matches} found"), width))
                    .style(self.theme.accent()),
                status_area,
            );
        } else if let Some(status) = &self.status.as_ref().filter(|_| self.messages) {
            frame.render_widget(
                Line::from(truncate(status, width)).style(self.theme.success()),
                status_area,
            );
        }

        if !self.hints || !focused {
            return;
        }

        // Key names in words: the glyphs for enter and backspace are missing from many terminal
        // fonts, where they show as empty boxes. `P` sends the highlighted row to the selected
        // playlist — the player screen acts on it, but this is the pane it is pressed in, so this is
        // where it has to be named.
        let lines: Vec<Line<'_>> = keys
            .iter()
            .map(|keys| Line::from(*keys).style(self.theme.muted()))
            .collect();

        frame.render_widget(ratatui::widgets::Paragraph::new(lines), keys_area);
    }
}

/// Where downloads go when the application has not said otherwise.
///
/// The platform's music folder, under the player's own name, so a download is never mixed into a
/// library the user arranged by hand.
fn default_download_folder() -> PathBuf {
    dirs::audio_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ogma")
}

/// Move `source` into `folder`, keeping its name.
///
/// Nothing is ever overwritten: a name already taken in the target folder is refused rather than
/// resolved, since the two files with one name are the user's to sort out, not this pane's.
fn move_into(source: &Path, folder: &Path) -> Result<(), String> {
    let name = source.file_name().ok_or_else(|| format!("{} has no name", source.display()))?;

    if !source.exists() {
        return Err(format!("gone: {}", name_of(source)));
    }

    if source.parent() == Some(folder) {
        return Err(format!("already here: {}", name_of(source)));
    }

    // Moving a folder into itself, or into something inside it, would move the target along with
    // it — the rename either fails obscurely or takes the folder somewhere it cannot be reached.
    if folder == source || folder.starts_with(source) {
        return Err(format!("cannot hold itself: {}", name_of(source)));
    }

    let target = folder.join(name);

    if target.exists() {
        return Err(format!("already there: {}", name_of(source)));
    }

    match std::fs::rename(source, &target) {
        Ok(()) => Ok(()),
        // Another filesystem: a rename cannot span one, so the bytes have to be carried over and
        // the original removed once they are all there.
        Err(err) if err.raw_os_error() == Some(CROSS_DEVICE) => {
            copy_across(source, &target).map_err(|err| format!("{}: {err}", name_of(source)))
        }
        Err(err) => Err(format!("{}: {err}", name_of(source))),
    }
}

/// `EXDEV`: a rename whose two ends are on different filesystems.
const CROSS_DEVICE: i32 = 18;

/// Copy `source` to `target` and remove the original, for a move between filesystems.
///
/// The original goes only once the copy is complete, so an interrupted move leaves the file where
/// it was rather than nowhere at all.
fn copy_across(source: &Path, target: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        copy_tree(source, target)?;

        return std::fs::remove_dir_all(source);
    }

    std::fs::copy(source, target)?;

    std::fs::remove_file(source)
}

/// Copy a folder and everything below it.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;

    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let target = to.join(entry.file_name());

        if entry.file_type()?.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            std::fs::copy(&source, &target)?;
        }
    }

    Ok(())
}

/// The last component of a path, for display.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
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

/// Cut `text` to `width` cells, keeping the end.
fn truncate_left(text: &str, width: usize) -> String {
    let count = text.chars().count();

    if count <= width || width == 0 {
        return text.to_string();
    }

    let tail: String = text.chars().skip(count - width.saturating_sub(1)).collect();

    format!("…{tail}")
}
