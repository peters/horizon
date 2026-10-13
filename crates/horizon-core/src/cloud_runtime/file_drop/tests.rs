use super::*;

fn names(paths: &[&str]) -> Vec<String> {
    remote_names(&paths.iter().map(PathBuf::from).collect::<Vec<_>>())
}

#[test]
fn remote_names_keep_ordinary_names_and_number_repeats_before_the_extension() {
    assert_eq!(
        names(&[
            "/a/screen shot.png",
            "/b/screen shot.png",
            "/c/screen shot.png",
            "/d/O'Brien notes.txt",
            "/e/ærlig 测试.jpeg",
            "/f/.env",
            "/g/.env",
            "/h/-dash",
        ]),
        [
            "screen shot.png",
            "screen shot (2).png",
            "screen shot (3).png",
            "O'Brien notes.txt",
            "ærlig 测试.jpeg",
            ".env",
            ".env (2)",
            "-dash",
        ]
    );
}

#[test]
fn remote_names_replace_control_characters_and_fit_the_worker_limit() {
    assert_eq!(names(&["/a/line\nbreak\t.txt"]), ["line_break_.txt"]);
    assert_eq!(names(&["/", "/a/.."]), ["file", "file (2)"]);
    let long = format!("/a/{}.png", "é".repeat(200));
    let [first, second]: [String; 2] = names(&[&long, &long]).try_into().unwrap();
    assert!(first.len() <= NAME_MAX && first.ends_with("é.png"), "{first}");
    assert!(second.len() <= NAME_MAX && second.ends_with(" (2).png"), "{second}");
    let extension = format!("/a/x.{}", "e".repeat(300));
    assert_eq!(names(&[&extension])[0].len(), NAME_MAX);
}

#[test]
fn a_destination_refuses_identities_the_worker_refuses() {
    let worker: Worker = serde_json::from_value(serde_json::json!({
        "id":"worker1", "name":"fixture", "imageName":"fixture", "desiredStatus":"RUNNING",
        "sshHost":"192.0.2.1", "portMappings":{"22":2222}
    }))
    .unwrap();
    let root = Path::new("/synthetic");
    let destination = Destination::new(&worker, root, "cloud", "session-1").unwrap();
    assert_eq!(destination.worker_id(), "worker1");
    assert_eq!(destination.session(), "session-1");
    let batch = destination.batch();
    assert!(batch.directory().starts_with("/workspace/.horizon-drops/session-1/"));
    assert_ne!(batch, destination.batch(), "each drop gets its own folder");
    for session in ["", "../escape", "a b", "a;rm"] {
        assert!(
            Destination::new(&worker, root, "cloud", session).is_err(),
            "{session:?}"
        );
    }
    assert!(Destination::new(&worker, root, "../cloud", "session-1").is_err());
}

