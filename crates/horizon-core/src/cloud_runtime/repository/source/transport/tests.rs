use super::*;

#[test]
fn git_progress_is_read_for_its_phase_how_far_and_the_speed() {
    assert_eq!(
        parse_progress("Receiving objects:  45% (450/1000), 12.30 MiB | 4.50 MiB/s"),
        Some(("Receiving objects".into(), 45, "12.30 MiB | 4.50 MiB/s".into()))
    );
    assert_eq!(
        parse_progress("remote: Counting objects: 100% (5/5), done."),
        Some(("Counting objects".into(), 100, String::new()))
    );
    assert_eq!(
        parse_progress("Resolving deltas: 100% (10/10), done."),
        Some(("Resolving deltas".into(), 100, String::new()))
    );
    assert_eq!(
        parse_progress("Resolving deltas: 100% (1234/1234), completed with 2710 local objects."),
        Some(("Resolving deltas".into(), 100, String::new()))
    );
    assert!(is_chatter("remote: Enumerating objects: 34191, done."));
    assert!(is_chatter(
        "remote: Total 34191 (delta 0), reused 0 (delta 0), pack-reused 34191"
    ));
    assert!(!is_chatter("remote: Repository not found."));
    assert!(!is_chatter("fatal: early EOF"));
    assert_eq!(parse_progress("fatal: repository not found"), None);
    assert_eq!(parse_progress("Cloning into 'x'..."), None);
}

#[test]
fn the_time_left_follows_the_pace_and_waits_for_one() {
    assert_eq!(eta(Duration::from_secs(10), 50), Some(Duration::from_secs(10)));
    assert_eq!(eta(Duration::from_secs(9), 90), Some(Duration::from_secs(1)));
    assert_eq!(eta(Duration::from_millis(500), 50), None, "too early to know");
    assert_eq!(eta(Duration::from_secs(10), 1), None);
    assert_eq!(eta(Duration::from_secs(10), 100), None, "nothing left");
}

#[test]
fn a_host_that_never_stops_talking_leaves_only_a_bounded_tail() {
    let mut tail = String::new();
    for _ in 0..10_000 {
        keep_tail(&mut tail, "fatal: a very talkative remote, éééé\n");
    }
    keep_tail(&mut tail, "fatal: could not read Username");
    assert!(tail.len() <= STDERR_TAIL);
    assert!(
        tail.ends_with("could not read Username"),
        "the end is what names the failure"
    );
}

#[test]
fn a_pipe_that_never_closes_cannot_hold_the_deadline() {
    let (_sender, reader) = mpsc::channel::<String>();
    let started = Instant::now();
    assert_eq!(collected(&reader, Duration::from_millis(100)), "");
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// A repository with a few commits and a second branch, reachable by a `file://` address so
/// that Git honours a depth.
fn origin(temp: &Path) -> Remote {
    origin_with(temp, 3)
}

fn origin_with(temp: &Path, commits: usize) -> Remote {
    let origin = temp.join("origin");
    std::fs::create_dir(&origin).unwrap();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .arg("-C")
            .arg(&origin)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "{args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    for number in 1..=commits {
        git(&["commit", "-q", "--allow-empty", "-m", &format!("commit {number}")]);
    }
    git(&["branch", "extra"]);
    // A tag on a commit that no branch reaches, which Git does not follow on its own.
    git(&["checkout", "-q", "--detach"]);
    git(&["commit", "-q", "--allow-empty", "-m", "off every branch"]);
    git(&["tag", "lonely"]);
    git(&["checkout", "-q", "main"]);
    Remote {
        url: {
            let path = origin.to_string_lossy().replace('\\', "/");
            if path.starts_with('/') {
                format!("file://{path}")
            } else {
                format!("file:///{path}")
            }
        },
        host: "example.com".into(),
        name: "origin".into(),
    }
}

fn count(folder: &Path, args: &[&str]) -> String {
    git_output(folder, args).unwrap().trim().to_owned()
}

