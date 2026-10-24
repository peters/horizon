use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};

use super::{StagedFile, create_new, read, read_dir, replace};

fn entry_names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn replace_creates_then_overwrites_without_leaving_staged_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");

    replace(&path, b"first").unwrap();
    replace(&path, b"second").unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), b"second");
    assert_eq!(entry_names(root.path()), ["panel.json"]);
}

#[cfg(unix)]
#[test]
fn replace_publishes_owner_only_files() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");
    replace(&path, b"{}").unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode & 0o077, 0, "group and other bits must stay clear, got {mode:o}");
}

#[test]
fn replace_reports_a_missing_directory_and_cleans_up() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("missing").join("panel.json");

    let error = replace(&path, b"{}").unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(entry_names(root.path()).is_empty());
}

/// An open reader keeps the file it opened and never blocks the replace.
#[test]
fn replace_succeeds_while_a_reader_holds_the_destination_open() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");
    replace(&path, b"old").unwrap();
    let mut reader = std::fs::File::open(&path).unwrap();

    replace(&path, b"new").unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), b"new");
    let mut held = Vec::new();
    io::Read::read_to_end(&mut reader, &mut held).unwrap();
    assert_eq!(held, b"old", "an open reader keeps the file it opened");
    drop(reader);
    assert_eq!(entry_names(root.path()), ["panel.json"]);
}

#[test]
fn concurrent_readers_never_fail_a_replace_or_observe_a_partial_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");
    let first = vec![b'a'; 64 * 1024];
    let second = vec![b'b'; 64 * 1024];
    replace(&path, &first).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let reader_count = 4;
    let polling = Arc::new(Barrier::new(reader_count + 1));
    let readers: Vec<_> = (0..reader_count)
        .map(|_| {
            let path = path.clone();
            let stop = Arc::clone(&stop);
            let polling = Arc::clone(&polling);
            let (first, second) = (first.clone(), second.clone());
            std::thread::spawn(move || {
                let mut reads = 0_u32;
                loop {
                    let contents = read(&path).unwrap();
                    assert!(contents == first || contents == second, "observed a partial file");
                    reads += 1;
                    if reads == 1 {
                        polling.wait();
                    }
                    if stop.load(Ordering::Relaxed) {
                        return reads;
                    }
                }
            })
        })
        .collect();
    polling.wait();

    for round in 0..200 {
        let contents = if round % 2 == 0 { &second } else { &first };
        replace(&path, contents).unwrap();
    }
    stop.store(true, Ordering::Relaxed);

    for reader in readers {
        assert!(reader.join().unwrap() > 0);
    }
    assert_eq!(entry_names(root.path()), ["panel.json"]);
}

#[test]
fn read_reports_an_absent_file_at_once() {
    let root = tempfile::tempdir().unwrap();

    let error = read(&root.path().join("panel.json")).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

/// A rename publishes without waiting for readers, so a missing destination
/// is genuine even while a staged sibling exists.
#[test]
fn read_does_not_wait_for_a_staged_sibling() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");
    let staged = StagedFile::write(&path, b"next").unwrap();

    assert_eq!(read(&path).unwrap_err().kind(), io::ErrorKind::NotFound);
    drop(staged);
}

#[test]
fn read_dir_leaves_staging_out() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");
    replace(&path, b"{}").unwrap();
    std::fs::write(root.path().join(format!(".{}.tmp", "c".repeat(32))), b"").unwrap();

    let names: Vec<_> = read_dir(root.path())
        .unwrap()
        .into_iter()
        .map(|entry| entry.file_name())
        .collect();

    assert_eq!(names, ["panel.json"]);
}

#[test]
fn replace_accepts_a_destination_leaf_near_the_name_limit() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(format!("{}.json", "p".repeat(245)));

    replace(&path, b"first").unwrap();
    replace(&path, b"second").unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), b"second");
    assert_eq!(entry_names(root.path()).len(), 1);
}

#[test]
fn create_new_refuses_an_existing_file_and_keeps_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("step-0.json");

    create_new(&path, b"first").unwrap();
    let error = create_new(&path, b"second").unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&path).unwrap(), b"first");
    assert_eq!(entry_names(root.path()), ["step-0.json"]);
}

