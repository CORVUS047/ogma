//! The socket a running player listens on, and what it makes of the commands that arrive.
//!
//! Socket paths are kept short and directly in `/tmp`: a Unix socket's path has a hard length limit
//! of about a hundred bytes, which a nested scratch directory can exceed.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ogma::config::Config;
use ogma::daemon::Daemon;
use ogma::ipc::{self, Command, Server};
use ogma::playlist::Playlist;

/// A short, unique socket path, removed if something left one behind.
fn socket(name: &str) -> PathBuf {
    let path = PathBuf::from(format!("/tmp/ogma-t-{name}-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);

    path
}

/// Answer whatever arrives, with `reply`, until the deadline. Returns the commands it saw.
fn answer_for(server: &Server, reply: &str, how_long: Duration) -> Vec<Command> {
    let deadline = Instant::now() + how_long;
    let mut seen = Vec::new();

    while Instant::now() < deadline {
        for request in server.pending() {
            seen.push(request.command.clone());
            request.answer(reply);
        }

        if !seen.is_empty() {
            break;
        }

        std::thread::sleep(Duration::from_millis(5));
    }

    seen
}

#[test]
fn a_command_reaches_the_player_and_its_answer_comes_back() {
    let path = socket("round-trip");
    let server = Server::start_at(path.clone()).expect("listen");

    // The client blocks until answered, so the answering happens on another thread.
    let sending = std::thread::spawn({
        let path = path.clone();
        move || ipc::send_to(&path, &Command::Volume(-10))
    });

    let seen = answer_for(&server, "ok: volume 40%", Duration::from_secs(5));

    assert_eq!(seen, [Command::Volume(-10)], "the player was given the command");
    assert_eq!(
        sending.join().expect("the client finished").expect("an answer"),
        "ok: volume 40%",
        "and its answer reached the client"
    );
}

#[test]
fn a_command_that_cannot_be_read_is_refused_without_troubling_the_player() {
    let path = socket("bad-command");
    let server = Server::start_at(path.clone()).expect("listen");

    // Written straight to the socket, since the tool would not have built this.
    use std::io::{BufRead, BufReader, Write};
    let stream = std::os::unix::net::UnixStream::connect(&path).expect("connect");
    let mut writer = stream.try_clone().expect("clone");
    writeln!(writer, "dance vigorously").expect("write");

    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).expect("read");

    assert!(reply.starts_with("error:"), "got {reply:?}");
    assert!(reply.contains("unknown command"));
    assert!(server.pending().next().is_none(), "the player was not asked to do anything");
}

#[test]
fn a_socket_left_behind_by_a_player_that_is_gone_is_replaced() {
    let path = socket("stale");

    // What a crash leaves: a socket file with nothing listening.
    std::fs::write(&path, b"not a socket").expect("write file");

    let server = Server::start_at(path.clone()).expect("listen despite the stale file");
    assert_eq!(server.path(), path);

    // And it works, which is what matters.
    let sending = std::thread::spawn({
        let path = path.clone();
        move || ipc::send_to(&path, &Command::Play)
    });
    answer_for(&server, "ok: playing", Duration::from_secs(5));
    assert_eq!(sending.join().expect("joined").expect("answer"), "ok: playing");
}

#[test]
fn a_second_player_does_not_take_over_the_socket() {
    let path = socket("in-use");
    let first = Server::start_at(path.clone()).expect("listen");

    let err = Server::start_at(path.clone()).expect_err("the socket is taken");

    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    assert!(err.to_string().contains("already listening"), "{err}");

    // The first is still the one answering.
    assert_eq!(first.path(), path);
}

#[test]
fn the_socket_goes_when_the_player_does() {
    let path = socket("cleanup");

    {
        let _server = Server::start_at(path.clone()).expect("listen");
        assert!(path.exists());
    }

    assert!(!path.exists(), "a socket left behind would look like a running player");
}

#[test]
fn a_client_with_nobody_to_talk_to_says_so() {
    let path = socket("nobody");

    let err = ipc::send_to(&path, &Command::Play).expect_err("nobody is listening");

    assert!(err.contains("no player listening"), "{err}");
    assert!(err.contains(&path.display().to_string()), "and says where it looked: {err}");
}

// ------------------------------------------------------- what the player makes of each command

/// A daemon with a library and a playlist, and its config kept out of the way.
fn daemon_with_library(name: &str) -> (Daemon, PathBuf) {
    let root = std::env::temp_dir().join(format!("ogma-ipc-{name}"));
    let _ = std::fs::remove_dir_all(&root);

    let music = root.join("music");
    std::fs::create_dir_all(&music).expect("create music folder");

    for (file, title, artist) in [
        ("01.wav", "Xtal", "Aphex Twin"),
        ("02.wav", "Tha", "Aphex Twin"),
        ("03.wav", "Nocturne", "Chopin"),
    ] {
        common::write_wav(
            &music,
            &common::WavSpec {
                name: file,
                title: Some(title),
                artist: Some(artist),
                ..common::WavSpec::default()
            },
        );
    }

    // The config, and so the playlists, live under this tree.
    // SAFETY: the tests calling this run one at a time, being `#[test]`s in one binary that each set
    // the same variables before building their daemon.
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        // A socket of its own, so a player running on this machine is not disturbed.
        std::env::set_var("OGMA_SOCKET", socket(name));
    }

    let mut config = Config::default();
    config.set_default_folder(&music).expect("set folder");
    config.save_to(&Config::config_path().expect("a config path")).expect("save config");

    Playlist::with_songs(
        "Late Night",
        [
            ogma::song::Song::new(music.join("03.wav")),
            ogma::song::Song::new(music.join("01.wav")),
        ],
    )
    .save()
    .expect("save playlist");

    (Daemon::new(), music)
}