#[test]
fn a_clone_that_stops_after_a_step_is_picked_up_where_it_stopped() {
    let temp = tempfile::tempdir().unwrap();
    let remote = origin(temp.path());
    let target = temp.path().join("clones").join("origin");
    let (cancel, progress) = (Cancellation::default(), Progress::default());
    let stopped = in_steps(&remote, &target, None, &cancel, &progress, 1);
    assert!(
        matches!(&stopped, Err(Failure::Interrupted(text)) if text.contains("Continue resumes")),
        "{stopped:?}"
    );
    assert!(
        target.join(".git").join(MARKER).is_file(),
        "the folder says it is unfinished"
    );
    assert_eq!(resumable(target.parent().unwrap(), &remote), Some(target.clone()));
    assert!(!super::super::is_checkout(&target));
    assert_eq!(
        count(&target, &["rev-list", "--count", "refs/remotes/origin/main"]),
        "1",
        "the latest commit only"
    );

    clone(&remote, &target, None, &cancel, &progress).unwrap();
    assert!(super::super::is_checkout(&target));
    assert!(!target.join(".git").join(MARKER).exists(), "finished");
    assert!(
        std::fs::read_dir(target.join(".git"))
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().contains("claim")),
        "a finished checkout keeps nothing of the clone"
    );
    assert!(!target.join(".git").join("shallow").exists(), "the whole history");
    assert_eq!(count(&target, &["rev-list", "--count", "HEAD"]), "3");
    assert_eq!(count(&target, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert_eq!(
        count(&target, &["rev-parse", "--abbrev-ref", "main@{upstream}"]),
        "origin/main"
    );
    assert_eq!(
        count(&target, &["rev-parse", "--verify", "refs/remotes/origin/extra"]).len(),
        40,
        "every branch"
    );
    assert!(progress.lock().unwrap().resumed || progress.lock().unwrap().step == STEPS);
    assert_eq!(resumable(target.parent().unwrap(), &remote), None);
}

#[cfg(unix)]
#[test]
fn progress_is_shown_but_never_becomes_the_reason_a_step_failed() {
    let temp = tempfile::tempdir().unwrap();
    let remote = origin(temp.path());
    let step = |script: &str| {
        let mut command = Command::new("sh");
        command.args(["-c", script]).stderr(Stdio::piped());
        let progress = Progress::default();
        let result = run(command, temp.path(), &remote, &Cancellation::default(), &progress);
        (result, progress.lock().unwrap().clone())
    };
    let (result, shown) = step("printf 'Receiving objects:  50%% (1/2), 1.00 MiB | 1.00 MiB/s\\r' >&2; kill -KILL $$");
    assert_eq!(result, Err(Failure::Other("Git stopped before it finished.".into())));
    assert_eq!((shown.phase.as_str(), shown.percent), ("Receiving objects", Some(50)));
    let (result, _) = step("printf 'remote: Enumerating objects: 5, done.\\n' >&2; kill -KILL $$");
    assert_eq!(
        result,
        Err(Failure::Other("Git stopped before it finished.".into())),
        "chatter is not a reason"
    );
    let (result, _) = step("printf 'Receiving objects:  50%% (1/2)\\rfatal: early EOF\\n' >&2; exit 128");
    assert_eq!(result, Err(Failure::Other("fatal: early EOF".into())));
    let (result, _) = step("printf 'fatal: no newline at the end' >&2; exit 128");
    assert_eq!(
        result,
        Err(Failure::Other("fatal: no newline at the end".into())),
        "the last word counts without a line end"
    );
}

#[test]
fn the_history_comes_in_pieces_and_each_finished_piece_is_kept() {
    assert_eq!(usize::from(STEPS), DEPTHS.len() + 1);
    let temp = tempfile::tempdir().unwrap();
    let remote = origin_with(temp.path(), 130);
    let target = temp.path().join("deep");
    let (cancel, progress) = (Cancellation::default(), Progress::default());
    let commits = |folder: &Path| count(folder, &["rev-list", "--count", "refs/remotes/origin/main"]);
    assert!(in_steps(&remote, &target, None, &cancel, &progress, 1).is_err());
    assert_eq!((commits(&target), marker_done(&target)), ("1".to_owned(), 1));
    assert!(in_steps(&remote, &target, None, &cancel, &progress, 2).is_err());
    assert_eq!((commits(&target), marker_done(&target)), ("100".to_owned(), 2));
    // A drop during the third piece keeps the two before it, and the try after starts at the third.
    assert!(in_steps(&remote, &target, None, &cancel, &progress, 3).is_err());
    assert_eq!((commits(&target), marker_done(&target)), ("130".to_owned(), 3));
    clone(&remote, &target, None, &cancel, &progress).unwrap();
    assert!(super::super::is_checkout(&target) && !target.join(".git").join(MARKER).exists());
    assert!(!target.join(".git").join("shallow").exists());
    assert_eq!(count(&target, &["rev-list", "--count", "HEAD"]), "130");
    let shown = progress.lock().unwrap().clone();
    assert_eq!((shown.step, shown.steps, shown.resumed), (STEPS, STEPS, true));
}

#[test]
fn the_checkout_carries_the_token_like_the_fetches_do() {
    let remote = super::super::parse("github.com/demo-org/demo").unwrap();
    let token = Token::new(&remote, "demo_token").unwrap();
    let envs = |command: &Command| {
        command
            .get_envs()
            .filter_map(|(key, value)| Some((key.to_str()?.to_owned(), value?.to_str()?.to_owned())))
            .collect::<std::collections::HashMap<_, _>>()
    };
    let with = envs(&checkout_command("main", Some(&token)));
    assert_eq!(with.get("GIT_CONFIG_COUNT").map(String::as_str), Some("1"));
    assert!(with["GIT_CONFIG_KEY_0"].starts_with("http.https://github.com"));
    assert!(!envs(&checkout_command("main", None)).contains_key("GIT_CONFIG_COUNT"));
    let args: Vec<_> = checkout_command("main", Some(&token))
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert!(args.iter().all(|arg| !arg.contains("demo_token")), "never argv");
}

#[test]
fn the_marker_is_replaced_whole_and_a_leftover_staging_file_changes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join(".git")).unwrap();
    let first = Marker {
        url: "https://github.com/demo-org/demo.git".into(),
        branch: "main".into(),
        done: 1,
    };
    write_marker(temp.path(), &first).unwrap();
    // A write cut off half way only ever touches the staging file.
    std::fs::write(
        temp.path().join(".git").join("horizon-clone.tmp"),
        "https://github.com/demo-or",
    )
    .unwrap();
    assert_eq!(marker_done(temp.path()), 1);
    clear_leftovers(temp.path());
    assert!(!temp.path().join(".git").join("horizon-clone.tmp").exists());
    write_marker(temp.path(), &Marker { done: 3, ..first }).unwrap();
    assert_eq!(marker_done(temp.path()), 3);
    assert!(!temp.path().join(".git").join("horizon-clone.tmp").exists());
}

