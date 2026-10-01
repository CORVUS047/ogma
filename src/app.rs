//! The application: what screen is showing, and the loop that drives it.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event};
use ratatui::{DefaultTerminal, Frame};

use crate::autofill;
use crate::config::{Config, DaemonOnClose};
use crate::ipc::OnLeave;
use crate::daemon;
use crate::library;
use crate::remote::Remote;
use crate::player::Player;
use crate::theme::Theme;
use crate::ui::{
    ConfigMenu, ConfigMenuOutcome, FolderBrowser, FolderBrowserOutcome, MismatchOutcome,
    MismatchPane, PlayerScreen, PlayerScreenOutcome, StartMenu, StartMenuChoice,
};

/// How long the loop waits for a key before catching up with the audio.
///
/// Fast enough that the progress bar moves smoothly, slow enough to leave the machine alone.
const TICK: Duration = Duration::from_millis(100);

/// Which screen the application is showing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScreenKind {
    Start,
    Config,
    Browse,
    Play,
    /// The dead end shown when the daemon speaks a different protocol.
    Mismatch,
}

/// Which screen has the user's attention.
#[derive(Debug)]
enum Screen {
    Start(StartMenu),
    Config(ConfigMenu),
    Browse(FolderBrowser),
    Play(Box<PlayerScreen>),
    Mismatch(MismatchPane),
}

/// The running player.
#[derive(Debug)]
pub struct App {
    screen: Screen,
    config: Config,
    /// The daemon that does the playing, and the mirror of what it is doing.
    remote: Remote,
    /// Whether this interface is what started the daemon, which decides what happens on close.
    started_daemon: bool,
    /// Files that a background metadata fill has finished with, as they come in.
    fills: Option<std::sync::mpsc::Receiver<PathBuf>>,
    /// Colours every screen draws with.
    theme: Theme,
    /// Why the daemon could not be started, if it could not.
    daemon_error: Option<String>,
    /// The protocol versions that did not match, when they did not: ours, and the daemon's.
    ///
    /// Set only after restarting the daemon failed to fix it. While it is set the interface does
    /// nothing but say so, and stops the daemon when it closes.
    protocol_mismatch: Option<(u32, Option<u32>)>,
    /// The player screen, set aside while another screen is showing.
    ///
    /// Leaving the player does not stop it — the audio keeps going — so the screen is kept rather
    /// than rebuilt, and Continue puts the same one back, queue, listing and selections intact.
    suspended: Option<Box<PlayerScreen>>,
    /// The folder the library is built from, once one has been chosen.
    library_root: Option<PathBuf>,
    running: bool,
}

