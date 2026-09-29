//! What comes back from yt-dlp, and what it is turned into.
//!
//! Nothing here runs yt-dlp or touches the network: the search output is fed in as the text the
//! real thing would print, so the parsing is checked on a machine with no yt-dlp and no connection.

use std::time::Duration;

use ogma::ytdl::{self, Format, Track};

/// Two results, in the shape `yt-dlp --flat-playlist --dump-json` prints them.
const TWO_RESULTS: &str = r#"
{"_type": "url", "id": "aaaaaaaaaaa", "url": "https://www.youtube.com/watch?v=aaaaaaaaaaa", "title": "First Song", "duration": 131, "channel": "A Channel", "uploader": "An Uploader"}
{"_type": "url", "id": "bbbbbbbbbbb", "url": "https://www.youtube.com/watch?v=bbbbbbbbbbb", "title": "Second Song", "duration": 245.5, "channel": "Another Channel"}
"#;

#[test]
fn reads_the_results_yt_dlp_prints() {
    let results = ytdl::parse_search(TWO_RESULTS);

    assert_eq!(results.len(), 2);

    assert_eq!(results[0].id, "aaaaaaaaaaa");
    assert_eq!(results[0].title, "First Song");
    assert_eq!(results[0].url, "https://www.youtube.com/watch?v=aaaaaaaaaaa");
    assert_eq!(results[0].duration, Some(Duration::from_secs(131)));
    // The uploader is preferred over the channel, being the nearer thing to an artist.
    assert_eq!(results[0].uploader.as_deref(), Some("An Uploader"));

    // A result with no uploader falls back to the channel the search page gives.
    assert_eq!(results[1].uploader.as_deref(), Some("Another Channel"));
    assert_eq!(results[1].duration, Some(Duration::from_millis(245_500)));
}

#[test]
fn a_result_with_no_url_gets_one_from_its_id() {
    let results = ytdl::parse_search(r#"{"id": "ccccccccccc", "title": "Third Song"}"#);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].url, "https://www.youtube.com/watch?v=ccccccccccc");
}

#[test]
fn unusable_lines_are_skipped_rather_than_failing_the_search() {
    let output = format!(
        "not json at all\n{}\n{{\"title\": \"no id here\"}}\n",
        r#"{"id": "ddddddddddd", "title": "Fourth Song"}"#
    );

    let results = ytdl::parse_search(&output);

    assert_eq!(results.len(), 1, "the one good line survives");
    assert_eq!(results[0].title, "Fourth Song");
}

#[test]
fn a_live_stream_has_no_length_but_is_still_listed() {
    let results = ytdl::parse_search(r#"{"id": "eeeeeeeeeee", "title": "Live Now", "duration": null}"#);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].duration, None);
    assert_eq!(results[0].display_duration(), "--:--");
}

#[test]
fn a_result_becomes_a_song_that_knows_what_it_is() {
    let track = Track {
        id: "fffffffffff".to_string(),
        url: "https://www.youtube.com/watch?v=fffffffffff".to_string(),
        title: "Fifth Song".to_string(),
        uploader: Some("Someone".to_string()),
        duration: Some(Duration::from_secs(200)),
    };

    let song = track.song();

    assert!(song.is_stream(), "it plays from the network, not from disk");
    assert_eq!(song.title(), Some("Fifth Song"));
    assert_eq!(song.display_artist(), "Someone");
    assert_eq!(song.duration(), Some(Duration::from_secs(200)));
    assert_eq!(song.uri(), track.url);
    // Nothing tries to read tags off a URL, so the title stands rather than being lost to a
    // failed probe.
    assert_eq!(song.display_title(), "Fifth Song");
}

#[test]
fn only_urls_are_streams() {
    assert!(ytdl::is_stream("https://www.youtube.com/watch?v=ggggggggggg"));
    assert!(ytdl::is_stream("http://example.com/audio"));
    assert!(!ytdl::is_stream("/home/someone/music/track.flac"));
    assert!(!ytdl::is_stream("track.flac"));
}

#[test]
fn the_download_formats_cycle_and_describe_themselves() {
    // Nothing is converted by default: YouTube's audio is already lossy.
    assert_eq!(Format::default(), Format::Original);

    let mut format = Format::default();

    for _ in 0..Format::ALL.len() {
        format = format.next();
    }

    assert_eq!(format, Format::default(), "the cycle comes back round");

    for choice in Format::ALL {
        assert!(!choice.label().is_empty());
        assert!(!choice.describe().is_empty());
    }
}

/// Fetching a real track, which needs yt-dlp, ffmpeg and a connection, so it is not part of the
/// usual run: `cargo test --test ytdl -- --ignored`.
///
/// The track is Kevin MacLeod's, published under Creative Commons.
#[test]
#[ignore = "downloads from YouTube"]
fn a_track_can_be_fetched_from_youtube() {
    let folder = std::env::temp_dir().join("ogma-tests-ytdl-download");
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("create folder");

    let results = ytdl::search("kevin macleod monkeys spinning monkeys", 1).expect("search");
    assert!(!results.is_empty(), "the search found something");

    let path = ytdl::download(&results[0], &folder, Format::Original).expect("download");

    assert!(path.is_file(), "the file is where yt-dlp said: {}", path.display());
    assert!(path.starts_with(&folder), "and inside the folder it was given");
    assert!(std::fs::metadata(&path).expect("size").len() > 0);
}

#[test]
fn a_title_that_would_break_a_row_is_tidied() {
    let results = ytdl::parse_search(
        r#"{"id": "hhhhhhhhhhh", "title": "Two\nLines\tApart", "uploader": "  A   Channel  "}"#,
    );

    assert_eq!(results[0].title, "Two Lines Apart", "nothing that moves the cursor survives");
    assert_eq!(results[0].uploader.as_deref(), Some("A Channel"));
}