#[test]
fn a_folder_another_process_is_cloning_into_is_neither_resumed_nor_discarded() {
    let temp = tempfile::tempdir().unwrap();
    let remote = origin(temp.path());
    let target = temp.path().join("busy");
    let (cancel, progress) = (Cancellation::default(), Progress::default());
    assert!(in_steps(&remote, &target, None, &cancel, &progress, 1).is_err());
    // Another process holds the folder: it is not ours to touch, and it stays as it is.
    let other = Claim::take(&target).unwrap();
    let resumed = clone(&remote, &target, None, &cancel, &progress);
    assert!(
        matches!(&resumed, Err(Failure::Other(text)) if text.contains("Another Horizon window")),
        "{resumed:?}"
    );
    assert!(matches!(discard(&target), Err(Failure::Other(text)) if text.contains("Another Horizon window")));
    assert!(target.join(".git").is_dir() && marker(&target).is_some());
    assert_eq!(marker_done(&target), 1, "what it received is untouched");
    // Once it lets go, the folder is resumed as usual.
    drop(other);
    clone(&remote, &target, None, &cancel, &progress).unwrap();
    assert!(super::super::is_checkout(&target));
    assert!(
        std::fs::read_dir(target.join(".git"))
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().contains("claim"))
    );
}

#[test]
fn a_folder_that_is_not_a_clone_is_never_claimed() {
    let temp = tempfile::tempdir().unwrap();
    let remote = origin(temp.path());
    let theirs = temp.path().join("theirs");
    std::fs::create_dir_all(theirs.join(".git")).unwrap();
    assert!(clone(&remote, &theirs, None, &Cancellation::default(), &Progress::default()).is_err());
    assert_eq!(
        std::fs::read_dir(theirs.join(".git")).unwrap().count(),
        0,
        "no file is added to a repository that is not ours"
    );
    // The hold on it is let go: another try takes it at once.
    drop(Claim::take(&theirs).unwrap());
}