/// The worker runs these scripts with GNU coreutils; macOS `stat` takes other options.
#[cfg(target_os = "linux")]
mod worker_scripts {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Output, Stdio};
    use std::time::{Duration, Instant};

    struct Fixture {
        _temp: tempfile::TempDir,
        workspace: PathBuf,
        batch: Batch,
    }

    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(workspace.join("sessions/session-1")).unwrap();
        let batch = Batch::new(workspace.to_str().unwrap(), "session-1");
        Fixture {
            _temp: temp,
            workspace,
            batch,
        }
    }

    /// Runs a remote command as the worker's login shell does: as one `sh -c` string.
    fn run(command: &str, input: &[u8]) -> Output {
        let mut child = Command::new("/bin/sh")
            .args(["-c", command])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn part(batch: &Batch) -> PathBuf {
        PathBuf::from(batch.directory()).with_file_name(format!(".{}.{}.part", batch.id, batch.token))
    }

    fn entries(path: &Path) -> usize {
        std::fs::read_dir(path).unwrap().count()
    }

    #[test]
    fn a_batch_receives_unusual_names_complete_and_readable() {
        let fixture = fixture();
        let prepared = run(&fixture.batch.prepare_command(), b"");
        assert!(prepared.status.success(), "{prepared:?}");
        let directory = PathBuf::from(fixture.batch.directory());
        assert_eq!(mode(&directory), 0o750);
        assert_eq!(mode(directory.parent().unwrap()), 0o750);
        assert_eq!(
            mode(&part(&fixture.batch)),
            0o700,
            "only the SSH user sees partial files"
        );
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0xff];
        for (name, content) in [
            ("screen shot.png", &png[..]),
            ("O'Brien \"notes\" $HOME.txt", b"text\n"),
            ("ærlig 测试.jpeg", b"\xff\xd8\xff"),
            ("-dash", b""),
        ] {
            let written = run(&fixture.batch.write_command(name, content.len() as u64), content);
            assert!(written.status.success(), "{name}: {written:?}");
            let path = PathBuf::from(fixture.batch.path(name));
            assert_eq!(std::fs::read(&path).unwrap(), content, "{name}");
            assert_eq!(mode(&path), 0o640, "{name}");
            assert_eq!(
                std::fs::metadata(&path).unwrap().gid(),
                std::fs::metadata(&directory).unwrap().gid()
            );
        }
        assert_eq!(entries(&part(&fixture.batch)), 0);
        let again = run(&fixture.batch.write_command("screen shot.png", 3), b"new");
        assert!(!again.status.success(), "an upload never replaces a file");
        assert_eq!(std::fs::read(fixture.batch.path("screen shot.png")).unwrap(), png);
        assert!(
            !run(&fixture.batch.prepare_command(), b"").status.success(),
            "a batch folder is new"
        );
        assert!(run(&fixture.batch.finish_command(), b"").status.success());
        assert!(
            !part(&fixture.batch).exists(),
            "a finished drop keeps no private folder"
        );
        assert!(run(&fixture.batch.remove_command(), b"").status.success());
        assert!(directory.exists(), "the cleanup does not remove a finished drop");
    }

    #[test]
    fn a_cancel_before_the_finish_removes_a_complete_batch() {
        let fixture = fixture();
        assert!(run(&fixture.batch.prepare_command(), b"").status.success());
        assert!(
            run(&fixture.batch.write_command("done.txt", 4), b"done")
                .status
                .success()
        );
        assert!(run(&fixture.batch.remove_command(), b"").status.success());
        assert!(!Path::new(&fixture.batch.directory()).exists());
        assert!(!part(&fixture.batch).exists());
    }

    /// Cuts a transfer off with `signal` sent to its process group, as sshd does when the
    /// connection ends, after the worker started to write the file.
    fn cut_off(batch: &Batch, signal: &str) {
        let mut child = Command::new("/bin/sh")
            .args(["-c", &batch.write_command("cut off.bin", 1 << 20)])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(&[7; 4096]).unwrap();
        stdin.flush().unwrap();
        let partial = part(batch).join("cut off.bin");
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::fs::metadata(&partial).map_or(true, |metadata| metadata.len() < 4096) {
            assert!(Instant::now() < deadline, "the worker did not start to write");
            std::thread::sleep(Duration::from_millis(10));
        }
        let group = format!("-{}", child.id());
        assert!(
            Command::new("kill")
                .args([signal, "--", &group])
                .status()
                .unwrap()
                .success()
        );
        drop(stdin);
        assert!(!child.wait().unwrap().success());
    }

    #[test]
    fn a_transfer_cut_off_by_a_signal_leaves_nothing_in_the_batch_folder() {
        let fixture = fixture();
        assert!(run(&fixture.batch.prepare_command(), b"").status.success());
        let directory = PathBuf::from(fixture.batch.directory());
        cut_off(&fixture.batch, "-HUP");
        assert_eq!(entries(&directory), 0);
        // The outer test shell ends at once; the worker's shell then runs its hangup trap.
        let deadline = Instant::now() + Duration::from_secs(10);
        while entries(&part(&fixture.batch)) > 0 {
            assert!(Instant::now() < deadline, "the hangup trap removes the partial file");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(entries(&directory), 0);
        // No trap runs for SIGKILL: the partial file stays private, and the cleanup removes it.
        cut_off(&fixture.batch, "-KILL");
        assert_eq!(entries(&directory), 0);
        assert!(run(&fixture.batch.remove_command(), b"").status.success());
        assert!(!directory.exists() && !part(&fixture.batch).exists());
    }

    #[test]
    fn a_short_or_long_transfer_leaves_no_file() {
        let fixture = fixture();
        assert!(run(&fixture.batch.prepare_command(), b"").status.success());
        for (size, content) in [(10, &b"cut"[..]), (2, b"longer")] {
            let written = run(&fixture.batch.write_command("partial.bin", size), content);
            assert_eq!(written.status.code(), Some(4), "{written:?}");
            assert!(String::from_utf8_lossy(&written.stderr).contains("kept nothing"));
            assert!(!Path::new(&fixture.batch.path("partial.bin")).exists());
        }
    }

    #[test]
    fn preparation_refuses_an_unknown_session_and_a_redirected_folder() {
        let fixture = fixture();
        let unknown = Batch::new(fixture.workspace.to_str().unwrap(), "session-2");
        let refused = run(&unknown.prepare_command(), b"");
        assert_eq!(refused.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&refused.stderr).contains("no session for this panel"));
        assert!(!fixture.workspace.join(".horizon-drops").exists());

        let elsewhere = fixture.workspace.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, fixture.workspace.join(".horizon-drops")).unwrap();
        let refused = run(&fixture.batch.prepare_command(), b"");
        assert_eq!(refused.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&refused.stderr).contains("symbolic link"));
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
        let written = run(&fixture.batch.write_command("file.txt", 1), b"x");
        assert_eq!(written.status.code(), Some(3), "no write follows the link");
        // Folders that look like this drop's, behind the link: the cleanup leaves them.
        let behind = elsewhere.join("session-1");
        std::fs::create_dir_all(behind.join(&fixture.batch.id)).unwrap();
        std::fs::write(behind.join(&fixture.batch.id).join("keep.txt"), "keep").unwrap();
        std::fs::create_dir(behind.join(part(&fixture.batch).file_name().unwrap())).unwrap();
        assert!(run(&fixture.batch.remove_command(), b"").status.success());
        assert!(behind.join(&fixture.batch.id).join("keep.txt").exists());
    }

    #[test]
    fn a_drop_that_meets_another_drops_batch_folder_removes_nothing_of_it() {
        let fixture = fixture();
        assert!(run(&fixture.batch.prepare_command(), b"").status.success());
        assert!(
            run(&fixture.batch.write_command("first.txt", 5), b"first")
                .status
                .success()
        );
        let mut second = Batch::new(fixture.workspace.to_str().unwrap(), "session-1");
        second.id.clone_from(&fixture.batch.id);
        let refused = run(&second.prepare_command(), b"");
        assert!(!refused.status.success(), "a batch folder is new");
        assert!(!part(&second).exists());
        assert!(run(&second.remove_command(), b"").status.success());
        assert_eq!(std::fs::read(fixture.batch.path("first.txt")).unwrap(), b"first");
        assert!(part(&fixture.batch).exists());
    }
}
