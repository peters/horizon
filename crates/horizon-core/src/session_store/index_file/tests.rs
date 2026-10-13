use std::sync::{Arc, Barrier};

use super::*;

#[test]
fn updaters_that_share_only_the_file_lock_keep_every_change() {
    const CHANGES: usize = 40;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("sessions").join("index.yaml");
    let start = Arc::new(Barrier::new(2));
    // Each thread takes the file lock through its own handle and its own INDEX_UPDATE, as
    // two Horizon processes do.
    let updaters: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|updater| {
            let (path, start) = (path.clone(), Arc::clone(&start));
            std::thread::spawn(move || {
                let threads = Mutex::new(());
                let index = IndexFile {
                    path: &path,
                    threads: &threads,
                };
                start.wait();
                for change in 0..CHANGES {
                    let profile = format!("{updater}-{change}");
                    let written = index
                        .change(|index| index.touch_profile_session(&profile, "session"))
                        .unwrap();
                    assert!(written.names(&profile, "session"), "the index on disk has the change");
                }
            })
        })
        .collect();
    for updater in updaters {
        updater.join().unwrap();
    }
    let index = IndexFile::at(&path).load().unwrap();
    for profile in ["a", "b"]
        .iter()
        .flat_map(|updater| (0..CHANGES).map(move |change| format!("{updater}-{change}")))
    {
        assert!(index.names(&profile, "session"), "the change of {profile} is kept");
    }
    let mut left: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["index.lock", "index.yaml"], "no temporary file is left");
}
