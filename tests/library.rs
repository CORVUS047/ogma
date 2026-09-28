//! Scanning a folder for music.

mod common;

use ogma::library;

#[test]
fn finds_audio_files_recursively_in_path_order() {
    let root = common::scratch_dir("library-scan");

    std::fs::create_dir_all(root.join("artist").join("album")).expect("create dirs");
    std::fs::create_dir_all(root.join(".hidden")).expect("create hidden dir");

    for path in [
        root.join("loose.mp3"),
        root.join("artist").join("b.flac"),
        root.join("artist").join("a.opus"),
        root.join("artist").join("album").join("01.wav"),
        root.join("notes.txt"),
        root.join("cover.jpg"),
        root.join(".hidden").join("skipped.mp3"),
        root.join(".dotfile.flac"),
    ] {
        std::fs::write(path, b"contents do not matter here").expect("write file");
    }

    let songs = library::scan(&root);
    let names: Vec<String> = songs
        .iter()
        .map(|song| song.path().strip_prefix(&root).expect("under root").display().to_string())
        .collect();

    assert_eq!(
        names,
        [
            // Sorted by full path, so albums stay in their own order.
            "artist/a.opus",
            "artist/album/01.wav",
            "artist/b.flac",
            "loose.mp3",
        ]
    );
}

#[test]
fn a_folder_with_no_music_scans_to_nothing() {
    let root = common::scratch_dir("library-empty");
    std::fs::write(root.join("readme.md"), b"no music").expect("write file");

    assert!(library::scan(&root).is_empty());
}

#[test]
fn a_missing_folder_scans_to_nothing_rather_than_failing() {
    assert!(library::scan(std::path::Path::new("/definitely/not/here")).is_empty());
}
