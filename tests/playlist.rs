//! Playlists: their order, their sorting, and their files on disk.

mod common;

use std::path::{Path, PathBuf};

use common::WavSpec;
use ogma::playlist::{Playlist, PlaylistError, SortBy};
use ogma::song::Song;

/// Four tracks whose tags differ in every way a sort can use.
fn library(name: &str) -> (PathBuf, Vec<Song>) {
    let dir = common::scratch_dir(name);

    let specs = [
        WavSpec {
            name: "c-zulu.wav",
            title: Some("Zulu"),
            artist: Some("Beta Band"),
            album: Some("Middle"),
            track: Some(2),
            year: Some("2001"),
            millis: 900,
        },
        WavSpec {
            name: "a-alpha.wav",
            title: Some("Alpha"),
            artist: Some("Cetacean"),
            album: Some("Zenith"),
            track: Some(1),
            year: Some("1995"),
            millis: 300,
        },
        WavSpec {
            name: "d-mike.wav",
            title: Some("Mike"),
            artist: Some("Alpine"),
            album: Some("Aurora"),
            track: Some(3),
            year: Some("2010"),
            millis: 1_500,
        },
        WavSpec {
            name: "b-kilo.wav",
            title: Some("Kilo"),
            artist: Some("Beta Band"),
            album: Some("Middle"),
            track: Some(1),
            year: Some("2001"),
            millis: 600,
        },
    ];

    let songs = specs.iter().map(|spec| Song::new(common::write_wav(&dir, spec))).collect();

    (dir, songs)
}

/// The titles of a playlist's tracks, in the order the playlist says.
fn titles(playlist: &Playlist) -> Vec<String> {
    playlist
        .ordered()
        .into_iter()
        .map(|song| song.display_title())
        .collect()
}

#[test]
fn a_playlist_holds_a_name_and_its_tracks() {
    let (_dir, songs) = library("playlist-basics");

    let mut playlist = Playlist::new("Late Night");
    assert_eq!(playlist.name(), "Late Night");
    assert!(playlist.is_empty());

    playlist.add(songs[0].clone());
    playlist.add(songs[1].clone());

    assert_eq!(playlist.len(), 2);
    assert_eq!(playlist.as_added(), &songs[..2]);

    // Adding what is already there does nothing.
    assert_eq!(playlist.add_all(vec![songs[0].clone(), songs[2].clone()]), 1);
    assert_eq!(playlist.len(), 3);

    playlist.clear();
    assert!(playlist.is_empty());
}

#[test]
fn manual_order_is_the_order_tracks_were_added() {
    let (_dir, songs) = library("playlist-manual");
    let playlist = Playlist::with_songs("As Added", songs.clone());

    assert_eq!(playlist.sort(), SortBy::Manual, "the default");
    assert_eq!(titles(&playlist), ["Zulu", "Alpha", "Mike", "Kilo"]);
}

#[test]
fn each_sort_orders_by_what_it_says() {
    let (_dir, songs) = library("playlist-sorts");
    let mut playlist = Playlist::with_songs("Every Way", songs);

    playlist.set_sort(SortBy::Title);
    assert_eq!(titles(&playlist), ["Alpha", "Kilo", "Mike", "Zulu"]);

    // Artist, then album, then disc and track: an artist's records in their running order.
    playlist.set_sort(SortBy::Artist);
    assert_eq!(titles(&playlist), ["Mike", "Kilo", "Zulu", "Alpha"]);

    playlist.set_sort(SortBy::Album);
    assert_eq!(titles(&playlist), ["Mike", "Kilo", "Zulu", "Alpha"]);

    playlist.set_sort(SortBy::Year);
    assert_eq!(titles(&playlist), ["Alpha", "Kilo", "Zulu", "Mike"]);

    playlist.set_sort(SortBy::Duration);
    assert_eq!(titles(&playlist), ["Alpha", "Kilo", "Zulu", "Mike"]);

    playlist.set_sort(SortBy::FileName);
    assert_eq!(titles(&playlist), ["Alpha", "Kilo", "Zulu", "Mike"]);

    // And manual gives the original order back, which is why that order is the one kept.
    playlist.set_sort(SortBy::Manual);
    assert_eq!(titles(&playlist), ["Zulu", "Alpha", "Mike", "Kilo"]);
}

#[test]
fn the_direction_can_be_reversed() {
    let (_dir, songs) = library("playlist-direction");
    let mut playlist = Playlist::with_songs("Backwards", songs);
    playlist.set_sort(SortBy::Title);

    assert!(playlist.toggle_direction());
    assert!(playlist.descending());
    assert_eq!(titles(&playlist), ["Zulu", "Mike", "Kilo", "Alpha"]);

    assert!(!playlist.toggle_direction());
    assert_eq!(titles(&playlist), ["Alpha", "Kilo", "Mike", "Zulu"]);
}

