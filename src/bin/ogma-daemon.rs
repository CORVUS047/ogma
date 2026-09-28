//! The part of ogma that makes sound.
//!
//! Holds the queue, what has played, and what is playing, and drives the audio device. Everything
//! else — browsing, playlists, tags, the interface — belongs to whoever is driving it, over the
//! socket in `ogma::ipc`.
//!
//! ```text
//! $ ogma-daemon &
//! ogma-daemon: listening on /run/user/1000/ogma.sock
//! $ ogma-cmd play_pause
//! ok: playing Xtal
//! ```

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut daemon = ogma::daemon::Daemon::new();

    match daemon.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // Almost always another daemon already holding the socket, which is not a failure worth a
            // backtrace: one daemon is what is wanted.
            eprintln!("ogma-daemon: {err}");

            ExitCode::FAILURE
        }
    }
}
