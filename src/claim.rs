//! Claiming a music file before writing to it, so that two players never write to one at once.
//!
//! Several interfaces can be open on one library, and each of them fills in missing metadata on a
//! thread of its own ([`crate::autofill`]) as well as writing genres the user types ([`crate::genre`]).
//! Both rewrite a file whole — read it, put the tag in, write it back — so two of them on one path
//! at the same moment is how a track ends up truncated or carrying half a tag. A claim is what makes
//! them take turns.
//!
//! A claim is a file in the runtime directory, created with `create_new` so that the filesystem
//! itself settles who got there first; dropping the claim removes it. The runtime directory is
//! cleared on reboot, like the socket's, so nothing here outlives a session, and a claim left behind
//! by a player that died is stolen once it is [`STALE_AFTER`] old.
//!
//! Claims are advisory and bind this player only: anything else writing to the same files — a tag
//! editor, a sync tool — knows nothing about them. What they are for is the case this player makes
//! for itself by letting several of its own interfaces run at once.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Where claims are kept, under the runtime directory.
const FOLDER_NAME: &str = "ogma-claims";

/// A claim nobody has let go of for this long is taken to be left over from a player that died
/// mid-write, and is stolen.
///
/// Comfortably longer than one file can honestly take: the services are given fifteen seconds each
/// and MusicBrainz is held to one request a second, so the slowest lookup chain is a couple of
/// minutes.
const STALE_AFTER: Duration = Duration::from_secs(300);

/// How long [`Claim::waited_for`] keeps trying before giving up.
const WAIT_FOR: Duration = Duration::from_secs(2);

/// How often it tries again in the meantime.
const WAIT_BETWEEN: Duration = Duration::from_millis(50);

/// A claim on one file, held while it is being written and given up when dropped.
///
/// Hold it for the whole of a read-decide-write: claiming only around the write itself would let the
/// other player read the file's tags before this one has finished changing them, and then write back
/// what it read.
#[derive(Debug)]
pub struct Claim {
    /// The claim file to remove, or `None` when claims cannot be kept at all and the write went
    /// ahead unprotected.
    lock: Option<PathBuf>,
    /// What was written into the claim file, so that dropping this removes only a claim still held
    /// by it. A claim of ours that was stolen as stale is somebody else's now, and taking their file
    /// away on the way out would let a third player in while they write.
    token: String,
}

/// What came of trying to create a claim file.
enum Taken {
    /// It is ours; nobody else was holding it.
    Ours,
    /// Somebody else is holding it.
    Theirs,
    /// Claims cannot be kept here at all.
    Impossible,
}

impl Claim {
    /// Claim `target`, or `None` when another player is already dealing with it.
    ///
    /// A machine where claims cannot be kept — nowhere writable to put them — answers with a claim
    /// that protects nothing rather than with `None`: filling in a cover is worth more than the
    /// protection, and a player running on its own needs none.
    pub fn on(target: &Path) -> Option<Claim> {
        let lock = path_for(target);

        let Some(folder) = lock.parent() else {
            return Some(Claim { lock: None, token: token() });
        };

        if std::fs::create_dir_all(folder).is_err() {
            return Some(Claim { lock: None, token: token() });
        }

        let token = token();

        match take(&lock, &token) {
            Taken::Ours => Some(Claim { lock: Some(lock), token }),
            Taken::Impossible => Some(Claim { lock: None, token }),
            // A claim old enough to be left over from a player that is no longer running is taken
            // over. Two players stealing it at the same moment is settled the way any other race
            // here is: one of them creates the file, the other finds it already there.
            Taken::Theirs if is_stale(&lock) => {
                let _ = std::fs::remove_file(&lock);

                match take(&lock, &token) {
                    Taken::Ours => Some(Claim { lock: Some(lock), token }),
                    Taken::Impossible => Some(Claim { lock: None, token }),
                    Taken::Theirs => None,
                }
            }
            Taken::Theirs => None,
        }
    }

