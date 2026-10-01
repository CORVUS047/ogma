//! Taking turns over a file, so two players never write to one at once.

mod common;

use std::time::{Duration, SystemTime};

use ogma::claim::Claim;

/// A file to claim. Nothing opens it; a claim is about a path.
fn target(name: &str) -> std::path::PathBuf {
    let dir = common::scratch_dir(name);
    let path = dir.join("track.wav");
    std::fs::write(&path, b"not really audio").expect("write target");

    path
}

#[test]
fn a_claimed_file_cannot_be_claimed_again() {
    let path = target("claim-held");

    let held = Claim::on(&path).expect("the first claim is granted");

    assert!(Claim::on(&path).is_none(), "a second player is turned away");

    drop(held);

    assert!(Claim::on(&path).is_some(), "and let in once the first is done");
}

#[test]
fn claims_on_different_files_do_not_block_each_other() {
    let dir = common::scratch_dir("claim-separate");
    let one = dir.join("one.wav");
    let two = dir.join("two.wav");
    std::fs::write(&one, b"one").expect("write one");
    std::fs::write(&two, b"two").expect("write two");

    let _first = Claim::on(&one).expect("claim one");

    assert!(Claim::on(&two).is_some(), "another file is nobody's business");
}

#[test]
fn one_file_reached_by_two_paths_is_one_claim() {
    let path = target("claim-canonical");
    let folder = path.parent().expect("folder");

    // The same file, named the long way round.
    let roundabout = folder.join(".").join("track.wav");

    let _held = Claim::on(&path).expect("claim");

    assert!(Claim::on(&roundabout).is_none(), "the same file, however it is spelt");
}

#[test]
fn a_claim_left_behind_by_a_dead_player_is_stolen() {
    let path = target("claim-stale");

    let held = Claim::on(&path).expect("claim");
    let lock = held.file().expect("a claim that protects something").to_path_buf();

    // What a player that died mid-write leaves: a claim file nobody will ever remove. Backdating it
    // is how the test reaches the age at which that is assumed, without waiting five minutes.
    let long_ago = SystemTime::now() - Duration::from_secs(600);
    let file = std::fs::File::options().write(true).open(&lock).expect("open claim");
    file.set_times(std::fs::FileTimes::new().set_modified(long_ago)).expect("backdate claim");

    let stolen = Claim::on(&path).expect("a stale claim is taken over");

    // The player that was presumed dead letting go must not take the new holder's claim with it: it
    // is the new holder who is writing to the file now.
    drop(held);

    assert!(lock.exists(), "the claim stays with whoever holds it now");
    assert!(Claim::on(&path).is_none(), "so nobody else gets in meanwhile");

    drop(stolen);

    assert!(!lock.exists(), "and it is given up when the holder is done");
}

#[test]
fn waiting_for_a_claim_gives_up_rather_than_hanging() {
    let path = target("claim-wait");

    let _held = Claim::on(&path).expect("claim");

    let started = std::time::Instant::now();
    let waited = Claim::waited_for(&path);
    let spent = started.elapsed();

    assert!(waited.is_none(), "the file was never free");
    assert!(spent >= Duration::from_millis(500), "it did wait: {spent:?}");
    assert!(spent < Duration::from_secs(10), "but not forever: {spent:?}");
}

#[test]
fn waiting_gets_the_claim_once_the_holder_is_done() {
    let path = target("claim-wait-ok");

    let held = Claim::on(&path).expect("claim");

    let letting_go = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(held);
    });

    assert!(Claim::waited_for(&path).is_some(), "the wait ends with the claim");

    letting_go.join().expect("the holder thread");
}
