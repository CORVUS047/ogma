//! Send a command to a running player.
//!
//! ```text
//! $ ogma-cmd pause
//! ok: paused
//! $ ogma-cmd volume -10
//! ok: volume 45% (-14.0 dB)
//! $ ogma-cmd load_playlist "Late Night"
//! ok: queued 24 from Late Night by title
//! ```
//!
//! Anything the player reports is printed as it came; a command it could not carry out exits
//! non-zero, so this can be used from a keybinding or a script without guessing.

use std::process::ExitCode;

use ogma::ipc::{self, Command};

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    if arguments.is_empty() || matches!(arguments[0].as_str(), "-h" | "--help" | "help") {
        print_usage();

        // Asking for help is not a failure; being given nothing to do is.
        return if arguments.is_empty() { ExitCode::FAILURE } else { ExitCode::SUCCESS };
    }

    // The arguments are joined so that a name needs no quoting: `ogma-cmd load_playlist Late Night`
    // works as well as the quoted form.
    let command = match Command::parse(&arguments.join(" ")) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("ogma-cmd: {err}");
            print_usage();

            return ExitCode::FAILURE;
        }
    };

    match ipc::send(&command) {
        Ok(reply) => {
            println!("{reply}");

            // The player says whether it managed; passing that on lets a script tell.
            if reply.starts_with("error") {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(err) => {
            eprintln!("ogma-cmd: {err}");

            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    eprintln!("usage: ogma-cmd <command> [name]\n");

    for (name, what) in Command::usage() {
        eprintln!("  {name:<24} {what}");
    }

    eprintln!("\nthe player is found at {}", ipc::socket_path().display());
    eprintln!("set OGMA_SOCKET to reach a player listening elsewhere");
}
