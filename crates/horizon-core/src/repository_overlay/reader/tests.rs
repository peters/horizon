use super::{RepositoryReadError as Error, SelectedRepositoryNode as Node, *};
use std::{
    fs,
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    },
};

#[test]
fn reads_exact_binary_nested_unicode_empty_bytes_and_owner_executable_mode() {
    let directory = tempfile::tempdir().expect("fixture");
    fs::create_dir(directory.path().join("nested")).expect("directory");
    let path = directory.path().join("nested/æøå");
    fs::write(&path, [0, 255, 10, 128]).expect("bytes");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o740)).expect("mode");
    fs::write(directory.path().join("empty"), []).expect("empty");
    let reader = SelectedRepositoryReader::open(directory.path()).expect("root");
    assert_eq!(
        reader.read("nested/æøå", 4),
        Ok(Node::File {
            bytes: vec![0, 255, 10, 128],
            executable: true
        })
    );
    assert_eq!(
        reader.read("empty", 0),
        Ok(Node::File {
            bytes: vec![],
            executable: false
        })
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o611)).expect("group/other executable");
    assert_eq!(
        reader.read("nested/æøå", 4),
        Ok(Node::File {
            bytes: vec![0, 255, 10, 128],
            executable: false
        })
    );
    assert_eq!(fs::read(&path).expect("unchanged bytes"), [0, 255, 10, 128]);
}

#[test]
fn path_policy_runs_before_any_node_read_and_errors_are_redacted() {
    let directory = tempfile::tempdir().expect("fixture");
    let reader = SelectedRepositoryReader::open(directory.path()).expect("root");
    for path in [
        "../outside",
        "/outside",
        "C:/outside",
        "a\\b",
        "a//b",
        ".git/config",
        ".env",
        "target/cache",
        "keys/id_rsa",
    ] {
        let result = reader.read(path, 100);
        assert!(matches!(result, Err(Error::Policy(_))));
        assert!(!format!("{result:?}").contains(path));
    }
    assert_eq!(reader.read("private-missing", 100), Err(Error::Missing));
    assert!(!format!("{:?}", reader.read("private-missing", 100)).contains("private-missing"));
}

#[test]
fn limits_cover_declared_and_actual_payloads_without_truncation() {
    let directory = tempfile::tempdir().expect("fixture");
    fs::write(directory.path().join("file"), b"abcd").expect("file");
    let reader = SelectedRepositoryReader::open(directory.path()).expect("root");
    assert_eq!(reader.read("file", 3), Err(Error::TooLarge));
    assert_eq!(reader.read("file", 0), Err(Error::TooLarge));
    assert!(reader.read("file", 4).is_ok());
    assert_eq!(reader.read("file", usize::MAX), Err(Error::InvalidLimit));
    let large = fs::File::create(directory.path().join("large")).expect("sparse file");
    large.set_len(MAX_READ_BYTES as u64 + 1).expect("size");
    assert_eq!(reader.read("large", MAX_READ_BYTES), Err(Error::TooLarge));
}

#[test]
fn links_are_literal_bounded_and_never_followed() {
    let directory = tempfile::tempdir().expect("fixture");
    fs::create_dir(directory.path().join("links")).expect("links");
    symlink("./../missing//file", directory.path().join("links/current")).expect("dangling literal link");
    let reader = SelectedRepositoryReader::open(directory.path()).expect("root");
    let target = "./../missing//file";
    let node = reader.read("links/current", target.len()).expect("literal link");
    assert_eq!(node, Node::Symlink { target: target.into() });
    assert!(!format!("{node:?}").contains("missing"));
    assert_eq!(reader.read("links/current", target.len() - 1), Err(Error::TooLarge));
    for (name, target) in [
        ("escape", "../../outside"),
        ("absolute", "/outside"),
        ("excluded", "../.env"),
    ] {
        symlink(target, directory.path().join("links").join(name)).expect("unsafe link fixture");
        assert!(matches!(
            reader.read(&format!("links/{name}"), 100),
            Err(Error::Policy(OverlayPlanError::InvalidLink))
        ));
    }
}

#[test]
fn linked_roots_and_parents_are_rejected_including_in_root_aliases() {
    let directory = tempfile::tempdir().expect("fixture");
    let root = directory.path().join("root");
    fs::create_dir(&root).expect("root");
    fs::write(root.join("file"), b"inside").expect("inside");
    fs::write(directory.path().join("outside"), b"never read").expect("outside");
    symlink(&root, directory.path().join("alias")).expect("root alias");
    assert!(matches!(
        SelectedRepositoryReader::open(&directory.path().join("alias")),
        Err(Error::UnsafePath)
    ));
    symlink(directory.path(), root.join("escape")).expect("parent escape");
    symlink(".", root.join("alias")).expect("internal parent alias");
    let reader = SelectedRepositoryReader::open(&root).expect("root");
    assert_eq!(reader.read("escape/outside", 100), Err(Error::UnsafePath));
    assert_eq!(reader.read("alias/file", 100), Err(Error::UnsafePath));
    assert_eq!(
        fs::read(directory.path().join("outside")).expect("outside intact"),
        b"never read"
    );
}

#[test]
fn directories_fifos_sockets_and_hardlinks_fail_without_opening_for_content() {
    let directory = tempfile::tempdir().expect("fixture");
    fs::create_dir(directory.path().join("subdirectory")).expect("directory");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        directory.path().join("fifo"),
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .expect("fifo");
    let _socket = UnixListener::bind(directory.path().join("socket")).expect("socket");
    fs::write(directory.path().join("original"), b"bytes").expect("file");
    fs::hard_link(directory.path().join("original"), directory.path().join("hardlink")).expect("hardlink");
    let reader = SelectedRepositoryReader::open(directory.path()).expect("root");
    for path in ["subdirectory", "fifo", "socket", "original", "hardlink"] {
        assert_eq!(reader.read(path, 100), Err(Error::UnsupportedNode));
    }
}

#[test]
fn root_handle_does_not_switch_to_a_replacement_at_its_old_path() {
    let directory = tempfile::tempdir().expect("fixture");
    let root = directory.path().join("root");
    fs::create_dir(&root).expect("root");
    fs::write(root.join("file"), b"selected").expect("selected file");
    let reader = SelectedRepositoryReader::open(&root).expect("root");
    fs::rename(&root, directory.path().join("retained")).expect("rename");
    fs::create_dir(&root).expect("replacement root");
    fs::write(root.join("file"), b"not selected").expect("replacement file");
    assert_eq!(
        reader.read("file", 100),
        Ok(Node::File {
            bytes: b"selected".to_vec(),
            executable: false
        })
    );
}

#[test]
fn invalid_roots_and_content_debug_do_not_leak_values() {
    for path in ["relative", "/", "/tmp/../private"] {
        assert!(matches!(
            SelectedRepositoryReader::open(Path::new(path)),
            Err(Error::InvalidRoot)
        ));
    }
    let node = Node::File {
        bytes: b"private-source".to_vec(),
        executable: true,
    };
    assert!(!format!("{node:?}").contains("private-source"));
    assert!(!format!("{node:?}").contains("112, 114"));
}
