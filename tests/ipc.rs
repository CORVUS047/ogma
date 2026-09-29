//! The socket a running player listens on, and what it makes of the commands that arrive.
//!
//! Socket paths are kept short and directly in `/tmp`: a Unix socket's path has a hard length limit
//! of about a hundred bytes, which a nested scratch directory can exceed.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ogma::config::Config;
use ogma::daemon::Daemon;
use ogma::ipc::{self, Command, OnLeave, Server, Status};
use ogma::player::Repeat;
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

#[test]
fn the_player_cycles_what_repeats_and_reports_it_in_its_status() {
    let mut daemon = Daemon::with_config(&Config::default());

    let status = Status::parse(&daemon.apply(Command::Status)).expect("a status");
    assert_eq!(status.repeat, "off", "nothing repeats until it is asked for");

    // No mode named cycles, which is what a keybinding sends.
    assert_eq!(daemon.apply(Command::Repeat(None)), "ok: repeat queue");
    assert_eq!(daemon.apply(Command::Repeat(None)), "ok: repeat song");
    assert_eq!(daemon.apply(Command::Repeat(None)), "ok: repeat off");

    // A named one sets it, which is what a script sends.
    assert_eq!(daemon.apply(Command::Repeat(Some(Repeat::Song))), "ok: repeat song");
    assert_eq!(daemon.apply(Command::Repeat(Some(Repeat::Song))), "ok: repeat song", "again is fine");

    let status = Status::parse(&daemon.apply(Command::Status)).expect("a status");
    assert_eq!(status.repeat, "song", "and the interface can see it");
}

// -------------------------------------------------------------- interfaces coming and going

#[test]
fn an_interface_attaches_over_a_connection_it_then_holds_open() {
    let path = socket("attach");
    let server = Server::start_at(path.clone()).expect("listen");

    // Attaching is answered like any command; the connection it came on stays open afterwards.
    let attaching = std::thread::spawn({
        let path = path.clone();
        move || ipc::attach_to(&path, true)
    });

    let seen = answer_for(&server, "ok: attached · 1 interface", Duration::from_secs(5));
    let attachment = attaching.join().expect("joined").expect("attached");

    assert_eq!(
        seen,
        [Command::Attach { id: attachment.id().to_string(), spawned: true }],
        "the player was told an interface is here, and that it started this daemon"
    );

    // Saying goodbye carries what the interface would like done, which is the player's to act on.
    let leaving = std::thread::spawn({
        let path = path.clone();
        let id = attachment.id().to_string();
        move || ipc::send_to(&path, &Command::Leave { id, on_leave: OnLeave::StopIfSpawned })
    });

    let seen = answer_for(&server, "ok: left · finishing", Duration::from_secs(5));
    assert_eq!(
        seen,
        [Command::Leave {
            id: attachment.id().to_string(),
            on_leave: OnLeave::StopIfSpawned
        }]
    );
    assert_eq!(leaving.join().expect("joined").expect("an answer"), "ok: left · finishing");
}

#[test]
fn an_interface_that_vanishes_without_a_word_is_still_heard_leaving() {
    let path = socket("attach-vanish");
    let server = Server::start_at(path.clone()).expect("listen");

    let attaching = std::thread::spawn({
        let path = path.clone();
        move || ipc::attach_to(&path, false)
    });

    answer_for(&server, "ok: attached · 1 interface", Duration::from_secs(5));
    let attachment = attaching.join().expect("joined").expect("attached");
    let id = attachment.id().to_string();

    // What a killed interface does: the connection ends with nothing said on it.
    drop(attachment);

    let seen = answer_for(&server, "ok: left · no interfaces", Duration::from_secs(5));

    assert_eq!(
        seen,
        [Command::Leave { id, on_leave: OnLeave::Keep }],
        "gone, and asking for nothing: an interface that crashed did not ask for the music to stop"
    );
}