    /// [`Claim::on`], keeping at it for a moment before giving up.
    ///
    /// For a write the user asked for by hand: whatever holds the file is writing one track, which
    /// takes milliseconds, so waiting is nearly always better than telling them it could not be done.
    pub fn waited_for(target: &Path) -> Option<Claim> {
        let deadline = Instant::now() + WAIT_FOR;

        loop {
            if let Some(claim) = Claim::on(target) {
                return Some(claim);
            }

            if Instant::now() >= deadline {
                return None;
            }

            std::thread::sleep(WAIT_BETWEEN);
        }
    }

    /// The claim file, or `None` when the claim protects nothing.
    ///
    /// For looking at what is held; nothing needs this to take a claim.
    pub fn file(&self) -> Option<&Path> {
        self.lock.as_deref()
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let Some(lock) = &self.lock else {
            return;
        };

        // Only a claim this one still holds is given up. A claim that was taken from it as stale now
        // belongs to whoever is writing the file, and removing their file would let a third player
        // start writing while they do.
        let held = std::fs::read_to_string(lock).map(|held| held.trim() == self.token);

        if held.is_ok_and(|held| held) {
            // A claim that cannot be removed is one the next player steals as stale, which is the
            // same end by a slower road.
            let _ = std::fs::remove_file(lock);
        }
    }
}

/// Create the claim file, which is what decides who holds it.
fn take(lock: &Path, token: &str) -> Taken {
    match OpenOptions::new().create_new(true).write(true).open(lock) {
        Ok(mut file) => {
            // Read back only by this claim's own `Drop`, to tell a claim it still holds from one that
            // has since been stolen. It carries the process id so that a person looking in the folder
            // can see who is writing.
            let _ = writeln!(file, "{token}");

            Taken::Ours
        }
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Taken::Theirs,
        Err(_) => Taken::Impossible,
    }
}

/// What goes in a claim file: this process, and which claim of its own this is.
///
/// Two claims in one process must not look alike either — a run that fills a file while the user
/// types a genre into another holds two at once.
fn token() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);

    format!("{} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Whether a claim is old enough to have been left behind rather than held.
fn is_stale(lock: &Path) -> bool {
    let Ok(modified) = std::fs::metadata(lock).and_then(|meta| meta.modified()) else {
        // A claim whose age cannot be read is left alone: a file that waits is better than two
        // players writing to it at once.
        return false;
    };

    modified.elapsed().is_ok_and(|age| age > STALE_AFTER)
}

/// The claim file for `target`.
fn path_for(target: &Path) -> PathBuf {
    // One file reached by two different paths — a relative one, a symlinked library — must come to
    // one claim, which is what canonicalising is for. A path that cannot be canonicalised is taken
    // as it is; it is about to be opened anyway.
    let full = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());

    let name = full.file_name().and_then(|name| name.to_str()).unwrap_or("file");

    // The name is there to make the folder readable by a person; the fingerprint is what makes the
    // claim unique.
    folder().join(format!(
        "{:016x}-{}.claim",
        fingerprint(full.as_os_str().as_encoded_bytes()),
        tidy(name)
    ))
}

/// Where claims live.
///
/// The runtime directory, as the socket does, so that a reboot clears them; failing that a name in
/// the temporary directory carrying the user id, so two people on one machine do not collide.
fn folder() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join(FOLDER_NAME);
    }

    std::env::temp_dir().join(format!("{FOLDER_NAME}-{}", crate::ipc::users_id()))
}

/// FNV-1a, so that every build of this player works out the same claim for the same file.
///
/// The standard library's hasher would do the job, but it does not promise the same answer from one
/// version of Rust to the next, and two players that disagree about a file's claim do not take turns.
fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;

    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }

    hash
}

/// A file name as a claim's name can carry it: plain ASCII, and short enough to leave room.
fn tidy(name: &str) -> String {
    name.chars()
        .map(|c| match c.is_ascii_alphanumeric() || c == '.' || c == '-' {
            true => c,
            false => '_',
        })
        .take(48)
        .collect()
}
