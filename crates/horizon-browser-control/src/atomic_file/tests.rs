use std::cell::Cell;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use super::{
    StagedFile, create_new, is_staged_name, publication_in_flight, read, read_dir, replace, retake_while_torn,
    retry_while_blocked,
};

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

/// Before #847 the Windows replace used `MoveFileExW`, which fails with
/// access denied whenever any handle has the destination open.
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
fn only_staging_names_mark_a_publication_in_flight() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");
    replace(&path, b"{}").unwrap();
    std::fs::write(root.path().join(".notes.tmp"), b"").unwrap();
    std::fs::write(root.path().join(format!(".{}.tmp", "g".repeat(32))), b"").unwrap();
    assert!(!publication_in_flight(&path));

    let staged = StagedFile::write(&path, b"next").unwrap();
    assert!(is_staged_name(staged.path.file_name().unwrap()));
    assert!(publication_in_flight(&path));
    drop(staged);
    assert!(!publication_in_flight(&path));
}

#[test]
fn abandoned_staging_does_not_mark_a_publication_in_flight() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");
    let orphan = std::fs::File::create(root.path().join(format!(".{}.tmp", "b".repeat(32)))).unwrap();
    orphan
        .set_modified(std::time::SystemTime::now() - super::ABANDONED_STAGING_AGE * 2)
        .unwrap();
    drop(orphan);

    assert!(!publication_in_flight(&path));
}

#[test]
fn read_reports_an_absent_file_at_once() {
    let root = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();

    let error = read(&root.path().join("panel.json")).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "no publication was in flight"
    );
}

/// Windows can leave the destination missing while a replacing rename is
/// in flight; the staged sibling is still there, so the read waits.
#[cfg(windows)]
#[test]
fn read_waits_out_a_publication_that_is_mid_rename() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");
    let staged = root.path().join(format!(".{}.tmp", "a".repeat(32)));
    std::fs::write(&staged, b"new").unwrap();
    let publish = {
        let path = path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            std::fs::rename(staged, path).unwrap();
        })
    };

    assert_eq!(read(&path).unwrap(), b"new");
    publish.join().unwrap();
}

/// Unix renames are atomic for readers, so a missing file is genuine even
/// while a sibling is being published.
#[cfg(not(windows))]
#[test]
fn read_does_not_wait_for_publications_on_unix() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");
    let _staged = StagedFile::write(&path, b"next").unwrap();
    let started = std::time::Instant::now();

    assert_eq!(read(&path).unwrap_err().kind(), io::ErrorKind::NotFound);
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn listings_are_retaken_until_they_are_whole() {
    let takes = Cell::new(0);
    let listing = retake_while_torn(
        || {
            takes.set(takes.get() + 1);
            Ok(takes.get())
        },
        |take| *take < 3,
        Duration::from_secs(5),
    )
    .unwrap();

    assert_eq!(listing, 3);
}

#[test]
fn a_torn_listing_is_returned_after_the_window() {
    let takes = Cell::new(0);
    let listing = retake_while_torn(
        || {
            takes.set(takes.get() + 1);
            Ok(takes.get())
        },
        |_| true,
        Duration::from_millis(20),
    )
    .unwrap();

    assert!(listing > 1, "a torn listing is retaken at least once");
}

#[test]
fn read_dir_leaves_staging_out() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");
    replace(&path, b"{}").unwrap();
    let orphan = std::fs::File::create(root.path().join(format!(".{}.tmp", "c".repeat(32)))).unwrap();
    orphan
        .set_modified(std::time::SystemTime::now() - super::ABANDONED_STAGING_AGE * 2)
        .unwrap();
    drop(orphan);

    let names: Vec<_> = read_dir(root.path())
        .unwrap()
        .into_iter()
        .map(|entry| entry.file_name())
        .collect();

    assert_eq!(names, ["panel.json"]);
}

/// A listing taken mid-rename on Windows shows only the staging; the retaken
/// listing names the published file.
#[cfg(windows)]
#[test]
fn read_dir_waits_out_a_publication_that_is_mid_rename() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");
    let staged = root.path().join(format!(".{}.tmp", "d".repeat(32)));
    std::fs::write(&staged, b"new").unwrap();
    let publish = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        std::fs::rename(staged, path).unwrap();
    });

    let names: Vec<_> = read_dir(root.path())
        .unwrap()
        .into_iter()
        .map(|entry| entry.file_name())
        .collect();

    assert_eq!(names, ["panel.json"]);
    publish.join().unwrap();
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

#[test]
fn blocked_publication_retries_until_it_succeeds() {
    let attempts = Cell::new(0);
    let result = retry_while_blocked(
        || {
            attempts.set(attempts.get() + 1);
            if attempts.get() < 3 {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                Ok(())
            }
        },
        |error| error.kind() == io::ErrorKind::PermissionDenied,
        Duration::from_secs(5),
    );

    result.unwrap();
    assert_eq!(attempts.get(), 3);
}

#[test]
fn blocked_publication_gives_up_after_the_window() {
    let window = Duration::from_millis(20);
    let started = std::time::Instant::now();
    let error = retry_while_blocked::<()>(
        || Err(io::Error::from(io::ErrorKind::PermissionDenied)),
        |error| error.kind() == io::ErrorKind::PermissionDenied,
        window,
    )
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(
        started.elapsed() >= window,
        "a blocked publication waits out its retry window"
    );
}

#[test]
fn unrelated_publication_errors_are_not_retried() {
    let attempts = Cell::new(0);
    let error = retry_while_blocked::<()>(
        || {
            attempts.set(attempts.get() + 1);
            Err(io::Error::from(io::ErrorKind::NotFound))
        },
        |error| error.kind() == io::ErrorKind::PermissionDenied,
        Duration::from_secs(5),
    )
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert_eq!(attempts.get(), 1);
}

/// A reader that denies delete sharing blocks even a POSIX-semantics rename;
/// the replace waits for it instead of failing.
#[cfg(windows)]
#[test]
fn replace_waits_for_a_reader_that_denies_delete_sharing() {
    use std::os::windows::fs::OpenOptionsExt as _;

    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panel.json");
    replace(&path, b"old").unwrap();
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&path)
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        drop(reader);
    });

    replace(&path, b"new").unwrap();

    release.join().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"new");
    assert_eq!(entry_names(root.path()), ["panel.json"]);
}

#[cfg(windows)]
#[test]
fn only_sharing_failures_count_as_blocked_on_windows() {
    use super::is_blocked_by_open_handle;

    assert!(is_blocked_by_open_handle(&io::Error::from_raw_os_error(5)));
    assert!(is_blocked_by_open_handle(&io::Error::from_raw_os_error(32)));
    assert!(is_blocked_by_open_handle(&io::Error::from_raw_os_error(33)));
    assert!(!is_blocked_by_open_handle(&io::Error::from_raw_os_error(2)));
    assert!(!is_blocked_by_open_handle(&io::Error::from(
        io::ErrorKind::PermissionDenied
    )));
}