#[test]
fn a_daemon_another_interface_is_driving_is_not_one_interface_to_stop() {
    let mut daemon = Daemon::with_config(&Config::default());

    assert_eq!(daemon.apply(Command::Interfaces), "ok: 0 interfaces");

    let reply = daemon.apply(Command::Attach { id: "one".to_string(), spawned: true });
    assert_eq!(reply, "ok: attached · 1 interface");

    let reply = daemon.apply(Command::Attach { id: "two".to_string(), spawned: false });
    assert_eq!(reply, "ok: attached · 2 interfaces");

    // The interface that started the daemon closes, asking for it to go too. The other is still
    // listening to it, so it stays.
    let reply =
        daemon.apply(Command::Leave { id: "one".to_string(), on_leave: OnLeave::StopIfSpawned });
    assert_eq!(reply, "ok: left · 1 interface");
    assert!(daemon.is_running(), "the other interface is still driving it");

    // Not even a flat stop is acted on while somebody is attached.
    daemon.apply(Command::Attach { id: "three".to_string(), spawned: false });
    let reply = daemon.apply(Command::Leave { id: "two".to_string(), on_leave: OnLeave::Stop });
    assert_eq!(reply, "ok: left · 1 interface");
    assert!(daemon.is_running(), "closing one of two interfaces does not stop the music");
    assert_eq!(daemon.interfaces(), 1);

    // The last one out is the one it follows. An interface started this daemon, so it finishes.
    let reply =
        daemon.apply(Command::Leave { id: "three".to_string(), on_leave: OnLeave::StopIfSpawned });
    assert_eq!(reply, "ok: left · finishing");
    assert!(!daemon.is_running());
    assert_eq!(daemon.interfaces(), 0);
}

