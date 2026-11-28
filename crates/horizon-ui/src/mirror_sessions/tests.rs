//! The mode of the publisher of the session mirror: its home, its join, and the
//! frames that it sends.
//!
//! The stores of the tests are made up, and the home of the person is a temporary
//! folder: this mode must write nothing outside its own home.

use std::path::{Path, PathBuf};

use horizon_core::session_mirror::{MirrorProgram, read_list};
use horizon_sync::MirrorList;

use super::home::MirrorHome;
use super::{MirrorMode, device_name, run_in};

/// One home for a test: the home of the person, and the home of the publisher.
fn homes() -> (tempfile::TempDir, MirrorHome) {
    let directory = tempfile::tempdir().expect("a directory");
    let home = MirrorHome::at(directory.path().join("mirror"));
    home.prepare().expect("the home of the publisher");
    (directory, home)
}

/// The paths of every file under `root`, with the time of its change.
fn tree(root: &Path) -> Vec<(PathBuf, u128)> {
    let mut files = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                folders.push(path.clone());
            }
            let time = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_nanos());
            files.push((path, time));
        }
    }
    files.sort();
    files
}

/// One Claude transcript in `home`, so the list of the mirror holds a session.
fn transcript(home: &Path, id: &str) {
    let folder = home.join(".claude/projects/-repo");
    std::fs::create_dir_all(&folder).expect("the folder of the project");
    let line = serde_json::json!({
        "type": "user",
        "cwd": "/repo",
        "message": {"role": "user", "content": "made-up request"},
    });
    std::fs::write(folder.join(format!("{id}.jsonl")), format!("{line}\n")).expect("the transcript");
    let registry = home.join(".claude/sessions");
    std::fs::create_dir_all(&registry).expect("the folder of the live registry");
}

#[test]
fn a_start_without_a_join_writes_nothing_outside_its_home() {
    let (directory, home) = homes();
    let person = directory.path().join("person");
    transcript(&person, "00000000-0000-0000-0000-000000000001");
    let before = tree(directory.path());

    let code = run_in(&home, &MirrorMode::with_join(None));

    assert_ne!(code, 0, "a publisher that is not joined does not start");
    let after = tree(directory.path());
    let changed: Vec<&PathBuf> = after
        .iter()
        .map(|(path, _)| path)
        .filter(|path| !before.iter().any(|(held, _)| held == *path))
        .collect();
    assert!(changed.is_empty(), "the mode wrote no file of its own: {changed:?}");
    assert!(home.root().is_dir(), "the home of the publisher is there");
}

#[test]
fn a_join_of_a_text_that_is_no_link_writes_nothing_and_ends_with_a_code() {
    let (directory, home) = homes();
    let before = tree(directory.path());

    let code = run_in(&home, &MirrorMode::with_join(Some("not a link".to_owned())));

    assert_ne!(code, 0);
    assert_eq!(tree(directory.path()), before, "a refused link writes no file");
    assert_eq!(home.device(), None, "a refused link keeps no name");
    assert!(home.address().is_none(), "a refused link keeps no address");
}

#[test]
fn the_list_of_a_home_holds_the_session_that_its_store_holds() {
    let (directory, _home) = homes();
    let person = directory.path().join("person");
    transcript(&person, "00000000-0000-0000-0000-000000000002");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock")
        .as_millis();
    let catalog = read_list(&person, i64::try_from(now).expect("a time"), &|_| false);

    let list = MirrorList {
        sessions: catalog.rows().iter().map(|row| row.session.clone()).collect(),
        unreadable: catalog.unreadable().to_vec(),
    };
    assert_eq!(list.sessions.len(), 1);
    assert_eq!(list.sessions[0].program, MirrorProgram::Claude);
    assert_eq!(list.sessions[0].folder, "repo");
    assert_eq!(list.unreadable, [MirrorProgram::Antigravity]);
}

#[test]
fn the_name_of_a_publisher_is_a_device_name_of_its_server() {
    let name = device_name("build-machine");
    assert_eq!(name, "build-machine-sessions");
    assert!(horizon_sync::check_device_name(&name), "{name}");
}
