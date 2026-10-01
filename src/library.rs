//! Finding the music in a folder.

use std::path::{Path, PathBuf};

use crate::meta;
use crate::song::Song;

/// How deep the scan walks below the folder it was given.
///
/// Libraries are laid out as artist/album/disc at the deepest, so this is generous while keeping a
/// mistyped root from walking a whole filesystem.
const MAX_DEPTH: usize = 8;

/// Every audio file at or below `root`, sorted by path so albums stay in order.
///
/// Extensions are the filter here; nothing is opened. A file that turns out to be unreadable shows
/// up as a [`Song`] whose metadata is `None`, which is what the UI displays as unknown.
pub fn scan(root: &Path) -> Vec<Song> {
    paths(root).into_iter().map(Song::new).collect()
}

/// The path of every audio file at or below `root`, sorted.
///
/// What [`scan`] is built on, for the sake of work that wants the files themselves rather than
/// anything to display: the background metadata fill walks the library this way on every pass.
pub fn paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    walk(root, 0, &mut paths);

    // Sorting by full path keeps disc and track files adjacent, which is the order they are named in.
    paths.sort();

    paths
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        // An unreadable folder is skipped rather than failing the whole scan.
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        let hidden = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.'));

        if hidden {
            continue;
        }

        if path.is_dir() {
            walk(&path, depth + 1, found);
        } else if meta::has_audio_extension(&path) {
            found.push(path);
        }
    }
}
