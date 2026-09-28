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
    ConfigMenu, ConfigMenuOutcome, FolderBrowser, FolderBrowserOutcome, PlayerScreen,
    PlayerScreenOutcome, StartMenu, StartMenuChoice,
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
}

/// Which screen has the user's attention.
#[derive(Debug)]
enum Screen {
    Start(StartMenu),
    Config(ConfigMenu),
    Browse(FolderBrowser),
    Play(Box<PlayerScreen>),
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

        let mut remote = Remote::new();

        // Being counted among the daemon's interfaces is what keeps it playing when one of several
        // interfaces closes. It is told who started it, so the last one out is the one that closes it.
        remote.attach(started_daemon);
        remote.refresh();

        let theme = Theme::load(config.custom_theme());

        let mut menu = StartMenu::new();
        menu.set_hints(config.show_control_hints());
        menu.set_theme(theme);

        let mut app = App {
            screen: Screen::Start(menu),
            // A configured folder is the library root until the user picks another.
            library_root: config.default_folder().map(PathBuf::from),
            config,
            remote,
            started_daemon,
            daemon_error,
            fills: None,
            theme,
            suspended: None,
            running: true,
        };

        // With a folder already configured there is nothing to ask about: go straight to the player.
        // A folder that has since been moved or removed is not usable, so that falls back to the
        // menu, where it can be pointed somewhere else.
        if let Some(folder) = app.config.default_folder().map(PathBuf::from)
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
        while self.running {
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

    fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();

        match &mut self.screen {
            Screen::Start(menu) => menu.render(frame, area),
            Screen::Config(menu) => menu.render(frame, area, &self.config),
            Screen::Browse(browser) => browser.render(frame, area),
            Screen::Play(screen) => screen.render(frame, area, self.remote.player()),
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
            // Asking the internet is its own switch, so the two consents stay separate.
            let settings = autofill::Settings { online: self.config.fetch_artwork_online() };

            self.fills = Some(autofill::fill_in_background(paths, settings));
        }

        let mut screen = PlayerScreen::browsing(root);
        screen.set_hints(self.config.show_control_hints());
        screen.set_theme(self.theme);
        screen.set_messages(!self.config.hide_status_messages());

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

    /// Why the daemon cannot be reached, if it cannot.
    pub fn daemon_error(&self) -> Option<&str> {
        self.daemon_error.as_deref().or_else(|| self.remote.error())
    }


}