impl Screen {
    fn kind(&self) -> ScreenKind {
        match self {
            Screen::Start(_) => ScreenKind::Start,
            Screen::Config(_) => ScreenKind::Config,
            Screen::Browse(_) => ScreenKind::Browse,
            Screen::Play(_) => ScreenKind::Play,
            Screen::Mismatch(_) => ScreenKind::Mismatch,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        let config = Config::load();

        // The daemon is what plays; this interface only drives it. One that is already running is
        // left as it is, and one started here is remembered, since that decides what happens on close.
        let already_running = daemon::is_running();
        let mut started_daemon = false;
        let mut daemon_error = None;

        if !already_running {
            match daemon::spawn() {
                Ok(()) => started_daemon = true,
                Err(err) => daemon_error = Some(err),
            }
        }

        // Before anything is asked of the daemon, ask whether it speaks the same protocol. A
        // daemon left running from an older build is the usual reason it does not, so one is
        // stopped and started again from this installation before giving up on it.
        let mut protocol_mismatch = None;

        if let daemon::Handshake::Mismatch { ours, theirs } = daemon::handshake() {
            match daemon::restart() {
                Ok(()) => {
                    started_daemon = true;

                    if let daemon::Handshake::Mismatch { ours, theirs } = daemon::handshake() {
                        protocol_mismatch = Some((ours, theirs));
                    }
                }
                // It could not be replaced, so what is there is what there is.
                Err(err) => {
                    protocol_mismatch = Some((ours, theirs));
                    daemon_error = Some(err);
                }
            }
        }

        let mut remote = Remote::new();

        // Being counted among the daemon's interfaces is what keeps it playing when one of several
        // interfaces closes. It is told who started it, so the last one out is the one that closes it.
        // A daemon that speaks another protocol is not attached to: it would be one more command
        // for it to misread, and this interface is about to close anyway.
        if protocol_mismatch.is_none() {
            remote.attach(started_daemon);
            remote.refresh();
        }

        let theme = Theme::load(config.custom_theme());

        let mut menu = StartMenu::new();
        menu.set_hints(config.show_control_hints());
        menu.set_theme(theme);

        // A mismatch is a dead end: the interface says which versions it found and waits to be
        // closed, rather than driving a daemon that may act on something else entirely.
        let screen = match protocol_mismatch {
            Some((ours, theirs)) => {
                let mut pane = MismatchPane::new(ours, theirs);
                pane.set_theme(theme);

                Screen::Mismatch(pane)
            }
            None => Screen::Start(menu),
        };

        let mut app = App {
            screen,
            // A configured folder is the library root until the user picks another.
            library_root: config.default_folder().map(PathBuf::from),
            config,
            remote,
            started_daemon,
            daemon_error,
            protocol_mismatch,
            fills: None,
            theme,
            suspended: None,
            running: true,
        };

        // With a folder already configured there is nothing to ask about: go straight to the player.
        // A folder that has since been moved or removed is not usable, so that falls back to the
        // menu, where it can be pointed somewhere else. None of that applies while the daemon is
        // the wrong version: there is nothing to play it with.
        if app.protocol_mismatch.is_none()
            && let Some(folder) = app.config.default_folder().map(PathBuf::from)
            && folder.is_dir()
        {
            app.open_library(&folder);
        }

        app
    }
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    /// Draw, handle input, follow the daemon, repeat, until something asks to quit.
    pub fn run(mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut showing = self.screen.kind();

        while self.running {
            // Only the cells that changed are written, which a screen of ordinary text survives
            // fine — but a double-width character or an emoji occupies two cells, and a terminal
            // asked to overwrite half of one can leave the other half behind. YouTube titles are
            // full of both, so a screen or a pane that has just been listing them is repainted
            // from scratch rather than written over.
            let changed = self.screen.kind() != showing;

            if changed || self.take_repaint() {
                terminal.clear()?;
                showing = self.screen.kind();
            }

            terminal.draw(|frame| self.draw(frame))?;

            // Waiting with a timeout rather than blocking: the clock has to advance and finished
            // songs have to give way to the next one even when nobody touches the keyboard.
            if event::poll(TICK)?
                && let Event::Key(key) = event::read()?
            {
                self.handle_key(key);
            }

            self.tick();
        }

        // Closing the interface is not closing the player, unless the config says it is.
        self.part_with_daemon();

        Ok(())
    }

    /// Finish with the daemon as the config asks, for a caller driving the interface itself.
    pub fn close(&mut self) {
        self.part_with_daemon();
    }

    /// Do with the daemon whatever the config asks for on close.
    ///
    /// The wish goes to the daemon rather than being acted on here: another interface may still be
    /// driving it, and a daemon somebody else is still listening to is not this interface's to stop.
    fn part_with_daemon(&mut self) {
        // A daemon this interface cannot talk to is no use to anything, and leaving it running
        // would have the next interface find the same wrong version and say the same thing again.
        if self.protocol_mismatch.is_some() {
            self.remote.quit_daemon();
            return;
        }

        let on_leave = match self.config.daemon_on_close() {
            DaemonOnClose::Stop => OnLeave::Stop,
            DaemonOnClose::StopIfWeStartedIt => OnLeave::StopIfSpawned,
            DaemonOnClose::Keep => OnLeave::Keep,
        };

        if self.remote.leave(on_leave).is_some() {
            return;
        }

        // Attaching never worked, so nothing is counting interfaces and the old answer is the best
        // one available: stop it if this interface is what started it.
        let stop = match self.config.daemon_on_close() {
            DaemonOnClose::Stop => true,
            DaemonOnClose::StopIfWeStartedIt => self.started_daemon,
            DaemonOnClose::Keep => false,
        };

        if stop {
            self.remote.quit_daemon();
        }
    }

    /// Whether a screen has asked to be drawn on a clean slate, taking the request.
    fn take_repaint(&mut self) -> bool {
        match &mut self.screen {
            Screen::Play(screen) => screen.take_repaint(),
            _ => false,
        }
    }

    /// One pass of the work the event loop does between key presses.
    ///
    /// Public so the player can be driven without a terminal, which is how the socket is tested.
    pub fn tick_once(&mut self) {
        self.tick();
    }

    /// Follow the daemon: take in what it is playing, and pick up filled-in metadata.
    fn tick(&mut self) {
        // The daemon is the authority on what is playing; this is where that arrives.
        self.remote.refresh();
        self.collect_fills();

        // Reading a folder's tags, asking YouTube and fetching a track all run on threads of their
        // own, and this is where what they did arrives — without it the pane would sit on
        // "searching…" with the answer already waiting.
        if let Screen::Play(screen) = &mut self.screen {
            screen.poll();
        }
    }

    /// Take in whatever the background fill has finished.
    ///
    /// Nothing needs reloading: the mirror builds its songs from the daemon's paths on each refresh,
    /// so a file that has just gained artwork is read afresh on the next tick.
    fn collect_fills(&mut self) {
        let Some(fills) = &self.fills else {
            return;
        };

        loop {
            match fills.try_recv() {
                Ok(_) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }

        self.fills = None;
    }

    /// Draw whichever screen is showing.
    ///
    /// Public so the interface can be drawn without a terminal of its own, which is how the screens
    /// are tested.
    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();

        match &mut self.screen {
            Screen::Start(menu) => menu.render(frame, area),
            Screen::Config(menu) => menu.render(frame, area, &self.config),
            Screen::Browse(browser) => browser.render(frame, area),
            Screen::Play(screen) => screen.render(frame, area, self.remote.player()),
            Screen::Mismatch(pane) => pane.render(frame, area),
        }
    }

    /// Act on one key press.
    ///
    /// Public so that the interface can be driven without a terminal — which is how it is tested.
    /// [`App::run`] follows each call with [`App::sync_audio`]'s work; a caller doing its own driving
    /// should do the same.
    pub fn handle_key(&mut self, key: event::KeyEvent) {
        match &mut self.screen {
            Screen::Start(menu) => {
                if let Some(choice) = menu.handle_key(key) {
                    self.activate(choice);
                }
            }
            Screen::Config(menu) => {
                if let Some(ConfigMenuOutcome::Close) = menu.handle_key(key, &mut self.config) {
                    // The config may now name a different folder to open next time.
                    self.library_root = self.config.default_folder().map(PathBuf::from);

                    // Settings that need no restart are pushed into the screens that already exist,
                    // so the player waiting behind Continue shows what the config now says.
                    self.apply_config();
                    self.screen = self.start_menu();
                }
            }
            Screen::Browse(browser) => match browser.handle_key(key, &mut self.config) {
                // The browser has already written the choice to the config.
                Some(FolderBrowserOutcome::Chosen(path)) => {
                    self.library_root = Some(path.clone());
                    self.open_library(&path);
                }
                Some(FolderBrowserOutcome::Cancel) => self.screen = self.start_menu(),
                None => {}
            },
            Screen::Play(screen) => {
                if let Some(PlayerScreenOutcome::Close) = screen.handle_key(key, &mut self.remote) {
                    self.suspend_player();
                }
            }
            Screen::Mismatch(pane) => {
                if let Some(MismatchOutcome::Quit) = pane.handle_key(key) {
                    self.running = false;
                }
            }
        }
    }

    /// Push the settings that take effect at once into the screens that already exist.
    ///
    /// A screen is dressed from the config when it is built, which is enough for the ones built on
    /// the way out of the config screen. The player screen set aside by Continue is not rebuilt —
    /// that is the point of it — so without this, hints or messages turned on or off would not reach
    /// it or its panes until the library was opened again.
    fn apply_config(&mut self) {
        let hints = self.config.show_control_hints();
        let messages = !self.config.hide_status_messages();

        // Worked out here rather than in the pane: where downloads go depends on the library, and
        // the application is what knows about that.
        let search_results = self.config.search_results();
        let format = self.config.download_format();
        let folder = self.download_folder();

        if let Some(screen) = self.suspended.as_mut() {
            screen.set_hints(hints);
            screen.set_messages(messages);

            if let Some(folder) = folder.clone() {
                screen.set_youtube(search_results, format, folder);
            }
        }

        match &mut self.screen {
            Screen::Play(screen) => {
                screen.set_hints(hints);
                screen.set_messages(messages);

                if let Some(folder) = folder {
                    screen.set_youtube(search_results, format, folder);
                }
            }
            Screen::Start(menu) => menu.set_hints(hints),
            Screen::Browse(browser) => {
                browser.set_hints(hints);
                browser.set_messages(messages);
            }
            Screen::Config(menu) => menu.set_messages(messages),
            // Nothing in the config reaches a screen that exists to be closed.
            Screen::Mismatch(_) => {}
        }
    }

    /// Leave the player for the menu, keeping its screen to come back to.
    fn suspend_player(&mut self) {
        // Swapping a menu in hands back the screen that was showing, which is the one to keep.
        let placeholder = Screen::Start(StartMenu::new());

        if let Screen::Play(screen) = std::mem::replace(&mut self.screen, placeholder) {
            self.suspended = Some(screen);
        }

        // Built after the swap, so the menu knows there is something to continue.
        self.screen = self.start_menu();
    }

    /// Go back to the player screen that was set aside.
    fn resume_player(&mut self) {
        if let Some(screen) = self.suspended.take() {
            self.screen = Screen::Play(screen);
        }
    }

    /// Show the playback screen, browsing `root`.
    ///
    /// Nothing is queued: the queue is the user's to fill from the file listing, rather than being
    /// handed a whole library in whatever order it happened to be scanned in.
    fn open_library(&mut self, root: &std::path::Path) {
        // Filling in what files are missing runs alongside everything else: reading and writing a
        // whole library takes long enough that waiting for it would look like a hang.
        if self.config.auto_fill_metadata() {
            let paths = library::scan(root)
                .iter()
                .map(|song| song.path().to_path_buf())
                .collect();
            // Each thing the filling may ask the internet for is its own switch, so the consents
            // stay separate: covers and genres are turned on one at a time.
            let settings = autofill::Settings {
                online: self.config.fetch_artwork_online(),
                genres: self.config.fetch_genres_online(),
            };

            self.fills = Some(autofill::fill_in_background(paths, settings));
        }

        let mut screen = PlayerScreen::browsing(root);
        screen.set_hints(self.config.show_control_hints());
        screen.set_theme(self.theme);
        screen.set_messages(!self.config.hide_status_messages());
        self.dress_youtube(&mut screen);

        // A newly opened library is the player now; there is nothing older to go back to.
        self.suspended = None;
        self.screen = Screen::Play(Box::new(screen));
    }

    /// The start menu, dressed as the config asks.
    ///
    /// It offers Continue whenever there is a player screen waiting to be returned to.
    fn start_menu(&self) -> Screen {
        let mut menu = StartMenu::with_continue(self.suspended.is_some());
        menu.set_hints(self.config.show_control_hints());
        menu.set_theme(self.theme);

        Screen::Start(menu)
    }

    /// Tell a player screen what its YouTube mode should work from.
    fn dress_youtube(&self, screen: &mut PlayerScreen) {
        if let Some(folder) = self.download_folder() {
            screen.set_youtube(
                self.config.search_results(),
                self.config.download_format(),
                folder,
            );
        }
    }

    /// Where downloads are written, when something other than the platform default is wanted.
    fn download_folder(&self) -> Option<PathBuf> {
        if let Some(folder) = self.config.download_folder() {
            return Some(folder.to_path_buf());
        }

        self.library_root.as_ref().map(|root| root.join("Downloads"))
    }

    /// Act on a start menu entry.
    fn activate(&mut self, choice: StartMenuChoice) {
        match choice {
            StartMenuChoice::Continue => self.resume_player(),
            // Browsing starts in the configured folder when there is one, else where the player
            // was launched from.
            StartMenuChoice::SelectFolder => {
                let mut browser = match self.config.default_folder() {
                    Some(folder) => FolderBrowser::at(folder),
                    None => FolderBrowser::new(),
                };
                browser.set_hints(self.config.show_control_hints());
                browser.set_theme(self.theme);
                browser.set_messages(!self.config.hide_status_messages());

                self.screen = Screen::Browse(browser);
            }
            StartMenuChoice::OpenConfig => {
                let mut menu = ConfigMenu::new();
                menu.set_theme(self.theme);
                menu.set_messages(!self.config.hide_status_messages());

                self.screen = Screen::Config(menu);
            }
            StartMenuChoice::Quit => self.running = false,
        }
    }

    /// The folder the library is built from, once one has been chosen.
    pub fn library_root(&self) -> Option<&std::path::Path> {
        self.library_root.as_deref()
    }

    /// The folder the player's file listing is showing, when there is a player to ask.
    ///
    /// Answers for the screen set aside as well, since that is the one Continue brings back.
    pub fn browsing_path(&self) -> Option<&std::path::Path> {
        match &self.screen {
            Screen::Play(screen) => Some(screen.browsing_path()),
            _ => self.suspended.as_ref().map(|screen| screen.browsing_path()),
        }
    }

    /// Which screen is showing.
    pub fn screen(&self) -> ScreenKind {
        self.screen.kind()
    }

    /// The settings in force.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// What the daemon is playing, as far as the last status said.
    pub fn player(&self) -> &Player {
        self.remote.player()
    }

    /// Whether the daemon answered when last asked.
    pub fn daemon_connected(&self) -> bool {
        self.remote.connected()
    }

    /// Whether this interface started the daemon it is driving.
    pub fn started_daemon(&self) -> bool {
        self.started_daemon
    }

    /// The protocol versions that did not match, when they did not: this build's, and the
    /// daemon's — `None` for a daemon too old to be asked.
    pub fn protocol_mismatch(&self) -> Option<(u32, Option<u32>)> {
        self.protocol_mismatch
    }

    /// Why the daemon cannot be reached, if it cannot.
    pub fn daemon_error(&self) -> Option<&str> {
        self.daemon_error.as_deref().or_else(|| self.remote.error())
    }


}