#[test]
fn the_player_carries_out_every_command_and_says_what_came_of_it() {
    let (mut daemon, _music) = daemon_with_library("commands");

    // --- nothing queued yet ---
    assert_eq!(daemon.apply(Command::Play), "error: nothing to play");
    assert_eq!(
        daemon.apply(Command::PlayPause),
        "error: nothing to play",
        "with an empty queue there is nothing to toggle"
    );

    // --- playlists ---
    let reply = daemon.apply(Command::LoadPlaylist("Late Night".to_string()));
    assert!(reply.starts_with("ok:"), "{reply}");
    assert!(reply.contains("2 from Late Night"), "{reply}");
    assert_eq!(daemon.player().queue().len(), 2);

    // A part of the name is enough, and the playlist's own order is what arrives.
    daemon.apply(Command::LoadPlaylist("late".to_string()));
    assert_eq!(daemon.player().queue().len(), 2);

    let reply = daemon.apply(Command::AddPlaylist("Late Night".to_string()));
    assert!(reply.contains("added 2"), "{reply}");
    assert_eq!(daemon.player().queue().len(), 4);

    assert_eq!(
        daemon.apply(Command::LoadPlaylist("nothing".to_string())),
        "error: no playlist matching \"nothing\""
    );

    // --- transport ---
    let reply = daemon.apply(Command::Play);
    assert!(reply.starts_with("ok: playing"), "{reply}");
    assert!(daemon.player().is_playing());

    assert_eq!(daemon.apply(Command::Pause), "ok: paused");
    assert!(daemon.player().is_paused());

    // Toggling goes both ways from wherever it finds things.
    let reply = daemon.apply(Command::PlayPause);
    assert!(reply.starts_with("ok: playing"), "{reply}");
    assert!(daemon.player().is_playing());

    assert_eq!(daemon.apply(Command::PlayPause), "ok: paused");
    assert!(daemon.player().is_paused());

    daemon.apply(Command::PlayPause);
    assert!(daemon.player().is_playing(), "and back again");

    let before = daemon.player().current().cloned();
    let reply = daemon.apply(Command::Next);
    assert!(reply.starts_with("ok:"), "{reply}");
    assert_ne!(daemon.player().current().cloned(), before, "it moved on");

    let reply = daemon.apply(Command::Previous);
    assert!(reply.starts_with("ok:"), "{reply}");
    assert_eq!(daemon.player().current().cloned(), before, "and back again");

    // --- shuffle ---
    let reply = daemon.apply(Command::Shuffle);
    assert!(reply.contains("shuffled"), "{reply}");

    // --- volume, up and down ---
    let reply = daemon.apply(Command::Volume(-10));
    assert!(reply.contains("40%"), "{reply}");
    assert!(reply.contains("dB"), "it says what that means: {reply}");

    let reply = daemon.apply(Command::Volume(25));
    assert!(reply.contains("65%"), "{reply}");
    assert_eq!(daemon.player().volume(), 0.65);

    // The ends hold.
    daemon.apply(Command::Volume(-500));
    assert_eq!(daemon.player().volume(), 0.0);
    daemon.apply(Command::Volume(500));
    assert_eq!(daemon.player().volume(), 1.0);

    // --- a song by name ---
    let reply = daemon.apply(Command::PlaySong("noct".to_string()));
    assert!(reply.contains("Chopin — Nocturne"), "{reply}");
    assert_eq!(daemon.player().current().map(|song| song.display_title()), Some("Nocturne".to_string()));

    // Found across the library, not only in the queue.
    let reply = daemon.apply(Command::PlaySong("Tha".to_string()));
    assert!(reply.contains("Tha"), "{reply}");

    assert_eq!(
        daemon.apply(Command::PlaySong("zzzz".to_string())),
        "error: no song matching \"zzzz\""
    );

    // --- and finishing is a command like any other ---
    assert!(daemon.is_running());
    assert_eq!(daemon.apply(Command::Quit), "ok: finishing");
    assert!(!daemon.is_running(), "the loop is asked to end");
}

