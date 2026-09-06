use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

#[test]
fn pinned_regular_node_rejects_in_place_content_or_mode_changes() {
    let directory = tempfile::tempdir().expect("fixture");
    let path = directory.path().join("file");
    fs::write(&path, b"a").expect("file");
    let root = Root::open(directory.path()).expect("root");
    let node = root.pin("file").expect("pin");
    fs::write(&path, b"different length").expect("mutation");
    assert_eq!(node.regular(100), Err(Error::Changed));
    let node = root.pin("file").expect("new pin");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("mode mutation");
    assert_eq!(node.regular(100), Err(Error::Changed));
}

#[test]
fn replacement_after_pinning_never_redirects_a_content_open() {
    let directory = tempfile::tempdir().expect("fixture");
    let path = directory.path().join("file");
    fs::write(&path, b"selected").expect("file");
    let root = Root::open(directory.path()).expect("root");
    let node = root.pin("file").expect("pin");
    fs::rename(&path, directory.path().join("retained")).expect("rename");
    rustix::fs::mkfifoat(CWD, &path, Mode::RUSR | Mode::WUSR).expect("replacement fifo");
    // Renaming may change ctime; either rejection or the pinned old bytes are safe.
    let result = node.regular(100);
    assert!(
        result == Err(Error::Changed)
            || result
                == Ok(Node::File {
                    bytes: b"selected".to_vec(),
                    executable: false
                })
    );
    assert_eq!(root.verify_path("file", &node), Err(Error::Changed));
}

#[test]
fn changed_parent_and_link_identity_fail_final_revalidation() {
    let directory = tempfile::tempdir().expect("fixture");
    fs::create_dir(directory.path().join("parent")).expect("parent");
    fs::write(directory.path().join("parent/file"), b"selected").expect("file");
    let root = Root::open(directory.path()).expect("root");
    let node = root.pin("parent/file").expect("pin");
    fs::rename(directory.path().join("parent"), directory.path().join("retained")).expect("rename");
    symlink("retained", directory.path().join("parent")).expect("linked parent");
    assert_eq!(root.verify_path("parent/file", &node), Err(Error::Changed));
    symlink("old", directory.path().join("link")).expect("link");
    let link = root.pin("link").expect("pin link");
    fs::remove_file(directory.path().join("link")).expect("replace owned link");
    symlink("new", directory.path().join("link")).expect("replacement link");
    assert_eq!(link.link("link", 100), Ok(Node::Symlink { target: "old".into() }));
    assert_eq!(root.verify_path("link", &link), Err(Error::Changed));
}

#[test]
fn confinement_errors_never_trigger_fallback_or_unbounded_retries() {
    for errno in [
        rustix::io::Errno::LOOP,
        rustix::io::Errno::XDEV,
        rustix::io::Errno::AGAIN,
    ] {
        assert_eq!(open_error(errno), Error::UnsafePath);
    }
    for errno in [rustix::io::Errno::NOSYS, rustix::io::Errno::INVAL] {
        assert_eq!(open_error(errno), Error::Unsupported);
    }
    assert_eq!(open_error(rustix::io::Errno::ACCESS), Error::ReadFailed);
}

#[test]
fn growing_or_truncated_input_is_rejected_without_partial_or_unbounded_results() {
    let mut input = std::io::Cursor::new(b"original and concurrent growth");
    assert_eq!(read_limited(&mut input, 4, 8), Err(Error::TooLarge));
    assert_eq!(input.position(), 9);
    assert_eq!(read_limited(&b"grown"[..], 4, 8), Err(Error::Changed));
    assert_eq!(read_limited(&b"short"[..], 8, 8), Err(Error::Changed));
    assert_eq!(read_limited(&b"exact"[..], 5, 5), Ok(b"exact".to_vec()));
}
