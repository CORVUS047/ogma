//! The version check an interface makes before it trusts a daemon.
//!
//! A test binary of its own: the check asks whatever is listening on the usual socket, so these
//! move `$OGMA_SOCKET` at their own pace without disturbing the tests that drive a daemon directly.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ogma::daemon::{self, Handshake};
use ogma::ipc::{self, Server};

/// A short, unique socket path, removed if something left one behind.
fn socket() -> PathBuf {
    let path = PathBuf::from(format!("/tmp/ogma-hs-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);

    path
}

/// Something listening on the socket that answers everything with one line.
///
/// Stands in for a daemon of whatever version the line says, which is the only thing about a real
/// one that this check looks at.
struct Fake {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Fake {
    fn answering(path: PathBuf, reply: impl Into<String>) -> Self {
        let reply = reply.into();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);

        let thread = std::thread::spawn(move || {
            let server = Server::start_at(path).expect("listen");

            while !flag.load(Ordering::Relaxed) {
                for request in server.pending() {
                    request.answer(reply.clone());
                }

                std::thread::sleep(Duration::from_millis(2));
            }
        });

        // Not ready until it answers, or the first check would see an empty socket.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !daemon::is_running() {
            std::thread::sleep(Duration::from_millis(2));
        }

        Fake { stop, thread: Some(thread) }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn the_interface_asks_what_the_daemon_speaks_and_says_when_it_differs() {
    let path = socket();

    // SAFETY: one variable, set once, before this binary's only test talks to a socket.
    unsafe {
        std::env::set_var("OGMA_SOCKET", &path);
    }

    // --- nobody there ---
    assert_eq!(daemon::handshake(), Handshake::Gone, "there is nothing yet to disagree with");

    // --- a daemon from this build ---
    {
        // Whatever this build speaks, said the way a daemon of the same build would say it.
        let _daemon = Fake::answering(path.clone(), ipc::version_reply());

        assert_eq!(
            daemon::handshake(),
            Handshake::Agreed,
            "the version this build speaks is {}",
            ipc::PROTOCOL_VERSION
        );
    }

    // --- a daemon from some other build ---
    {
        let _daemon = Fake::answering(path.clone(), "ok: ipc 99");

        assert_eq!(
            daemon::handshake(),
            Handshake::Mismatch { ours: ipc::PROTOCOL_VERSION, theirs: Some(99) }
        );
    }

    // --- a daemon too old to know the question ---
    {
        let _daemon = Fake::answering(path, "error: unknown command \"version\"");

        assert_eq!(
            daemon::handshake(),
            Handshake::Mismatch { ours: ipc::PROTOCOL_VERSION, theirs: None },
            "not knowing how to answer is itself an answer"
        );
    }
}

#[test]
fn a_version_reads_back_out_of_what_the_daemon_says() {
    assert_eq!(ipc::parse_version(&ipc::version_reply()), Some(ipc::PROTOCOL_VERSION));
    assert_eq!(ipc::parse_version("ok: ipc 7"), Some(7));
    assert_eq!(ipc::parse_version("  ok: ipc 7  "), Some(7));

    // Anything else is a daemon that cannot say, which is not the same as one that said zero.
    assert_eq!(ipc::parse_version("error: unknown command \"version\""), None);
    assert_eq!(ipc::parse_version("ok: ipc"), None);
    assert_eq!(ipc::parse_version("ok: 1"), None);
    assert_eq!(ipc::parse_version(""), None);
}