#[test]
fn a_resume_clears_the_locks_a_killed_git_left_behind() {
    let temp = tempfile::tempdir().unwrap();
    let remote = origin(temp.path());
    let target = temp.path().join("origin-clone");
    let (cancel, progress) = (Cancellation::default(), Progress::default());
    assert!(in_steps(&remote, &target, None, &cancel, &progress, 1).is_err());
    let git = target.join(".git");
    for stale in ["shallow.lock", "index.lock", "HEAD.lock"] {
        std::fs::write(git.join(stale), "").unwrap();
    }
    std::fs::write(git.join("refs/remotes/origin/main.lock"), "").unwrap();
    std::fs::write(git.join("objects/pack/tmp_pack_abc123"), "partial").unwrap();
    clone(&remote, &target, None, &cancel, &progress).unwrap();
    assert!(super::super::is_checkout(&target));
    assert!(!git.join("shallow.lock").exists() && !git.join("objects/pack/tmp_pack_abc123").exists());
}

#[cfg(unix)]
#[test]
fn clearing_leftovers_never_follows_a_link_out_of_the_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("theirs.lock"), "not ours").unwrap();
    let folder = temp.path().join("checkout");
    std::fs::create_dir_all(folder.join(".git").join("refs").join("heads")).unwrap();
    std::fs::write(folder.join(".git").join("refs").join("heads").join("main.lock"), "").unwrap();
    std::os::unix::fs::symlink(&outside, folder.join(".git").join("refs").join("link")).unwrap();
    clear_leftovers(&folder);
    assert!(
        outside.join("theirs.lock").is_file(),
        "a link leads outside, and is left alone"
    );
    assert!(
        !folder
            .join(".git")
            .join("refs")
            .join("heads")
            .join("main.lock")
            .exists()
    );
}

#[test]
fn a_clone_that_never_received_anything_leaves_no_folder_and_a_discard_removes_a_stopped_one() {
    let temp = tempfile::tempdir().unwrap();
    let remote = origin(temp.path());
    let cancelled = Cancellation::default();
    cancelled.cancel();
    let target = temp.path().join("early");
    assert_eq!(
        clone(&remote, &target, None, &cancelled, &Progress::default()),
        Err(Failure::Cancelled)
    );
    assert!(!target.exists());
    let stopped = temp.path().join("stopped");
    let _ = in_steps(
        &remote,
        &stopped,
        None,
        &Cancellation::default(),
        &Progress::default(),
        1,
    );
    assert!(stopped.is_dir());
    assert_eq!(discard(&stopped), Ok(()));
    assert!(!stopped.exists());
    let theirs = temp.path().join("theirs");
    std::fs::create_dir(&theirs).unwrap();
    assert_eq!(discard(&theirs), Ok(()));
    assert!(
        theirs.is_dir(),
        "a folder that is not an unfinished clone is left alone"
    );
}

