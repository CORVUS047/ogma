//! Grouping a folder's music by what its tags say.

mod common;

use std::path::PathBuf;

use common::WavSpec;
use ogma::browse::{self, Facet};
use ogma::song::Song;

/// A small library: two by one artist on one album, one by another, one with no tags at all.
fn library(name: &str) -> PathBuf {
    let root = common::scratch_dir(name);

    common::write_wav(
        &root,
        &WavSpec {
            name: "01.wav",
            title: Some("Xtal"),
            artist: Some("Aphex Twin"),
            album: Some("Selected Ambient Works"),
            year: Some("1992"),
            ..WavSpec::default()
        },
    );
    common::write_wav(
        &root,
        &WavSpec {
            name: "02.wav",
            title: Some("Tha"),
            artist: Some("Aphex Twin"),
            album: Some("Selected Ambient Works"),
            year: Some("1992"),
            ..WavSpec::default()
        },
    );

    std::fs::create_dir_all(root.join("piano")).expect("create folder");
    common::write_wav(
        &root.join("piano"),
        &WavSpec {
            name: "03.wav",
            title: Some("Nocturne"),
            artist: Some("Chopin"),
            album: Some("Nocturnes"),
            year: Some("1832"),
            ..WavSpec::default()
        },
    );
    common::write_wav(&root, &WavSpec { name: "04.wav", ..WavSpec::default() });

    root
}

fn values(songs: &[Song], facet: Facet) -> Vec<String> {
    browse::group(songs, facet).into_iter().map(|group| group.value).collect()
}

#[test]
fn the_whole_folder_is_read_including_what_is_below_it() {
    let songs = browse::index(&library("browse-index"));

    assert_eq!(songs.len(), 4, "the subfolder's track counts too");
    // The tags are read by the indexing, so nothing has to open a file to draw a row.
    assert!(songs.iter().any(|song| song.title() == Some("Nocturne")));
}

#[test]
fn artists_group_their_tracks_and_sort_by_name() {
    let songs = browse::index(&library("browse-artist"));
    let groups = browse::group(&songs, Facet::Artist);

    assert_eq!(
        groups.iter().map(|group| group.value.as_str()).collect::<Vec<_>>(),
        vec!["Aphex Twin", "Chopin", "Unknown Artist"],
        "by name, with the untagged track last"
    );

    assert_eq!(groups[0].songs.len(), 2, "both of theirs are in one group");
    assert_eq!(groups[2].songs.len(), 1);
}

#[test]
fn each_grouping_files_the_tracks_its_own_way() {
    let songs = browse::index(&library("browse-facets"));

    assert_eq!(values(&songs, Facet::Album), vec!["Nocturnes", "Selected Ambient Works", "Unknown Album"]);
    assert_eq!(values(&songs, Facet::Year), vec!["1832", "1992", "Unknown Year"]);
    // Nothing in the fixture carries a genre, so everything lands in the one group.
    assert_eq!(values(&songs, Facet::Genre), vec!["Unknown Genre"]);
}

#[test]
fn a_tracks_order_within_its_group_is_the_order_it_was_scanned_in() {
    let songs = browse::index(&library("browse-order"));
    let groups = browse::group(&songs, Facet::Artist);

    let titles: Vec<Option<&str>> = groups[0].songs.iter().map(|song| song.title()).collect();

    assert_eq!(titles, vec![Some("Xtal"), Some("Tha")], "01 before 02, as the files are named");
}

#[test]
fn the_groupings_cycle_and_name_themselves() {
    let mut facet = Facet::default();
    assert_eq!(facet, Facet::Artist, "artist is where browsing starts");

    for _ in 0..Facet::ALL.len() {
        facet = facet.next();
    }

    assert_eq!(facet, Facet::default(), "the cycle comes back round");

    for choice in Facet::ALL {
        assert!(!choice.label().is_empty());
        assert!(choice.unknown().starts_with("Unknown"));
    }
}

#[test]
fn an_empty_folder_groups_into_nothing() {
    let root = common::scratch_dir("browse-empty");
    let songs = browse::index(&root);

    assert!(songs.is_empty());
    assert!(browse::group(&songs, Facet::Artist).is_empty());
}