#[test]
fn sorting_is_stable_for_tracks_that_tie() {
    let dir = common::scratch_dir("playlist-ties");

    // Same everything but the file name, which is the tie-breaker.
    let songs: Vec<Song> = ["b.wav", "a.wav", "c.wav"]
        .iter()
        .map(|name| {
            Song::new(common::write_wav(
                &dir,
                &WavSpec { name, title: Some("Same"), ..WavSpec::default() },
            ))
        })
        .collect();

    let mut playlist = Playlist::with_songs("Ties", songs);
    playlist.set_sort(SortBy::Title);

    let first = titles(&playlist);
    let names: Vec<String> = playlist
        .ordered()
        .into_iter()
        .map(|song| song.path().file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    assert_eq!(names, ["a.wav", "b.wav", "c.wav"], "ties fall back to the path");
    assert_eq!(first, titles(&playlist), "and the order does not wander between draws");
}

#[test]
fn removing_takes_the_track_the_listing_shows() {
    let (_dir, songs) = library("playlist-remove");
    let mut playlist = Playlist::with_songs("Remove", songs);
    playlist.set_sort(SortBy::Title);

    // Index 0 of the sorted order is "Alpha", not the first track added.
    let removed = playlist.remove(0).expect("a track");

    assert_eq!(removed.display_title(), "Alpha");
    assert_eq!(titles(&playlist), ["Kilo", "Mike", "Zulu"]);
    assert_eq!(playlist.len(), 3);

    assert!(playlist.remove(99).is_none(), "out of range is not an error");
}

// ------------------------------------------------------------------------------------- on disk

#[test]
fn the_file_name_is_the_name_in_lowercase_with_underscores() {
    assert_eq!(Playlist::new("Late Night").file_name(), "late_night.toml");
    assert_eq!(Playlist::new("ROAD TRIP 2024").file_name(), "road_trip_2024.toml");
    assert_eq!(Playlist::new("Sunday   Morning!").file_name(), "sunday_morning.toml");
    assert_eq!(Playlist::new("90s / 00s").file_name(), "90s_00s.toml");
}

#[test]
fn playlists_live_in_a_playlists_directory_beside_the_config() {
    let directory = Playlist::directory().expect("a directory");
    let config = ogma::config::Config::config_path().expect("a config path");

    assert_eq!(directory.parent(), config.parent(), "beside the config file");
    assert!(directory.ends_with("playlists"));

    assert!(
        Playlist::new("Late Night").path().expect("a path").ends_with("playlists/late_night.toml")
    );
}

#[test]
fn a_playlist_survives_being_written_and_read_back() {
    let (_library, songs) = library("playlist-roundtrip");
    let dir = common::scratch_dir("playlist-roundtrip-store");

    let mut playlist = Playlist::with_songs("Late Night", songs.clone());
    playlist.set_sort(SortBy::Title);
    playlist.toggle_direction();

    let path = dir.join(playlist.file_name());
    playlist.save_to(&path).expect("save");

    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(text.contains("name = \"Late Night\""), "{text}");
    assert!(text.contains("sort = \"title\""), "the sort is part of the playlist: {text}");
    assert!(text.contains("descending = true"));
    assert!(text.contains("c-zulu.wav"), "and the tracks are listed: {text}");

    let loaded = Playlist::load_from(&path).expect("load");

    assert_eq!(loaded.name(), "Late Night");
    assert_eq!(loaded.sort(), SortBy::Title);
    assert!(loaded.descending());
    // The order tracks were added in is what is stored, so manual still means something.
    assert_eq!(loaded.as_added(), playlist.as_added());
    assert_eq!(titles(&loaded), titles(&playlist));
}

#[test]
fn a_playlist_with_no_usable_name_cannot_be_saved() {
    let dir = common::scratch_dir("playlist-noname");
    let playlist = Playlist::new("!!!");

    assert!(matches!(playlist.path(), Err(PlaylistError::EmptyName)));
    assert!(matches!(
        playlist.save_to(&dir.join("x.toml")),
        Err(PlaylistError::EmptyName)
    ));
}

#[test]
fn loading_a_directory_reads_every_playlist_and_skips_the_broken() {
    let dir = common::scratch_dir("playlist-load-all");
    let (_library, songs) = library("playlist-load-all-songs");

    Playlist::with_songs("Road Trip", songs[..2].to_vec())
        .save_to(&dir.join("road_trip.toml"))
        .expect("save");
    Playlist::new("Ambient")
        .save_to(&dir.join("ambient.toml"))
        .expect("save");

    // Neither of these should stop the others being read.
    std::fs::write(dir.join("broken.toml"), b"this is not a playlist").expect("write broken");
    std::fs::write(dir.join("notes.txt"), b"name = \"Ignored\"").expect("write other file");

    let playlists = Playlist::load_all_from(&dir);
    let names: Vec<&str> = playlists.iter().map(Playlist::name).collect();

    assert_eq!(names, ["Ambient", "Road Trip"], "by name, and the broken one left out");
    assert_eq!(playlists[1].len(), 2);
}

#[test]
fn a_missing_directory_holds_no_playlists() {
    assert!(Playlist::load_all_from(Path::new("/definitely/not/here")).is_empty());
}

#[test]
fn deleting_removes_the_file_and_forgives_a_missing_one() {
    let dir = common::scratch_dir("playlist-delete");
    let playlist = Playlist::new("Temporary");
    let path = dir.join(playlist.file_name());

    playlist.save_to(&path).expect("save");
    assert!(path.exists());

    std::fs::remove_file(&path).expect("remove");
    // `delete` works against the real playlists directory, where this was never saved: a playlist
    // that has no file is not an error to delete.
    assert!(playlist.delete().is_ok());
}

// ----------------------------------------------------------------- tracks played from the network

/// A YouTube result, as the search would hand one over.
fn a_stream() -> Song {
    Song::stream(
        "https://www.youtube.com/watch?v=aaaaaaaaaaa",
        ogma::song::StreamInfo {
            title: Some("First Song".to_string()),
            artist: Some("A Channel".to_string()),
            duration: Some(std::time::Duration::from_secs(131)),
        },
    )
}

#[test]
fn a_stream_keeps_its_name_through_a_playlist() {
    let (root, songs) = library("playlist-stream");
    let path = root.join("mixed.toml");

    let playlist = Playlist::with_songs("Mixed", [songs[0].clone(), a_stream()]);
    playlist.save_to(&path).expect("save");

    let back = Playlist::load_from(&path).expect("load");
    let tracks = back.as_added();

    assert_eq!(tracks.len(), 2, "both kinds of track came back");
    assert!(!tracks[0].is_stream(), "the file is still a file");

    let stream = &tracks[1];
    assert!(stream.is_stream(), "and the URL is still something to stream");
    assert_eq!(stream.title(), Some("First Song"));
    assert_eq!(stream.display_artist(), "A Channel");
    assert_eq!(stream.duration(), Some(std::time::Duration::from_secs(131)));
    assert_eq!(stream.uri(), "https://www.youtube.com/watch?v=aaaaaaaaaaa");
}

#[test]
fn a_stream_sorts_by_what_it_is_called() {
    let (root, _songs) = library("playlist-stream-sort");
    let path = root.join("streams.toml");

    let later = Song::stream(
        "https://www.youtube.com/watch?v=bbbbbbbbbbb",
        ogma::song::StreamInfo {
            title: Some("Zebra".to_string()),
            artist: Some("B Channel".to_string()),
            duration: Some(std::time::Duration::from_secs(90)),
        },
    );

    let mut playlist = Playlist::with_songs("Streams", [later, a_stream()]);
    playlist.set_sort(SortBy::Title);
    playlist.save_to(&path).expect("save");

    let back = Playlist::load_from(&path).expect("load");

    assert_eq!(titles(&back), vec!["First Song", "Zebra"], "titles the sort can see");
}

#[test]
fn the_tracks_are_still_a_plain_list_of_paths_on_disk() {
    let (root, songs) = library("playlist-stream-format");
    let path = root.join("mixed.toml");

    Playlist::with_songs("Mixed", [songs[0].clone(), a_stream()])
        .save_to(&path)
        .expect("save");

    let text = std::fs::read_to_string(&path).expect("read");

    // The URL sits in `tracks` like any other entry, so a reader that knows nothing about streams
    // still sees every track in order; the name lives beside it.
    assert!(text.contains("https://www.youtube.com/watch?v=aaaaaaaaaaa"), "{text}");
    assert!(text.contains("[[streams]]"), "{text}");
    assert!(text.contains("First Song"), "{text}");
}

#[test]
fn a_playlist_written_before_streams_existed_still_loads() {
    let (root, songs) = library("playlist-old-format");
    let path = root.join("old.toml");

    let text = format!(
        "name = \"Old\"\nsort = \"manual\"\ndescending = false\ntracks = [\"{}\"]\n",
        songs[0].path().display()
    );
    std::fs::write(&path, text).expect("write");

    let back = Playlist::load_from(&path).expect("load");

    assert_eq!(back.len(), 1);
    assert!(!back.as_added()[0].is_stream());
}
