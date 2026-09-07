use super::*;
use rustix::io::{FdFlags, fcntl_getfd};
use std::{os::unix::fs::PermissionsExt, process::Command};

const EXIT_DIRECTORY: &str = "HORIZON_TEST_ANONYMOUS_TRUST_DIRECTORY";

fn private_directory() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("fixture");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).expect("private");
    directory
}

fn snapshot(path: &Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(path)
        .expect("directory")
        .map(|entry| {
            let entry = entry.expect("entry");
            (entry.file_name(), std::fs::read(entry.path()).expect("file"))
        })
        .collect();
    files.sort();
    files
}

#[test]
fn exact_owner_descriptor_is_anonymous_private_and_readable_by_a_child() {
    let directory = private_directory();
    std::fs::write(directory.path().join("identity"), b"synthetic identity sentinel").expect("sentinel");
    let before = snapshot(directory.path());
    let first = KnownHosts::create(directory.path(), "synthetic first pin").expect("first");
    let second = KnownHosts::create(directory.path(), "synthetic second pin").expect("second");
    assert_ne!(first.path(), second.path());
    assert_eq!(fcntl_getfd(&first.file).expect("descriptor flags"), FdFlags::CLOEXEC);
    for guard in [&first, &second] {
        let metadata = std::fs::metadata(guard.path()).expect("held inode");
        assert!(metadata.is_file());
        assert_eq!(metadata.nlink(), 0);
        assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(metadata.mode() & 0o7777, 0o600);
        assert_eq!(
            guard.path(),
            Path::new(&format!("/proc/{}/fd/{}", std::process::id(), guard.file.as_raw_fd()))
        );
    }
    let output = Command::new("/bin/cat")
        .arg(first.path())
        .output()
        .expect("child reader");
    assert!(output.status.success());
    assert_eq!(output.stdout, format!("{HOST_ALIAS} synthetic first pin\n").as_bytes());
    drop(first);
    assert_eq!(
        std::fs::read_to_string(second.path()).expect("independent reader"),
        format!("{HOST_ALIAS} synthetic second pin\n")
    );
    assert_eq!(snapshot(directory.path()), before);
    drop(second);
    assert_eq!(snapshot(directory.path()), before);
}

#[test]
fn unsafe_or_unavailable_parent_rejects_without_a_named_fallback() {
    let directory = private_directory();
    let alias = directory.path().join("alias");
    std::os::unix::fs::symlink(directory.path(), &alias).expect("fixture link");
    assert!(matches!(KnownHosts::create(&alias, "pin"), Err(Error::TrustStorage)));
    assert!(matches!(
        KnownHosts::create(&directory.path().join("missing"), "pin"),
        Err(Error::TrustStorage)
    ));
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).expect("insecure");
    let failure = KnownHosts::create(directory.path(), "pin");
    assert!(matches!(failure, Err(Error::TrustStorage)));
    assert_eq!(std::fs::read_dir(directory.path()).expect("no fallback").count(), 1);
}

#[test]
fn process_exit_without_destructors_leaves_no_named_pin_or_identity_change() {
    let directory = private_directory();
    std::fs::write(directory.path().join("identity"), b"synthetic identity sentinel").expect("sentinel");
    let before = snapshot(directory.path());
    let output = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "remote_worker_ssh::trust::tests::process_exit_fixture",
            "--nocapture",
        ])
        .env(EXIT_DIRECTORY, directory.path())
        .output()
        .expect("owned exit fixture");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("anonymous trust held before exit"));
    assert_eq!(snapshot(directory.path()), before);
}

#[test]
fn process_exit_fixture() {
    let Some(directory) = std::env::var_os(EXIT_DIRECTORY) else {
        return;
    };
    let guard = KnownHosts::create(Path::new(&directory), "synthetic exit pin").expect("anonymous trust");
    assert_eq!(std::fs::metadata(guard.path()).expect("held").nlink(), 0);
    assert_eq!(std::fs::read_dir(directory).expect("no named pin").count(), 1);
    println!("anonymous trust held before exit");
    std::process::exit(0);
}
