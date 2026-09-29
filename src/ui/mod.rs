//! The terminal interface.

mod artwork_view;
mod config_menu;
mod file_pane;
mod folder_browser;
mod player_screen;
mod playlist_pane;
mod start_menu;
mod youtube;

pub use config_menu::{ConfigMenu, ConfigMenuOutcome};
pub use file_pane::FilePane;
pub use folder_browser::{FolderBrowser, FolderBrowserOutcome};
pub use player_screen::{PlayerScreen, PlayerScreenOutcome};
pub use playlist_pane::{PlaylistAction, PlaylistPane};
pub use start_menu::{StartMenu, StartMenuChoice};
pub use youtube::{YoutubeOutcome, YoutubeScreen};