#[cfg(unix)]
#[test]
fn a_helper_that_keeps_stdout_open_cannot_hold_a_command_past_its_limit() {
    let temp = tempfile::tempdir().unwrap();
    let remote = origin(temp.path());
    // The shell ends at once, but a process it started keeps both pipes open for far longer.
    let mut command = Command::new("sh");
    command
        .args(["-c", "sleep 20 & echo 'ref: refs/heads/main\tHEAD'"])
        .stderr(Stdio::piped());
    let started = Instant::now();
    let result = bounded(command, &remote, &Cancellation::default(), Duration::from_millis(600));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
    assert!(
        result.is_ok(),
        "a command that ended well is not failed for a pipe left open: {result:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_removed_says_so_instead_of_reporting_success() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let remote = origin(temp.path());
    let parent = temp.path().join("clones");
    let target = parent.join("stuck");
    let (cancel, progress) = (Cancellation::default(), Progress::default());
    assert!(in_steps(&remote, &target, None, &cancel, &progress, 1).is_err());
    // The hold is made once, so that it can be taken again in a folder that can no longer be written to.
    drop(Claim::take(&target).unwrap());
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();
    let writable = std::fs::write(parent.join("probe"), "").is_ok();
    let result = discard(&target);
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    if writable {
        return; // Permissions do not bind this user (root), so there is nothing to refuse.
    }
    assert!(
        matches!(&result, Err(Failure::Other(text)) if text.contains("Could not remove")),
        "{result:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_link_to_a_stopped_clone_is_not_a_place_to_resume_it() {
    let temp = tempfile::tempdir().unwrap();
    let remote = origin(temp.path());
    let real = temp.path().join("real");
    let target = real.join(&remote.name);
    assert!(
        in_steps(
            &remote,
            &target,
            None,
            &Cancellation::default(),
            &Progress::default(),
            1
        )
        .is_err()
    );
    assert_eq!(resumable(&real, &remote), Some(target.clone()));
    // Reached by another name, it would be claimed under that name, and by two processes at once.
    let other = temp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::os::unix::fs::symlink(&target, other.join(&remote.name)).unwrap();
    assert_eq!(resumable(&other, &remote), None);
}

#[test]
fn a_parent_that_does_not_exist_yet_is_made_for_the_clone() {
    let temp = tempfile::tempdir().unwrap();
    let remote = origin(temp.path());
    // As on a fresh machine, where the default `~/Horizon` is not there until the first clone.
    let target = temp.path().join("home").join("Horizon").join(&remote.name);
    assert!(!target.parent().unwrap().exists());
    clone(&remote, &target, None, &Cancellation::default(), &Progress::default()).unwrap();
    assert!(super::super::is_checkout(&target));
}

#[test]
fn a_clone_that_ended_before_its_marker_was_removed_is_finished_and_never_discarded() {
    let temp = tempfile::tempdir().unwrap();
    let remote = origin(temp.path());
    let parent = temp.path().join("clones");
    let target = parent.join(&remote.name);
    let (cancel, progress) = (Cancellation::default(), Progress::default());
    clone(&remote, &target, None, &cancel, &progress).unwrap();
    // As if the process ended right after the checkout: the marker says every step is done.
    write_marker(
        &target,
        &Marker {
            url: remote.url.clone(),
            branch: "main".into(),
            done: DEPTHS.len() + 1,
        },
    )
    .unwrap();
    assert!(!unfinished(&target), "the checkout is complete");
    assert_eq!(resumable(&parent, &remote), None, "there is nothing to resume");
    assert_eq!(discard(&target), Ok(()));
    assert!(
        super::super::is_checkout(&target),
        "Start over never removes a finished checkout"
    );
    // Continuing only takes the marker away.
    clone(&remote, &target, None, &cancel, &progress).unwrap();
    assert!(marker(&target).is_none());
    assert!(super::super::is_checkout(&target));
}

#[test]
fn a_persons_own_ssh_command_is_kept_wherever_it_is_set() {
    let set = |scope: &str| (scope == "--global").then(|| "ssh -i ~/.ssh/work\n".to_owned());
    let none = |_: &str| None;
    let blank = |_: &str| Some("  \n".to_owned());
    assert!(ssh_command_in(true, none), "the environment");
    assert!(ssh_command_in(false, set), "the user's configuration");
    assert!(!ssh_command_in(false, none));
    assert!(!ssh_command_in(false, blank), "an empty one is not a command");
}

#[test]
fn the_checkout_tracks_its_branch_whatever_the_machines_git_settings_say() {
    let args: Vec<_> = checkout_command("main", None)
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert!(args.contains(&"--track".to_owned()), "{args:?}");
}

#[cfg(unix)]
#[test]
fn a_failed_step_that_cannot_clean_up_after_itself_says_where_it_left_the_folder() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("clones");
    std::fs::create_dir(&parent).unwrap();
    let folder = parent.join("stuck");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("file"), "x").unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();
    let writable = std::fs::write(parent.join("probe"), "").is_ok();
    let said = removed(&folder, Failure::Network);
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    if writable {
        return; // Permissions do not bind this user (root), so removal cannot fail here.
    }
    assert!(
        matches!(&said, Failure::Other(text) if text.contains("could not be removed") && text.contains("Cannot reach the host")),
        "{said:?}"
    );
    assert_eq!(
        removed(&temp.path().join("gone"), Failure::Network),
        Failure::Network,
        "nothing to remove is no news"
    );
}
