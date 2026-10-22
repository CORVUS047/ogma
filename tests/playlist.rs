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
fn removing_a_track_takes_it_wherever_it_sits() {
    let (_dir, songs) = library("playlist-remove-song");
    let mut playlist = Playlist::with_songs("Remove Song", songs.clone());
    playlist.set_sort(SortBy::Title);

    // Named rather than counted, so the sort the listing is in does not come into it.
    assert!(playlist.remove_song(&songs[2]), "Mike was in it");
    assert_eq!(titles(&playlist), ["Alpha", "Kilo", "Zulu"]);

    // The added order keeps its gap closed, which is what `manual` comes back to.
    playlist.set_sort(SortBy::Manual);
    assert_eq!(titles(&playlist), ["Zulu", "Alpha", "Kilo"]);

    assert!(!playlist.remove_song(&songs[2]), "and is not in it twice");
    assert_eq!(playlist.len(), 3);
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

// ---------------------------------------------------- tracks that move out from under a playlist

#[test]
fn a_saved_playlist_carries_a_fingerprint_for_each_track() {
    let (_library, songs) = library("playlist-prints");
    let dir = common::scratch_dir("playlist-prints-store");

    let playlist = Playlist::with_songs("Late Night", songs[..2].to_vec());
    let path = dir.join(playlist.file_name());
    playlist.save_to(&path).expect("save");

    let text = std::fs::read_to_string(&path).expect("read back");

    assert!(text.contains("[[files]]"), "the fingerprints are a list beside the tracks: {text}");
    assert!(text.contains("audio = "), "{text}");
    // The tracks themselves stay a plain list of paths, editable by hand as they always were.
    assert!(text.contains("tracks = ["), "{text}");

    // And they come back, which is what lets a reloaded playlist go looking for a moved file.
    let loaded = Playlist::load_from(&path).expect("load");
    assert_eq!(loaded, playlist);
}

#[test]
fn a_playlist_without_fingerprints_still_loads() {
    let dir = common::scratch_dir("playlist-no-prints");
    let path = dir.join("old.toml");

    // What a playlist written before fingerprints existed looks like.
    std::fs::write(
        &path,
        "name = \"Old\"\nsort = \"manual\"\ndescending = false\ntracks = [\"/music/a.wav\"]\n",
    )
    .expect("write");

    let loaded = Playlist::load_from(&path).expect("load");

    assert_eq!(loaded.name(), "Old");
    assert_eq!(loaded.len(), 1);
}

#[test]
fn retargeting_follows_one_file() {
    let (library_dir, songs) = library("playlist-retarget-file");
    let mut playlist = Playlist::with_songs("Late Night", songs.clone());

    let was = songs[0].path().to_path_buf();
    let now = library_dir.join("moved-and-renamed.wav");
    std::fs::rename(&was, &now).expect("move the file");

    assert_eq!(playlist.retarget(&was, &now), 1);
    assert_eq!(playlist.as_added()[0].path(), now, "the playlist points where the file went");
    assert_eq!(playlist.len(), songs.len(), "and nothing else changed");

    // Nothing else in the playlist is touched by a move that does not concern it.
    assert_eq!(playlist.retarget(&library_dir.join("never-existed.wav"), &now), 0);
}

#[test]
fn retargeting_follows_a_whole_folder() {
    let (library_dir, songs) = library("playlist-retarget-folder");
    let mut playlist = Playlist::with_songs("Road Trip", songs.clone());

    let elsewhere = library_dir.parent().expect("a parent").join("playlist-retarget-moved");
    let _ = std::fs::remove_dir_all(&elsewhere);
    std::fs::rename(&library_dir, &elsewhere).expect("move the folder");

    assert_eq!(playlist.retarget(&library_dir, &elsewhere), songs.len());

    for song in playlist.as_added() {
        assert!(song.path().starts_with(&elsewhere), "{}", song.path().display());
        assert!(song.path().is_file(), "and it is really there: {}", song.path().display());
    }
}

#[test]
fn a_track_moved_by_something_else_is_found_again_by_its_audio() {
    let (library_dir, songs) = library("playlist-resolve");
    let mut playlist = Playlist::with_songs("Late Night", songs.clone());

    // What a file manager or a shell does: the playlist is told nothing at all.
    let was = songs[1].path().to_path_buf();
    let deeper = library_dir.join("Some Artist").join("Some Album");
    std::fs::create_dir_all(&deeper).expect("make the folders");
    let now = deeper.join("01 - whatever it is called now.wav");
    std::fs::rename(&was, &now).expect("move the file");

    let resolution = playlist.resolve_missing(&library_dir);

    assert_eq!(resolution.moved, vec![(was, now.clone())]);
    assert!(resolution.lost.is_empty());
    assert!(
        playlist.as_added().iter().any(|song| song.path() == now),
        "the playlist now points at where the track went"
    );
    assert_eq!(playlist.len(), songs.len(), "and it is still the same playlist");
}

#[test]
fn a_whole_album_moved_at_once_is_found_in_one_pass() {
    let (library_dir, songs) = library("playlist-resolve-album");
    let mut playlist = Playlist::with_songs("Road Trip", songs.clone());

    let deeper = library_dir.join("Rearranged");
    std::fs::create_dir_all(&deeper).expect("make the folder");

    for song in &songs {
        let name = song.path().file_name().expect("a name");
        std::fs::rename(song.path(), deeper.join(name)).expect("move the file");
    }

    let resolution = playlist.resolve_missing(&library_dir);

    assert_eq!(resolution.moved.len(), songs.len());
    assert!(resolution.lost.is_empty());

    for song in playlist.as_added() {
        assert_eq!(song.path().parent(), Some(deeper.as_path()));
    }
}

#[test]
fn a_deleted_track_is_given_up_on_rather_than_searched_for_again() {
    let (library_dir, songs) = library("playlist-resolve-deleted");
    let mut playlist = Playlist::with_songs("Late Night", songs.clone());

    let gone = songs[2].path().to_path_buf();
    std::fs::remove_file(&gone).expect("delete the file");

    let first = playlist.resolve_missing(&library_dir);

    assert_eq!(first.lost, vec![gone.clone()], "swept the library and did not find it");
    assert!(first.moved.is_empty());
    assert!(
        playlist.as_added().iter().any(|song| song.path() == gone),
        "the track stays in the playlist: it is the user's to remove, not ours"
    );
    assert!(playlist.is_lost(&songs[2]), "and the interface can say so");

    // Sweeping again for a file that is not coming back would mean hashing the whole library on
    // every load, so the second pass does nothing at all.
    let second = playlist.resolve_missing(&library_dir);
    assert!(!second.any(), "{second:?}");

    // Unless asked, which is what makes a file that comes back findable again.
    assert_eq!(playlist.forget_unresolved(), 1);

    let restored = library_dir.join("restored.wav");
    std::fs::copy(songs[0].path(), &restored).expect("copy something back");

    let third = playlist.resolve_missing(&library_dir);
    assert!(third.any(), "the search happens again");
}

#[test]
fn giving_up_is_remembered_in_the_file() {
    let (library_dir, songs) = library("playlist-resolve-remembered");
    let dir = common::scratch_dir("playlist-resolve-remembered-store");

    let mut playlist = Playlist::with_songs("Late Night", songs.clone());
    std::fs::remove_file(songs[0].path()).expect("delete the file");
    playlist.resolve_missing(&library_dir);

    let path = dir.join(playlist.file_name());
    playlist.save_to(&path).expect("save");

    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(
        text.contains(&format!("unresolved = [\"{}\"]", library_dir.display())),
        "the mark says which root was swept, not just that one was: {text}"
    );

    // A reload picks the mark back up, so it is one sweep ever rather than one on every run.
    let mut loaded = Playlist::load_from(&path).expect("load");
    assert!(!loaded.resolve_missing(&library_dir).any());
    assert_eq!(loaded.forget_unresolved(), 1);
}

#[test]
fn a_playlist_whose_tracks_are_all_there_reads_nothing() {
    let (library_dir, songs) = library("playlist-resolve-nothing");
    let mut playlist = Playlist::with_songs("Late Night", songs.clone());

    // Every track is where it should be, so there is nothing to search for and nothing to write.
    assert!(!playlist.resolve_missing(&library_dir).any());

    for song in &songs {
        assert!(!playlist.is_lost(song));
    }
}

#[test]
fn a_stream_is_never_looked_for_on_disk() {
    let dir = common::scratch_dir("playlist-resolve-stream");
    let stream = Song::stream(
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        ogma::song::StreamInfo {
            title: Some("Something".to_string()),
            artist: Some("Someone".to_string()),
            duration: None,
        },
    );

    let mut playlist = Playlist::with_songs("Links", vec![stream.clone()]);

    assert!(!playlist.resolve_missing(&dir).any(), "there is no file to have moved");
    assert!(!playlist.is_lost(&stream));
    assert_eq!(playlist.as_added(), &[stream]);
}

#[test]
fn two_copies_of_one_recording_are_both_found() {
    let (library_dir, songs) = library("playlist-resolve-duplicates");

    // The same audio under two names, as a library that has been merged from two sources holds it.
    let twin = library_dir.join("the-same-again.wav");
    std::fs::copy(songs[0].path(), &twin).expect("copy");

    let mut playlist =
        Playlist::with_songs("Doubled", vec![songs[0].clone(), Song::new(&twin)]);

    let tidied = library_dir.join("tidied");
    std::fs::create_dir_all(&tidied).expect("make the folder");

    let (first, second) = (tidied.join("one.wav"), tidied.join("two.wav"));
    std::fs::rename(songs[0].path(), &first).expect("move one");
    std::fs::rename(&twin, &second).expect("move the other");

    let resolution = playlist.resolve_missing(&library_dir);

    assert_eq!(resolution.moved.len(), 2, "both rows found a file: {resolution:?}");
    assert!(resolution.lost.is_empty(), "neither is given up on: {resolution:?}");

    let mut found: Vec<&Path> = playlist.as_added().iter().map(|song| song.path()).collect();
    found.sort();

    assert_eq!(found, [first.as_path(), second.as_path()]);
}

/// A playlist file as the version before fingerprints wrote one.
fn written_without_fingerprints(dir: &Path, name: &str, songs: &[Song]) -> PathBuf {
    let tracks: Vec<String> =
        songs.iter().map(|song| format!("    \"{}\",", song.path().display())).collect();

    let path = dir.join(format!("{}.toml", name.to_lowercase()));

    std::fs::write(
        &path,
        format!(
            "name = \"{name}\"\nsort = \"manual\"\ndescending = false\ntracks = [\n{}\n]\n",
            tracks.join("\n")
        ),
    )
    .expect("write the old playlist");

    path
}

#[test]
fn a_playlist_from_before_fingerprints_gains_them_when_it_is_checked() {
    let (library_dir, songs) = library("playlist-backfill");
    let store = common::scratch_dir("playlist-backfill-store");
    let path = written_without_fingerprints(&store, "Old", &songs);

    let mut playlist = Playlist::load_from(&path).expect("load");
    let resolution = playlist.resolve_missing(&library_dir);

    assert_eq!(resolution.filled, songs.len(), "every track that is there gained a fingerprint");
    assert!(resolution.moved.is_empty());
    assert!(resolution.lost.is_empty());
    assert!(resolution.any(), "so the caller knows to write the playlist back");

    playlist.save_to(&path).expect("save");
    assert!(
        std::fs::read_to_string(&path).expect("read back").contains("[[files]]"),
        "and the fingerprints are on disk, so this is paid once"
    );

    // Which is the whole point: an outside move is now followable for a playlist that predates all
    // of this.
    let was = songs[0].path().to_path_buf();
    let now = library_dir.join("moved-by-something-else.wav");
    std::fs::rename(&was, &now).expect("move the file");

    let mut reloaded = Playlist::load_from(&path).expect("load");
    assert_eq!(reloaded.resolve_missing(&library_dir).moved, vec![(was, now)]);
}

#[test]
fn backfilling_cannot_reach_a_track_that_is_already_gone() {
    let (library_dir, songs) = library("playlist-backfill-gone");
    let store = common::scratch_dir("playlist-backfill-gone-store");
    let path = written_without_fingerprints(&store, "Old", &songs);

    // The file went before this version ever saw the playlist, so there was never anything to
    // fingerprint: it cannot be searched for, and the listing says so rather than pretending.
    std::fs::remove_file(songs[1].path()).expect("delete");

    let mut playlist = Playlist::load_from(&path).expect("load");
    let resolution = playlist.resolve_missing(&library_dir);

    assert_eq!(resolution.filled, songs.len() - 1);
    assert!(resolution.lost.is_empty(), "it is not searched for, so it is not given up on either");
    assert!(playlist.is_lost(&songs[1]), "but it is still counted as missing");

    // The rest of the playlist is now protected, which is what could be salvaged.
    assert_eq!(playlist.len(), songs.len());
}

#[test]
fn a_second_check_fingerprints_nothing_again() {
    let (library_dir, songs) = library("playlist-backfill-once");
    let store = common::scratch_dir("playlist-backfill-once-store");
    let path = written_without_fingerprints(&store, "Old", &songs);

    let mut playlist = Playlist::load_from(&path).expect("load");
    assert_eq!(playlist.resolve_missing(&library_dir).filled, songs.len());
    assert!(!playlist.resolve_missing(&library_dir).any(), "nothing left to do");
}

#[test]
fn a_track_is_only_written_off_for_the_library_that_was_searched() {
    let (library_dir, songs) = library("playlist-two-roots");
    let elsewhere = common::scratch_dir("playlist-two-roots-elsewhere");

    let mut playlist = Playlist::with_songs("Spanning", songs.clone());

    // The track moves within its own library, but a different folder is what gets opened — a
    // second library, or the library pointed somewhere else in the config.
    let was = songs[0].path().to_path_buf();
    let now = library_dir.join("tidied-away.wav");
    std::fs::rename(&was, &now).expect("move the file");

    let wrong_root = playlist.resolve_missing(&elsewhere);
    assert_eq!(wrong_root.lost, vec![was.clone()], "not there, which says nothing about elsewhere");

    // The real library is still searched when it is opened, rather than the track having been
    // written off by a sweep of a folder it was never in.
    let right_root = playlist.resolve_missing(&library_dir);
    assert_eq!(right_root.moved, vec![(was, now)], "found where it actually went");
}

#[test]
fn saving_leaves_nothing_behind_and_never_a_half_written_playlist() {
    let (_library, songs) = library("playlist-atomic");
    let dir = common::scratch_dir("playlist-atomic-store");

    let playlist = Playlist::with_songs("Late Night", songs.clone());
    let path = dir.join(playlist.file_name());

    playlist.save_to(&path).expect("save");
    playlist.save_to(&path).expect("save again over the top");

    let left: Vec<String> = std::fs::read_dir(&dir)
        .expect("list")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();

    assert_eq!(left, ["late_night.toml"], "the file it is written through is renamed, not left");

    // And the playlist a reader sees is always a whole one.
    assert_eq!(Playlist::load_from(&path).expect("load"), playlist);
}