#[test]
fn the_last_interface_out_is_the_one_the_daemon_listens_to() {
    // Started by hand: the interfaces that came and went are not what it exists for.
    let mut daemon = Daemon::with_config(&Config::default());
    daemon.apply(Command::Attach { id: "one".to_string(), spawned: false });

    let reply =
        daemon.apply(Command::Leave { id: "one".to_string(), on_leave: OnLeave::StopIfSpawned });
    assert_eq!(reply, "ok: left · no interfaces");
    assert!(daemon.is_running(), "a daemon started by hand outlives the interfaces driving it");

    // Asked to be left alone, it is, whoever started it.
    let mut daemon = Daemon::with_config(&Config::default());
    daemon.apply(Command::Attach { id: "one".to_string(), spawned: true });
    assert_eq!(
        daemon.apply(Command::Leave { id: "one".to_string(), on_leave: OnLeave::Keep }),
        "ok: left · no interfaces"
    );
    assert!(daemon.is_running());

    // Asked to stop, it stops.
    let mut daemon = Daemon::with_config(&Config::default());
    daemon.apply(Command::Attach { id: "one".to_string(), spawned: false });
    assert_eq!(
        daemon.apply(Command::Leave { id: "one".to_string(), on_leave: OnLeave::Stop }),
        "ok: left · finishing"
    );
    assert!(!daemon.is_running());

    // And a leaving id nobody attached under changes nothing.
    let mut daemon = Daemon::with_config(&Config::default());
    daemon.apply(Command::Attach { id: "here".to_string(), spawned: true });
    let reply =
        daemon.apply(Command::Leave { id: "elsewhere".to_string(), on_leave: OnLeave::Stop });
    assert_eq!(reply, "ok: left · 1 interface");
    assert!(daemon.is_running(), "the interface that is here still needs it");
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


// ------------------------------------------------------------------ tracks played from the network

/// A daemon with no library behind it, for the commands that need none.
///
/// Built straight from a default config rather than through [`daemon_with_library`], which points
/// the environment at a scratch config: these tests run alongside that one, and two tests moving
/// `XDG_CONFIG_HOME` under each other is a race. yt-dlp is pointed at a program that does nothing,
/// so a stream the daemon decides to load asks the network for nothing.
fn bare_daemon() -> Daemon {
    // SAFETY: as for the other tests here — one variable, set to the same value by each of them.
    unsafe {
        std::env::set_var("OGMA_YTDLP", "true");
    }

    Daemon::with_config(&Config::default())
}

/// A YouTube result, as the interface would send one.
fn a_stream() -> ipc::StreamTrack {
    ipc::StreamTrack {
        url: "https://www.youtube.com/watch?v=aaaaaaaaaaa".to_string(),
        title: Some("First Song".to_string()),
        artist: Some("A Channel".to_string()),
        duration_ms: Some(131_000),
    }
}

#[test]
fn the_daemon_says_which_protocol_it_speaks() {
    let mut daemon = bare_daemon();

    let reply = daemon.apply(Command::Version);

    assert_eq!(reply, ipc::version_reply());
    assert_eq!(ipc::parse_version(&reply), Some(ipc::PROTOCOL_VERSION));

    // Asked for from a script, and by the hyphenated spelling a command line invites.
    assert_eq!(Command::parse("version"), Ok(Command::Version));
    assert_eq!(Command::parse("protocol"), Ok(Command::Version));
    assert_eq!(Command::Version.to_line(), "version");
}

#[test]
fn a_stream_survives_the_trip_over_the_socket() {
    let command = Command::PlayStream(a_stream());
    let line = command.to_line();

    assert_eq!(Command::parse(&line), Ok(command), "what is written reads back the same: {line}");

    // A script has only the URL to give, and that is enough.
    assert_eq!(
        Command::parse("queue_stream https://www.youtube.com/watch?v=bbbbbbbbbbb"),
        Ok(Command::QueueStream(ipc::StreamTrack {
            url: "https://www.youtube.com/watch?v=bbbbbbbbbbb".to_string(),
            ..ipc::StreamTrack::default()
        }))
    );

    assert!(Command::parse("play_stream").is_err(), "a stream needs something to play");
}

#[test]
fn the_player_queues_and_plays_what_comes_from_the_network() {
    let mut daemon = bare_daemon();

    let reply = daemon.apply(Command::QueueStream(a_stream()));
    assert_eq!(reply, "ok: queued First Song");

    let queued = &daemon.player().queue()[0];
    assert!(queued.is_stream(), "it plays from the network rather than from disk");
    assert_eq!(queued.title(), Some("First Song"));
    assert_eq!(queued.duration(), Some(Duration::from_secs(131)));

    let reply = daemon.apply(Command::PlayStream(a_stream()));
    assert!(reply.contains("First Song"), "{reply}");

    // A URL is not a file that has gone missing.
    let reply = daemon.apply(Command::PlayFile(PathBuf::from(
        "https://www.youtube.com/watch?v=ccccccccccc",
    )));
    assert!(!reply.contains("no such file"), "{reply}");
}

#[test]
fn the_status_says_what_the_streams_in_the_queue_are_called() {
    let mut daemon = bare_daemon();

    daemon.apply(Command::QueueAdd(PathBuf::from("/music/not-really-there.flac")));
    daemon.apply(Command::QueueStream(a_stream()));

    let status = daemon.status();

    assert_eq!(status.streams.len(), 1, "only the stream needs naming; the file has its own tags");
    assert_eq!(status.streams[0].title.as_deref(), Some("First Song"));
    assert_eq!(status.streams[0].url, a_stream().url);

    // And it reads back out of the JSON the interface receives.
    let sent = Status::parse(&status.to_json()).expect("a status");
    assert_eq!(sent.streams, status.streams);
}

#[test]
fn a_queue_sent_back_as_bare_urls_keeps_its_titles() {
    let mut daemon = bare_daemon();

    daemon.apply(Command::QueueStream(a_stream()));

    // What an interface does when it reorders the queue: the whole of it, as paths, with no room
    // for a title. The daemon remembers what it was told.
    daemon.apply(Command::SetQueue(vec![PathBuf::from(a_stream().url)]));

    let queued = &daemon.player().queue()[0];
    assert!(queued.is_stream());
    assert_eq!(queued.title(), Some("First Song"), "the name did not go with the round trip");
}
